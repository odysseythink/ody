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
            )?;

            let bytes = std::fs::read(abs_path.as_path()).map_err(|err| {
                FunctionCallError::RespondToModel(format!(
                    "unable to read `{}`: {err}",
                    abs_path.as_path().display()
                ))
            })?;

            // Cap bytes before decoding so a huge binary blob cannot be
            // materialized as a String just to be thrown away.
            let byte_capped = bytes.len() > MAX_BYTES;
            let slice = if byte_capped {
                &bytes[..floor_char_boundary(&bytes, MAX_BYTES)]
            } else {
                &bytes[..]
            };
            let text = String::from_utf8_lossy(slice);

            let (content, line_capped) = render(&text, args.offset, args.limit)?;
            Ok(boxed_tool_output(ReadFileOutput {
                content,
                truncated: byte_capped || line_capped,
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

/// Largest index <= `max` that is a UTF-8 character boundary.
fn floor_char_boundary(bytes: &[u8], max: usize) -> usize {
    let mut index = max.min(bytes.len());
    while index > 0 && (bytes[index] & 0b1100_0000) == 0b1000_0000 {
        index -= 1;
    }
    index
}
