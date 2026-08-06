use crate::function_tool::FunctionCallError;
use crate::state::PhaseRoadmapState;
use crate::tools::context::{
    FunctionToolOutput, ToolInvocation, ToolOutput, ToolPayload, boxed_tool_output,
};
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::submit_roadmap_spec::{
    SUBMIT_ROADMAP_TOOL_NAME, create_submit_roadmap_tool,
};
use crate::tools::registry::{CoreToolRuntime, ToolExecutor};
use ody_protocol::config_types::ModeKind;
use ody_protocol::request_user_input::{
    RequestUserInputArgs, RequestUserInputQuestion, RequestUserInputQuestionOption,
};
use ody_tools::{ToolName, ToolSpec};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SubmitRoadmapArgs {
    roadmap: String,
    slug: String,
}

pub struct SubmitRoadmapHandler;

impl ToolExecutor<ToolInvocation> for SubmitRoadmapHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(SUBMIT_ROADMAP_TOOL_NAME)
    }
    fn spec(&self) -> ToolSpec {
        create_submit_roadmap_tool()
    }
    fn handle(&self, invocation: ToolInvocation) -> ody_tools::ToolExecutorFuture<'_> {
        Box::pin(self.handle_call(invocation))
    }
}
impl CoreToolRuntime for SubmitRoadmapHandler {}

impl SubmitRoadmapHandler {
    async fn handle_call(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            call_id,
            payload,
            ..
        } = invocation;
        if turn.collaboration_mode.mode != ModeKind::Design {
            return Err(FunctionCallError::RespondToModel(
                "submit_roadmap is only available in Design mode".to_string(),
            ));
        }
        let ToolPayload::Function { arguments } = payload else {
            return Err(FunctionCallError::RespondToModel(
                "submit_roadmap handler received unsupported payload".to_string(),
            ));
        };
        let args: SubmitRoadmapArgs = parse_arguments(&arguments)?;
        if args.slug.is_empty()
            || !args
                .slug
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(FunctionCallError::RespondToModel("submit_roadmap rejected: slug must contain only lowercase ASCII letters, digits, and hyphens".to_string()));
        }
        let phases = parse_phase_rows(&args.roadmap);
        let active: Vec<_> = phases
            .iter()
            .filter(|(_, status)| status == "active")
            .collect();
        let threshold = turn
            .config
            .plan_mode
            .as_ref()
            .and_then(|c| c.split_threshold)
            .unwrap_or(2);
        if threshold == 0 || phases.len() <= threshold || active.len() != 1 || active[0].0 != "1" {
            return Err(FunctionCallError::RespondToModel(format!(
                "submit_roadmap rejected: the roadmap must contain more than {threshold} phase rows, and Phase 1 must be the only row with `active` status."
            )));
        }
        let question = RequestUserInputQuestion {
            id: "confirm_phase_roadmap".to_string(),
            header: "Phase roadmap".to_string(),
            question: format!(
                "This roadmap has {} phases; Phase {} will be the only active design scope. Confirm this split?",
                phases.len(),
                active[0].0
            ),
            is_other: false,
            is_secret: false,
            options: Some(vec![
                RequestUserInputQuestionOption {
                    label: "Confirm roadmap".to_string(),
                    description: "Persist the roadmap and design only the active phase."
                        .to_string(),
                },
                RequestUserInputQuestionOption {
                    label: "Revise roadmap".to_string(),
                    description: "Return to revise phase boundaries or ordering.".to_string(),
                },
            ]),
        };
        let response = session
            .request_user_input(
                turn.as_ref(),
                call_id,
                RequestUserInputArgs {
                    questions: vec![question],
                    auto_resolution_ms: None,
                },
            )
            .await
            .ok_or_else(|| {
                FunctionCallError::RespondToModel(
                    "submit_roadmap was cancelled before confirmation".to_string(),
                )
            })?;
        let confirmed = response
            .answers
            .get("confirm_phase_roadmap")
            .is_some_and(|answer| answer.answers.iter().any(|a| a == "Confirm roadmap"));
        if !confirmed {
            return Err(FunctionCallError::RespondToModel(
                "Roadmap was not confirmed. Revise it and call submit_roadmap again.".to_string(),
            ));
        }
        let date = turn.current_date.as_deref().unwrap_or("0000-00-00");
        let path = turn
            .cwd
            .join(".ody-code")
            .join("roadmaps")
            .join(format!("{date}-{}.md", args.slug));
        tokio::fs::create_dir_all(path.parent().expect("roadmap has parent"))
            .await
            .map_err(|e| {
                FunctionCallError::RespondToModel(format!(
                    "failed to create roadmaps directory: {e}"
                ))
            })?;
        tokio::fs::write(&path, &args.roadmap).await.map_err(|e| {
            FunctionCallError::RespondToModel(format!("failed to persist roadmap: {e}"))
        })?;
        session
            .set_phase_roadmap(PhaseRoadmapState {
                path: path.to_path_buf(),
                active_phase: active[0].0.clone(),
                phase_count: phases.len(),
            })
            .await;
        Ok(boxed_tool_output(FunctionToolOutput::from_text(
            format!(
                "Roadmap confirmed at {}; only Phase {} may now be designed.",
                path.display(),
                active[0].0
            ),
            Some(true),
        )))
    }
}

fn parse_phase_rows(roadmap: &str) -> Vec<(String, String)> {
    roadmap
        .lines()
        .filter_map(|line| {
            let cells: Vec<_> = line
                .trim()
                .trim_matches('|')
                .split('|')
                .map(str::trim)
                .collect();
            (cells.len() >= 2
                && cells[0].chars().all(|c| c.is_ascii_digit())
                && !cells[0].is_empty())
            .then(|| {
                (
                    cells[0].to_string(),
                    cells[cells.len() - 1].to_ascii_lowercase(),
                )
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::parse_phase_rows;

    #[test]
    fn parses_numeric_phase_rows_and_their_statuses() {
        let roadmap = r#"
## Phases
| ID | Goal | Status |
| -- | ---- | ------ |
| 1 | Foundation | active |
| 2 | Migration | pending |
| 3 | Rollout | PENDING |
"#;

        assert_eq!(
            parse_phase_rows(roadmap),
            vec![
                ("1".to_string(), "active".to_string()),
                ("2".to_string(), "pending".to_string()),
                ("3".to_string(), "pending".to_string()),
            ]
        );
    }
}
