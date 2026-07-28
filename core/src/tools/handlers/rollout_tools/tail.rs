use super::append_truncation_notice;
use super::clip_record_line;
use super::render_numbered_records;
use super::resolve_rollout_path;
use super::spec::DEFAULT_TAIL_RECORDS;
use super::spec::MAX_TAIL_RECORDS;
use super::spec::ROLLOUT_TAIL_TOOL_NAME;
use super::spec::create_rollout_tail_tool;
use crate::function_tool::FunctionCallError;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::rollout_tools::spec::RolloutToolOptions;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use ody_protocol::models::ResponseInputItem;
use ody_tools::ToolName;
use ody_tools::ToolSpec;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use serde_json::json;
use std::io;

#[derive(Deserialize)]
struct RolloutTailArgs {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    thread_id: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}

struct RolloutTailOutput {
    content: String,
    truncated: bool,
}

impl ToolOutput for RolloutTailOutput {
    fn log_preview(&self) -> String {
        format!("{} records", self.content.lines().count())
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
pub struct RolloutTailHandler {
    options: RolloutToolOptions,
}

impl RolloutTailHandler {
    pub fn new(options: RolloutToolOptions) -> Self {
        Self { options }
    }
}

impl ToolExecutor<ToolInvocation> for RolloutTailHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(ROLLOUT_TAIL_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_rollout_tail_tool(self.options)
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }

    fn handle(&self, invocation: ToolInvocation) -> ody_tools::ToolExecutorFuture<'_> {
        Box::pin(async move {
            let ToolInvocation { turn, payload, .. } = invocation;
            let ToolPayload::Function { arguments } = payload else {
                return Err(FunctionCallError::RespondToModel(format!(
                    "{ROLLOUT_TAIL_TOOL_NAME} handler received unsupported payload"
                )));
            };
            let args: RolloutTailArgs = parse_arguments(&arguments)?;
            let path = resolve_rollout_path(
                turn.as_ref(),
                args.path.as_deref(),
                args.thread_id.as_deref(),
            )
            .await?;

            let limit = args
                .limit
                .filter(|limit| *limit > 0)
                .map_or(DEFAULT_TAIL_RECORDS, |limit| {
                    (limit as usize).min(MAX_TAIL_RECORDS)
                });

            let (content, truncated) = tail_records(path.as_path(), limit).await?;
            Ok(boxed_tool_output(RolloutTailOutput { content, truncated }))
        })
    }
}

impl CoreToolRuntime for RolloutTailHandler {}

async fn tail_records(
    path: &std::path::Path,
    limit: usize,
) -> Result<(String, bool), FunctionCallError> {
    let mut scanner = ody_rollout::open_rollout_reverse_scanner(path)
        .await
        .map_err(|err| map_io_error(path, err))?;

    let mut records = Vec::with_capacity(limit);
    let mut parsed_count = 0usize;
    let mut truncated = false;

    while parsed_count < limit {
        match scanner.next_record::<serde_json::Value>().await {
            Ok(Some(ody_rollout::ScanOutcome::Parsed(value))) => {
                parsed_count += 1;
                records.push(clip_record_line(value.to_string().as_str()));
            }
            Ok(Some(ody_rollout::ScanOutcome::Rejected(_))) => {
                // Skip malformed records; the scanner remains usable.
                continue;
            }
            Ok(None) => break,
            Err(err) => return Err(map_io_error(path, err)),
        }
    }

    // If we stopped because we hit the limit, there are older records.
    if parsed_count == limit {
        truncated = true;
    }

    // Records are already newest-first; render them as-is.
    let mut content = render_numbered_records(0, &records);
    append_truncation_notice(&mut content, truncated);
    Ok((content, truncated))
}

fn map_io_error(path: &std::path::Path, err: io::Error) -> FunctionCallError {
    FunctionCallError::RespondToModel(format!(
        "unable to tail rollout `{}`: {err}",
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[tokio::test]
    async fn returns_newest_records_first() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("rollout.jsonl");
        let mut file = std::fs::File::create(&path).expect("create");
        for i in 1..=5 {
            writeln!(file, "{{\"id\":{i}}}").expect("write");
        }

        let (content, truncated) = tail_records(path.as_path(), 3).await.expect("tail");
        assert!(truncated);
        let record_lines: Vec<&str> = content
            .lines()
            .filter(|line| line.contains("id\""))
            .collect();
        assert_eq!(record_lines.len(), 3);
        assert!(record_lines[0].contains("\"id\":5"), "{content}");
        assert!(record_lines[1].contains("\"id\":4"), "{content}");
        assert!(record_lines[2].contains("\"id\":3"), "{content}");
    }

    #[tokio::test]
    async fn returns_all_records_when_file_has_fewer_than_limit() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("rollout.jsonl");
        let mut file = std::fs::File::create(&path).expect("create");
        for i in 1..=3 {
            writeln!(file, "{{\"id\":{i}}}").expect("write");
        }

        let (content, truncated) = tail_records(path.as_path(), 5).await.expect("tail");
        assert!(!truncated);
        assert_eq!(content.lines().count(), 3);
    }
}
