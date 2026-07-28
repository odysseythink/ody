use super::spec::DEFAULT_SEARCH_LIMIT;
use super::spec::ROLLOUT_SEARCH_TOOL_NAME;
use super::spec::create_rollout_search_tool;
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
use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::path::PathBuf;

#[derive(Deserialize)]
struct RolloutSearchArgs {
    search_term: String,
    #[serde(default)]
    archived: Option<bool>,
    #[serde(default)]
    head_limit: Option<i64>,
}

struct RolloutSearchOutput {
    matches: String,
    truncated: bool,
}

impl ToolOutput for RolloutSearchOutput {
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
pub struct RolloutSearchHandler {
    options: RolloutToolOptions,
}

impl RolloutSearchHandler {
    pub fn new(options: RolloutToolOptions) -> Self {
        Self { options }
    }
}

impl ToolExecutor<ToolInvocation> for RolloutSearchHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(ROLLOUT_SEARCH_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_rollout_search_tool(self.options)
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }

    fn handle(&self, invocation: ToolInvocation) -> ody_tools::ToolExecutorFuture<'_> {
        Box::pin(async move {
            let ToolInvocation { turn, payload, .. } = invocation;
            let ToolPayload::Function { arguments } = payload else {
                return Err(FunctionCallError::RespondToModel(format!(
                    "{ROLLOUT_SEARCH_TOOL_NAME} handler received unsupported payload"
                )));
            };
            let args: RolloutSearchArgs = parse_arguments(&arguments)?;
            let ody_home = turn.config.ody_home.to_path_buf();
            let archived = args.archived.unwrap_or(false);
            let limit = args
                .head_limit
                .filter(|limit| *limit > 0)
                .map_or(DEFAULT_SEARCH_LIMIT, |limit| {
                    (limit as usize).min(DEFAULT_SEARCH_LIMIT)
                });

            let (matches, truncated) = search_rollout(
                ody_home.as_path(),
                archived,
                args.search_term.as_str(),
                limit,
            )
            .await?;
            Ok(boxed_tool_output(RolloutSearchOutput {
                matches,
                truncated,
            }))
        })
    }
}

impl CoreToolRuntime for RolloutSearchHandler {}

async fn search_rollout(
    ody_home: &Path,
    archived: bool,
    search_term: &str,
    limit: usize,
) -> Result<(String, bool), FunctionCallError> {
    let rg_command = Path::new("rg");
    let matches: HashMap<PathBuf, Option<String>> =
        ody_rollout::search_rollout_matches(rg_command, ody_home, archived, search_term)
            .await
            .map_err(|err| {
                FunctionCallError::RespondToModel(format!(
                    "rollout search failed under {}: {err}",
                    ody_home.display()
                ))
            })?;

    let total = matches.len();
    let mut rows = Vec::with_capacity(total.min(limit));
    for (path, snippet) in matches.into_iter().take(limit) {
        match snippet {
            Some(snippet) => rows.push(format!("{}: {}", path.display(), snippet)),
            None => rows.push(path.display().to_string()),
        }
    }

    let truncated = total > rows.len();
    let mut out = if rows.is_empty() {
        "No matching rollout files found.".to_string()
    } else {
        rows.join("\n")
    };
    if truncated {
        out.push_str(&format!(
            "\n\n[showing {} of {}; use head_limit to see more]",
            rows.len(),
            total
        ));
    }
    Ok((out, truncated))
}

fn _map_io_error(path: &Path, err: io::Error) -> FunctionCallError {
    FunctionCallError::RespondToModel(format!(
        "rollout search failed under {}: {err}",
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[tokio::test]
    async fn finds_matching_rollout_files() {
        let dir = tempdir().expect("tempdir");
        let ody_home = dir.path();
        let sessions = ody_home.join("sessions");
        std::fs::create_dir_all(&sessions).expect("create sessions");
        let mut file = std::fs::File::create(sessions.join("rollout.jsonl")).expect("create");
        writeln!(file, "{{\"text\":\"hello world\"}}").expect("write");

        let (matches, truncated) = search_rollout(ody_home, false, "hello world", 10)
            .await
            .expect("search");
        assert!(!truncated);
        assert!(matches.contains("rollout.jsonl"), "{matches}");
    }

    #[tokio::test]
    async fn paginates_results() {
        let dir = tempdir().expect("tempdir");
        let ody_home = dir.path();
        let sessions = ody_home.join("sessions");
        std::fs::create_dir_all(&sessions).expect("create sessions");
        for i in 0..3 {
            let mut file =
                std::fs::File::create(sessions.join(format!("rollout{i}.jsonl"))).expect("create");
            writeln!(file, "{{\"text\":\"hello world{i}\"}}").expect("write");
        }

        let (matches, truncated) = search_rollout(ody_home, false, "hello world", 2)
            .await
            .expect("search");
        assert!(truncated);
        assert!(
            matches.contains("[showing 2 of 3; use head_limit to see more]"),
            "{matches}"
        );
    }
}
