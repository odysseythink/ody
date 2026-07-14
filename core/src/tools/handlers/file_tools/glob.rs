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
use crate::tools::handlers::file_tools_spec::GLOB_TOOL_NAME;
use crate::tools::handlers::file_tools_spec::create_glob_tool;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use globset::Glob;
use ignore::WalkBuilder;
use ody_protocol::models::ResponseInputItem;
use ody_tools::ToolName;
use ody_tools::ToolSpec;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use serde_json::json;
use std::path::Path;
use std::time::SystemTime;

#[derive(Deserialize)]
struct GlobArgs {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    head_limit: Option<usize>,
    #[serde(default)]
    environment_id: Option<String>,
}

struct GlobOutput {
    paths: String,
    truncated: bool,
}

impl ToolOutput for GlobOutput {
    fn log_preview(&self) -> String {
        format!("{} paths", self.paths.lines().count())
    }

    fn success_for_logging(&self) -> bool {
        true
    }

    fn to_response_item(&self, call_id: &str, payload: &ToolPayload) -> ResponseInputItem {
        FunctionToolOutput::from_text(self.paths.clone(), Some(true))
            .to_response_item(call_id, payload)
    }

    fn code_mode_result(&self, _payload: &ToolPayload) -> JsonValue {
        json!({ "paths": self.paths, "truncated": self.truncated })
    }
}

#[derive(Default)]
pub struct GlobHandler {
    options: FileToolOptions,
}

impl GlobHandler {
    pub fn new(options: FileToolOptions) -> Self {
        Self { options }
    }
}

impl ToolExecutor<ToolInvocation> for GlobHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(GLOB_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_glob_tool(self.options)
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }

    fn handle(&self, invocation: ToolInvocation) -> ody_tools::ToolExecutorFuture<'_> {
        Box::pin(async move {
            let ToolInvocation { turn, payload, .. } = invocation;
            let ToolPayload::Function { arguments } = payload else {
                return Err(FunctionCallError::RespondToModel(format!(
                    "{GLOB_TOOL_NAME} handler received unsupported payload"
                )));
            };
            let args: GlobArgs = parse_arguments(&arguments)?;
            let root = local_search_root(
                turn.as_ref(),
                args.environment_id.as_deref(),
                args.path.as_deref(),
            )?;
            let (paths, truncated) = run(&args, root.as_path())?;
            Ok(boxed_tool_output(GlobOutput { paths, truncated }))
        })
    }
}

impl CoreToolRuntime for GlobHandler {}

/// A pattern with no literal anchor matches the entire tree. That is never the
/// cheapest way to answer a question, and the resulting dump is exactly the
/// context blowout these tools exist to prevent — so reject it rather than
/// silently returning `head_limit` arbitrary files.
fn reject_unanchored(pattern: &str) -> Result<(), FunctionCallError> {
    let anchored = pattern
        .split('/')
        .any(|segment| segment.chars().any(|c| !matches!(c, '*' | '?' | '.')));
    if anchored {
        return Ok(());
    }
    Err(FunctionCallError::RespondToModel(format!(
        "glob.pattern `{pattern}` is a pure wildcard and would enumerate the whole tree. Anchor it \
         with an extension or a path segment (e.g. `**/*.rs`, `core/src/**/*.rs`)."
    )))
}

#[cfg(test)]
pub(super) fn reject_unanchored_for_test(pattern: &str) -> Result<(), FunctionCallError> {
    reject_unanchored(pattern)
}

#[cfg(test)]
pub(super) fn run_for_test(
    pattern: &str,
    root: &Path,
) -> Result<(String, bool), FunctionCallError> {
    run(
        &GlobArgs {
            pattern: pattern.to_string(),
            path: None,
            head_limit: None,
            environment_id: None,
        },
        root,
    )
}

fn run(args: &GlobArgs, root: &Path) -> Result<(String, bool), FunctionCallError> {
    reject_unanchored(&args.pattern)?;
    let matcher = Glob::new(&args.pattern)
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!("glob.pattern is not a valid glob: {err}"))
        })?
        .compile_matcher();

    let mut hits: Vec<(SystemTime, String)> = Vec::new();
    for entry in WalkBuilder::new(root).build().flatten() {
        if !entry
            .file_type()
            .is_some_and(|file_type| file_type.is_file())
        {
            continue;
        }
        let path = entry.path();
        let relative = path.strip_prefix(root).unwrap_or(path);
        if !matcher.is_match(relative) {
            continue;
        }
        let modified = entry
            .metadata()
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        hits.push((modified, relative.display().to_string()));
    }

    // Newest first: when a model asks "where are the X files", the ones touched
    // most recently are overwhelmingly the ones it means.
    hits.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));

    let total = hits.len();
    let limit = args.head_limit.unwrap_or(DEFAULT_HEAD_LIMIT);
    let shown: Vec<String> = hits.into_iter().take(limit).map(|(_, path)| path).collect();
    let count = shown.len();
    let mut out = if shown.is_empty() {
        "No files matched.".to_string()
    } else {
        shown.join("\n")
    };
    if let Some(notice) = pagination_notice(total, count, 0) {
        out.push_str(&notice);
        return Ok((out, true));
    }
    Ok((out, false))
}
