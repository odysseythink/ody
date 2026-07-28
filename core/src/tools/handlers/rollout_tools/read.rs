use super::append_truncation_notice;
use super::clip_record_line;
use super::render_numbered_records;
use super::resolve_rollout_path;
use super::spec::MAX_READ_RECORDS;
use super::spec::ROLLOUT_READ_TOOL_NAME;
use super::spec::create_rollout_read_tool;
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
struct RolloutReadArgs {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    thread_id: Option<String>,
    #[serde(default)]
    offset: Option<i64>,
    #[serde(default)]
    limit: Option<i64>,
}

struct RolloutReadOutput {
    content: String,
    truncated: bool,
}

impl ToolOutput for RolloutReadOutput {
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
pub struct RolloutReadHandler {
    options: RolloutToolOptions,
}

impl RolloutReadHandler {
    pub fn new(options: RolloutToolOptions) -> Self {
        Self { options }
    }
}

impl ToolExecutor<ToolInvocation> for RolloutReadHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(ROLLOUT_READ_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_rollout_read_tool(self.options)
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }

    fn handle(&self, invocation: ToolInvocation) -> ody_tools::ToolExecutorFuture<'_> {
        Box::pin(async move {
            let ToolInvocation { turn, payload, .. } = invocation;
            let ToolPayload::Function { arguments } = payload else {
                return Err(FunctionCallError::RespondToModel(format!(
                    "{ROLLOUT_READ_TOOL_NAME} handler received unsupported payload"
                )));
            };
            let args: RolloutReadArgs = parse_arguments(&arguments)?;
            let path = resolve_rollout_path(
                turn.as_ref(),
                args.path.as_deref(),
                args.thread_id.as_deref(),
            )
            .await?;

            let (content, truncated) = read_records(&path, args.offset, args.limit).await?;
            Ok(boxed_tool_output(RolloutReadOutput { content, truncated }))
        })
    }
}

impl CoreToolRuntime for RolloutReadHandler {}

async fn read_records(
    path: &std::path::Path,
    offset: Option<i64>,
    limit: Option<i64>,
) -> Result<(String, bool), FunctionCallError> {
    let requested_limit = limit
        .filter(|limit| *limit > 0)
        .map_or(MAX_READ_RECORDS, |limit| {
            (limit as usize).min(MAX_READ_RECORDS)
        });

    if let Some(offset) = offset {
        if offset == 0 {
            return Err(FunctionCallError::RespondToModel(
                "rollout_read.offset is 1-based; use 1 for the first record, or a negative value to \
                 read from the end of the file"
                    .to_string(),
            ));
        }
        if offset < 0 && offset.unsigned_abs() as usize > MAX_READ_RECORDS {
            return Err(FunctionCallError::RespondToModel(format!(
                "rollout_read.offset cannot look further back than {MAX_READ_RECORDS} records"
            )));
        }
    }

    let (mut records, truncated) = if let Some(offset) = offset.filter(|offset| *offset < 0) {
        read_from_end(path, offset.unsigned_abs() as usize, requested_limit).await?
    } else {
        read_from_start(path, offset.unwrap_or(1) as usize, requested_limit).await?
    };

    append_truncation_notice(&mut records, truncated);
    Ok((records, truncated))
}

async fn read_from_start(
    path: &std::path::Path,
    start_record: usize,
    requested_limit: usize,
) -> Result<(String, bool), FunctionCallError> {
    let mut reader = ody_rollout::open_rollout_line_reader(path)
        .await
        .map_err(|err| map_io_error(path, err))?;

    let mut records = Vec::with_capacity(requested_limit);
    let mut line_number = 0usize;
    let mut truncated = false;

    while records.len() < requested_limit {
        match reader.next_line().await {
            Ok(Some(line)) => {
                line_number += 1;
                if line_number < start_record {
                    continue;
                }
                records.push(clip_record_line(line.as_str()));
            }
            Ok(None) => break,
            Err(err) => return Err(map_io_error(path, err)),
        }
    }

    // Detect whether more records follow the returned window.
    if records.len() == requested_limit
        && reader
            .next_line()
            .await
            .map(|opt| opt.is_some())
            .unwrap_or(false)
    {
        truncated = true;
    }

    let content = render_numbered_records(start_record.saturating_sub(1), &records);
    Ok((content, truncated))
}

async fn read_from_end(
    path: &std::path::Path,
    back_records: usize,
    requested_limit: usize,
) -> Result<(String, bool), FunctionCallError> {
    let mut scanner = ody_rollout::open_rollout_reverse_scanner(path)
        .await
        .map_err(|err| map_io_error(path, err))?;

    let mut records = Vec::with_capacity(back_records.min(requested_limit));
    let mut parsed_count = 0usize;

    while parsed_count < back_records {
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

    // Reverse so the returned window is in forward order (oldest of the tail first).
    records.reverse();
    if records.len() > requested_limit {
        records.truncate(requested_limit);
    }

    let content = render_numbered_records(0, &records);
    Ok((content, false))
}

fn map_io_error(path: &std::path::Path, err: io::Error) -> FunctionCallError {
    FunctionCallError::RespondToModel(format!(
        "unable to read rollout `{}`: {err}",
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[tokio::test]
    async fn reads_from_start_with_offset_and_limit() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("rollout.jsonl");
        let mut file = std::fs::File::create(&path).expect("create");
        for i in 1..=3 {
            writeln!(file, "{{\"id\":{i}}}").expect("write");
        }

        let (content, truncated) = read_records(path.as_path(), Some(2), Some(2))
            .await
            .expect("read");
        assert!(!truncated);
        assert!(content.contains("2\t{\"id\":2}"), "{content}");
        assert!(content.contains("3\t{\"id\":3}"), "{content}");
        assert!(!content.contains("{\"id\":1}"), "{content}");
    }

    #[tokio::test]
    async fn reads_from_end_with_negative_offset() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("rollout.jsonl");
        let mut file = std::fs::File::create(&path).expect("create");
        for i in 1..=5 {
            writeln!(file, "{{\"id\":{i}}}").expect("write");
        }

        let (content, truncated) = read_records(path.as_path(), Some(-2), Some(1))
            .await
            .expect("read");
        assert!(!truncated);
        // Last two are id=5, id=4; limit 1 keeps only id=4 (forward order of the tail).
        assert!(content.contains("1\t{\"id\":4}"), "{content}");
        assert!(!content.contains("{\"id\":5}"), "{content}");
    }

    #[tokio::test]
    async fn detects_truncation_when_more_records_follow() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("rollout.jsonl");
        let mut file = std::fs::File::create(&path).expect("create");
        for i in 1..=5 {
            writeln!(file, "{{\"id\":{i}}}").expect("write");
        }

        let (content, truncated) = read_records(path.as_path(), Some(1), Some(2))
            .await
            .expect("read");
        assert!(truncated);
        assert!(content.contains("[truncated"), "{content}");
    }
}
