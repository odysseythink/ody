use super::PathAccessMode;
use super::local_search_root;
use crate::function_tool::FunctionCallError;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::file_tools_spec::FileToolOptions;
use crate::tools::handlers::file_tools_spec::MAX_BYTES;
use crate::tools::handlers::file_tools_spec::MAX_LINE_LENGTH;
use crate::tools::handlers::file_tools_spec::MAX_LINES;
use crate::tools::handlers::file_tools_spec::READ_FILE_TOOL_NAME;
use crate::tools::handlers::file_tools_spec::create_read_file_tool;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use ody_protocol::models::ResponseInputItem;
use ody_tools::ToolName;
use ody_tools::ToolSpec;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use serde_json::json;
use std::io::BufRead;

#[derive(Deserialize)]
struct ReadFileArgs {
    path: String,
    #[serde(default)]
    offset: Option<i64>,
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    environment_id: Option<String>,
}

struct ReadFileOutput {
    content: String,
    truncated: bool,
}

impl ToolOutput for ReadFileOutput {
    fn log_preview(&self) -> String {
        format!("{} lines", self.content.lines().count())
    }

    fn success_for_logging(&self) -> bool {
        true
    }

    fn to_response_item(&self, call_id: &str, payload: &ToolPayload) -> ResponseInputItem {
        FunctionToolOutput::from_text(self.content.clone(), Some(true))
            .to_response_item(call_id, payload)
    }

    fn code_mode_result(&self, _payload: &ToolPayload) -> JsonValue {
        json!({ "content": self.content, "truncated": self.truncated })
    }
}

#[derive(Default)]
pub struct ReadFileHandler {
    options: FileToolOptions,
}

impl ReadFileHandler {
    pub fn new(options: FileToolOptions) -> Self {
        Self { options }
    }
}

impl ToolExecutor<ToolInvocation> for ReadFileHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(READ_FILE_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_read_file_tool(self.options)
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }

    fn handle(&self, invocation: ToolInvocation) -> ody_tools::ToolExecutorFuture<'_> {
        Box::pin(async move {
            let ToolInvocation { turn, payload, .. } = invocation;
            let ToolPayload::Function { arguments } = payload else {
                return Err(FunctionCallError::RespondToModel(format!(
                    "{READ_FILE_TOOL_NAME} handler received unsupported payload"
                )));
            };
            let args: ReadFileArgs = parse_arguments(&arguments)?;
            let abs_path = local_search_root(
                turn.as_ref(),
                args.environment_id.as_deref(),
                Some(&args.path),
                PathAccessMode::AbsoluteOutsideAllowed,
            )?;

            let file = std::fs::File::open(abs_path.as_path()).map_err(|err| {
                FunctionCallError::RespondToModel(format!(
                    "unable to read `{}`: {err}",
                    abs_path.as_path().display()
                ))
            })?;
            let reader = std::io::BufReader::new(file);
            let mut lines = reader.lines();
            let mut text = String::new();
            let mut line_count = 0usize;
            let mut byte_capped = false;
            let mut line_capped = false;

            while let Some(line) = lines.next() {
                let line = line.map_err(|err| {
                    FunctionCallError::RespondToModel(format!(
                        "unable to read `{}`: {err}",
                        abs_path.as_path().display()
                    ))
                })?;
                if text.len() + line.len() + 1 > MAX_BYTES {
                    byte_capped = true;
                    break;
                }
                text.push_str(&line);
                text.push('\n');
                line_count += 1;
                if line_count >= MAX_LINES {
                    if lines.next().is_some() {
                        line_capped = true;
                    }
                    break;
                }
            }

            let (mut content, render_truncated) = render(&text, args.offset, args.limit)?;
            append_jq_hint_if_json(abs_path.as_path(), &mut content);
            Ok(boxed_tool_output(ReadFileOutput {
                content,
                truncated: byte_capped || line_capped || render_truncated,
            }))
        })
    }
}

impl CoreToolRuntime for ReadFileHandler {}

/// Renders the requested window as `cat -n`-style numbered lines.
///
/// Returns the rendered text and whether lines were left unread.
fn render(
    text: &str,
    offset: Option<i64>,
    limit: Option<i64>,
) -> Result<(String, bool), FunctionCallError> {
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();

    let start = match offset {
        None => 0usize,
        Some(0) => {
            return Err(FunctionCallError::RespondToModel(
                "read_file.offset is 1-based; use 1 for the first line, or a negative value to \
                 read from the end of the file"
                    .to_string(),
            ));
        }
        // Negative offset reads from the end: -50 means "the last 50 lines".
        Some(negative) if negative < 0 => {
            let back = negative.unsigned_abs() as usize;
            if back > MAX_LINES {
                return Err(FunctionCallError::RespondToModel(format!(
                    "read_file.offset cannot look further back than {MAX_LINES} lines"
                )));
            }
            total.saturating_sub(back)
        }
        Some(positive) => (positive as usize).saturating_sub(1),
    };

    let requested = limit
        .filter(|limit| *limit > 0)
        .map_or(MAX_LINES, |limit| (limit as usize).min(MAX_LINES));
    let end = start.saturating_add(requested).min(total);

    let mut out = String::new();
    for (index, line) in lines[start.min(total)..end].iter().enumerate() {
        let number = start + index + 1;
        let line = truncate_line(line);
        out.push_str(&format!("{number:6}\t{line}\n"));
    }

    let unread = end < total;
    if unread {
        out.push_str(&format!(
            "\n[truncated at line {end} of {total}; use offset={} to continue]\n",
            end + 1
        ));
    }
    Ok((out, unread))
}

#[cfg(test)]
pub(super) fn render_for_test(
    text: &str,
    offset: Option<i64>,
    limit: Option<i64>,
) -> Result<(String, bool), FunctionCallError> {
    render(text, offset, limit)
}

fn truncate_line(line: &str) -> String {
    if line.chars().count() <= MAX_LINE_LENGTH {
        return line.to_string();
    }
    let kept: String = line.chars().take(MAX_LINE_LENGTH).collect();
    format!("{kept}… [line truncated]")
}

/// Appends a hint pointing models at `jq` when the file is JSON/JSONL.
fn append_jq_hint_if_json(path: &std::path::Path, content: &mut String) {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return;
    };
    if ext.eq_ignore_ascii_case("json") || ext.eq_ignore_ascii_case("jsonl") {
        content.push_str(
            "\n\nThis file is JSON/JSONL. Use `jq` if you need to filter, count, or paginate \
             structured data instead of reading raw lines.",
        );
    }
}

#[cfg(test)]
mod read_tests {
    use super::append_jq_hint_if_json;
    use std::path::PathBuf;

    #[test]
    fn appends_hint_for_json() {
        let mut content = "     1\t{}\n".to_string();
        append_jq_hint_if_json(&PathBuf::from("data.json"), &mut content);
        assert!(
            content.contains("Use `jq`"),
            "JSON file should get jq hint: {content}"
        );
    }

    #[test]
    fn appends_hint_for_jsonl() {
        let mut content = "     1\t{}\n".to_string();
        append_jq_hint_if_json(&PathBuf::from("log.jsonl"), &mut content);
        assert!(
            content.contains("Use `jq`"),
            "JSONL file should get jq hint: {content}"
        );
    }

    #[test]
    fn appends_hint_for_uppercase_json_extension() {
        let mut content = "     1\t{}\n".to_string();
        append_jq_hint_if_json(&PathBuf::from("config.JSON"), &mut content);
        assert!(
            content.contains("Use `jq`"),
            "uppercase JSON extension should get jq hint: {content}"
        );
    }

    #[test]
    fn skips_hint_for_non_json() {
        let mut content = "     1\tfoo\n".to_string();
        append_jq_hint_if_json(&PathBuf::from("README.md"), &mut content);
        assert!(
            !content.contains("Use `jq`"),
            "non-JSON file should not get jq hint: {content}"
        );
    }

    #[test]
    fn skips_hint_for_no_extension() {
        let mut content = "     1\t{}\n".to_string();
        append_jq_hint_if_json(&PathBuf::from("Makefile"), &mut content);
        assert!(
            !content.contains("Use `jq`"),
            "file without extension should not get jq hint: {content}"
        );
    }
}
