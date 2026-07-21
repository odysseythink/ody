use super::PathAccessMode;
use super::local_search_root;
use super::pagination_notice;
use crate::function_tool::FunctionCallError;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::file_tools_spec::DEFAULT_HEAD_LIMIT;
use crate::tools::handlers::file_tools_spec::FileToolOptions;
use crate::tools::handlers::file_tools_spec::GREP_TOOL_NAME;
use crate::tools::handlers::file_tools_spec::MAX_LINE_LENGTH;
use crate::tools::handlers::file_tools_spec::create_grep_tool;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use globset::Glob;
use ignore::WalkBuilder;
use ody_protocol::models::ResponseInputItem;
use ody_tools::ToolName;
use ody_tools::ToolSpec;
use regex::RegexBuilder;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use serde_json::json;
use std::path::Path;

/// Files above this size are skipped: a match inside a multi-megabyte generated
/// artifact is almost never what the model is looking for, and reading them
/// dominates the walk.
const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Deserialize)]
struct GrepArgs {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    glob: Option<String>,
    #[serde(default)]
    output_mode: Option<String>,
    #[serde(default)]
    head_limit: Option<usize>,
    #[serde(default)]
    offset: Option<usize>,
    #[serde(rename = "-n", default)]
    line_numbers: Option<bool>,
    #[serde(rename = "-A", default)]
    after: Option<usize>,
    #[serde(rename = "-B", default)]
    before: Option<usize>,
    #[serde(rename = "-C", default)]
    context: Option<usize>,
    #[serde(rename = "-i", default)]
    case_insensitive: Option<bool>,
    #[serde(default)]
    environment_id: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OutputMode {
    FilesWithMatches,
    Content,
    CountMatches,
}

struct GrepOutput {
    matches: String,
    truncated: bool,
}

impl ToolOutput for GrepOutput {
    fn log_preview(&self) -> String {
        format!("{} rows", self.matches.lines().count())
    }

    fn success_for_logging(&self) -> bool {
        true
    }

    fn to_response_item(&self, call_id: &str, payload: &ToolPayload) -> ResponseInputItem {
        FunctionToolOutput::from_text(self.matches.clone(), Some(true))
            .to_response_item(call_id, payload)
    }

    fn code_mode_result(&self, _payload: &ToolPayload) -> JsonValue {
        json!({ "matches": self.matches, "truncated": self.truncated })
    }
}

#[derive(Default)]
pub struct GrepHandler {
    options: FileToolOptions,
}

impl GrepHandler {
    pub fn new(options: FileToolOptions) -> Self {
        Self { options }
    }
}

impl ToolExecutor<ToolInvocation> for GrepHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(GREP_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_grep_tool(self.options)
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }

    fn handle(&self, invocation: ToolInvocation) -> ody_tools::ToolExecutorFuture<'_> {
        Box::pin(async move {
            let ToolInvocation { turn, payload, .. } = invocation;
            let ToolPayload::Function { arguments } = payload else {
                return Err(FunctionCallError::RespondToModel(format!(
                    "{GREP_TOOL_NAME} handler received unsupported payload"
                )));
            };
            let args: GrepArgs = parse_arguments(&arguments)?;
            let root = local_search_root(
                turn.as_ref(),
                args.environment_id.as_deref(),
                args.path.as_deref(),
                PathAccessMode::WorkspaceRelativeOnly,
            )?;
            let (matches, truncated) = run(&args, root.as_path())?;
            Ok(boxed_tool_output(GrepOutput { matches, truncated }))
        })
    }
}

impl CoreToolRuntime for GrepHandler {}

fn parse_output_mode(raw: Option<&str>) -> Result<OutputMode, FunctionCallError> {
    // The default is the whole point of this tool: paths, not contents.
    match raw {
        None | Some("files_with_matches") => Ok(OutputMode::FilesWithMatches),
        Some("content") => Ok(OutputMode::Content),
        Some("count_matches") => Ok(OutputMode::CountMatches),
        Some(other) => Err(FunctionCallError::RespondToModel(format!(
            "grep.output_mode must be `files_with_matches`, `content`, or `count_matches`, got \
             `{other}`"
        ))),
    }
}

fn run(args: &GrepArgs, root: &Path) -> Result<(String, bool), FunctionCallError> {
    let mode = parse_output_mode(args.output_mode.as_deref())?;
    let regex = RegexBuilder::new(&args.pattern)
        .case_insensitive(args.case_insensitive.unwrap_or(false))
        .build()
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!(
                "grep.pattern is not a valid regular expression: {err}"
            ))
        })?;

    let glob = args
        .glob
        .as_deref()
        .map(|pattern| {
            Glob::new(pattern)
                .map(|glob| glob.compile_matcher())
                .map_err(|err| {
                    FunctionCallError::RespondToModel(format!(
                        "grep.glob is not a valid glob: {err}"
                    ))
                })
        })
        .transpose()?;

    let (before, after) = match args.context {
        Some(context) => (context, context),
        None => (args.before.unwrap_or(0), args.after.unwrap_or(0)),
    };
    let show_line_numbers = args.line_numbers.unwrap_or(true);

    let mut paths: Vec<String> = Vec::new();
    let mut content_rows: Vec<String> = Vec::new();
    let mut match_count = 0usize;

    // WalkBuilder honours .gitignore/.ignore and skips hidden files by default,
    // which is what keeps a broad search from drowning in target/ and node_modules/.
    for entry in WalkBuilder::new(root).build().flatten() {
        if !entry
            .file_type()
            .is_some_and(|file_type| file_type.is_file())
        {
            continue;
        }
        let path = entry.path();
        if let Some(glob) = &glob {
            let relative = path.strip_prefix(root).unwrap_or(path);
            if !glob.is_match(relative) {
                continue;
            }
        }
        if entry
            .metadata()
            .ok()
            .is_some_and(|metadata| metadata.len() > MAX_FILE_BYTES)
        {
            continue;
        }
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        // Binary files: a NUL in the first block is the standard heuristic.
        if bytes.iter().take(1024).any(|byte| *byte == 0) {
            continue;
        }
        let Ok(text) = std::str::from_utf8(&bytes) else {
            continue;
        };

        let display = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let lines: Vec<&str> = text.lines().collect();
        let mut file_matched = false;

        for (index, line) in lines.iter().enumerate() {
            if !regex.is_match(line) {
                continue;
            }
            file_matched = true;
            match_count += 1;
            if mode == OutputMode::FilesWithMatches {
                // Only this mode can stop early: it reports the file, so one
                // match is enough. `count_matches` must keep counting every
                // match in the file, and `content` must collect every line.
                break;
            }
            if mode == OutputMode::CountMatches {
                continue;
            }
            let low = index.saturating_sub(before);
            let high = (index + after + 1).min(lines.len());
            for (context_index, context_line) in lines[low..high].iter().enumerate() {
                let number = low + context_index + 1;
                let rendered = clip(context_line);
                content_rows.push(if show_line_numbers {
                    format!("{display}:{number}:{rendered}")
                } else {
                    format!("{display}:{rendered}")
                });
            }
        }

        if file_matched && mode == OutputMode::FilesWithMatches {
            paths.push(display);
        }
    }

    if mode == OutputMode::CountMatches {
        return Ok((format!("{match_count}"), false));
    }

    let rows = if mode == OutputMode::FilesWithMatches {
        paths
    } else {
        content_rows
    };
    Ok(paginate(rows, args.head_limit, args.offset.unwrap_or(0)))
}

fn paginate(rows: Vec<String>, head_limit: Option<usize>, offset: usize) -> (String, bool) {
    let total = rows.len();
    let limit = head_limit.unwrap_or(DEFAULT_HEAD_LIMIT);
    let window: Vec<String> = rows.into_iter().skip(offset).take(limit).collect();
    let shown = window.len();
    let mut out = if window.is_empty() {
        "No matches found.".to_string()
    } else {
        window.join("\n")
    };
    let seen = offset + shown;
    if let Some(notice) = pagination_notice(total, seen, 0) {
        out.push_str(&notice);
        return (out, true);
    }
    (out, false)
}

#[cfg(test)]
pub(super) fn run_for_test(
    pattern: &str,
    output_mode: Option<&str>,
    root: &Path,
) -> Result<(String, bool), FunctionCallError> {
    run(
        &GrepArgs {
            pattern: pattern.to_string(),
            path: None,
            glob: None,
            output_mode: output_mode.map(str::to_string),
            head_limit: None,
            offset: None,
            line_numbers: None,
            after: None,
            before: None,
            context: None,
            case_insensitive: None,
            environment_id: None,
        },
        root,
    )
}

fn clip(line: &str) -> String {
    if line.chars().count() <= MAX_LINE_LENGTH {
        return line.to_string();
    }
    line.chars().take(MAX_LINE_LENGTH).collect::<String>() + "… [line truncated]"
}
