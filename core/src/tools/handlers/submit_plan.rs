use crate::function_tool::FunctionCallError;
use crate::plan_mode_injector::parts_manifest::RowStatus;
use crate::plan_mode_injector::parts_manifest::update_task_status;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::submit_artifact::PLAN_WORDING;
use crate::tools::handlers::submit_artifact::handle_submit_artifact;
use crate::tools::handlers::submit_plan_spec::SUBMIT_PLAN_TOOL_NAME;
use crate::tools::handlers::submit_plan_spec::create_submit_plan_tool;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use ody_protocol::config_types::ModeKind;
use ody_protocol::submit_plan_tool::SubmitPlanArgs;
use ody_tools::ToolName;
use ody_tools::ToolSpec;

#[derive(Debug)]
pub struct SubmitPlanHandler;

impl ToolExecutor<ToolInvocation> for SubmitPlanHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(SUBMIT_PLAN_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_submit_plan_tool()
    }

    fn handle(&self, invocation: ToolInvocation) -> ody_tools::ToolExecutorFuture<'_> {
        Box::pin(self.handle_call(invocation))
    }
}

impl CoreToolRuntime for SubmitPlanHandler {}

impl SubmitPlanHandler {
    async fn handle_call(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        let arguments = match &invocation.payload {
            ToolPayload::Function { arguments } => arguments.clone(),
            _ => {
                return Err(FunctionCallError::RespondToModel(format!(
                    "{SUBMIT_PLAN_TOOL_NAME} handler received unsupported payload"
                )));
            }
        };

        let args: SubmitPlanArgs = parse_arguments(&arguments)?;
        let plan = match (args.plan, args.task_id, args.status) {
            (plan, None, None) => plan,
            (None, Some(task_id), Some(status)) => {
                let status = match status.as_str() {
                    "done" => RowStatus::Done,
                    other => {
                        return Err(FunctionCallError::RespondToModel(format!(
                            "submit_plan rejected: unsupported task status `{other}`; use `done`"
                        )));
                    }
                };
                let artifact = invocation.turn.plan_artifact.as_ref().ok_or_else(|| {
                    FunctionCallError::RespondToModel(
                        "submit_plan rejected: no active plan artifact".to_string(),
                    )
                })?;
                let persisted = artifact.last_plan_text().ok_or_else(|| {
                    FunctionCallError::RespondToModel(
                        "submit_plan rejected: submit the complete index once before using task_id/status updates".to_string(),
                    )
                })?;
                Some(
                    update_task_status(&persisted, &task_id, status).map_err(|error| {
                        FunctionCallError::RespondToModel(format!("submit_plan rejected: {error}"))
                    })?,
                )
            }
            (Some(_), Some(_), _) | (Some(_), _, Some(_)) => {
                return Err(FunctionCallError::RespondToModel(
                    "submit_plan rejected: send either complete `plan` markdown or a `task_id`/`status` update, not both".to_string(),
                ));
            }
            (_, Some(_), None) | (_, None, Some(_)) => {
                return Err(FunctionCallError::RespondToModel(
                    "submit_plan rejected: `task_id` and `status` must be provided together"
                        .to_string(),
                ));
            }
        };
        // Plan mode has no checkpoint/final split: every submit_plan is a
        // finalize-intent call (its own split-parts / rigor gates decide
        // terminality), so always request finalization here.
        handle_submit_artifact(invocation, ModeKind::Plan, &PLAN_WORDING, plan, true).await
    }
}

#[cfg(test)]
mod tests {
    // The helper-function tests (rigor_structure_gap, split_threshold_gap,
    // count_task_headings) now live in submit_artifact.rs.
    //
    // Thin-shell integration tests that need a real session/turn context are
    // covered by the existing integration test suite in core/tests/.
}
