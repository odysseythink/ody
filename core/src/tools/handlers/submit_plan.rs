use crate::function_tool::FunctionCallError;
use crate::plan_artifact::PlanWriteOutcome;
use crate::plan_mode_injector::parts_manifest::RowStatus;
use crate::plan_mode_injector::parts_manifest::parse_parts_manifest;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::submit_plan_spec::SUBMIT_PLAN_TOOL_NAME;
use crate::tools::handlers::submit_plan_spec::create_submit_plan_tool;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use ody_protocol::config_types::ModeKind;
use ody_protocol::items::PlanItem;
use ody_protocol::items::TurnItem;
use ody_protocol::protocol::EventMsg;
use ody_protocol::protocol::PlanDeltaEvent;
use ody_protocol::protocol::WarningEvent;
use ody_protocol::submit_plan_tool::SubmitPlanArgs;
use ody_tools::ToolName;
use ody_tools::ToolSpec;
use std::path::PathBuf;

const PLAN_SUBMITTED_MESSAGE: &str = "Plan submitted";
const PLAN_PART_SAVED_MESSAGE: &str = "Plan part saved. This plan is split into multiple parts and pending parts remain — stay in Plan mode and keep writing them one per turn; do not treat this call as final.";

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
        let ToolInvocation {
            session,
            turn,
            call_id,
            payload,
            ..
        } = invocation;

        let arguments = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(format!(
                    "{SUBMIT_PLAN_TOOL_NAME} handler received unsupported payload"
                )));
            }
        };

        if turn.collaboration_mode.mode != ModeKind::Plan {
            return Err(FunctionCallError::RespondToModel(
                "submit_plan is only available in Plan mode".to_string(),
            ));
        }

        let Some(artifact) = turn.plan_artifact.as_ref() else {
            return Err(FunctionCallError::RespondToModel(
                "submit_plan unavailable: no plan artifact".to_string(),
            ));
        };

        let args: SubmitPlanArgs = parse_arguments(&arguments)?;

        let item_id = format!("{}-plan", turn.sub_id);
        let plan_file_path = artifact.path().map(PathBuf::from);

        session
            .emit_turn_item_started(
                turn.as_ref(),
                &TurnItem::Plan(PlanItem {
                    id: item_id.clone(),
                    text: String::new(),
                    plan_file_path: plan_file_path.clone(),
                }),
            )
            .await;

        session
            .send_event(
                turn.as_ref(),
                EventMsg::PlanDelta(PlanDeltaEvent {
                    thread_id: session.thread_id.to_string(),
                    turn_id: turn.sub_id.clone(),
                    item_id: item_id.clone(),
                    delta: args.plan.clone(),
                }),
            )
            .await;

        let persist = turn
            .config
            .plan_mode
            .as_ref()
            .and_then(|pm| pm.persist_plan_file)
            .unwrap_or(true);
        let outcome = artifact.write_plan(&args.plan, persist).await;

        if let PlanWriteOutcome::Failed { error } = &outcome {
            session
                .send_event(
                    turn.as_ref(),
                    EventMsg::Warning(WarningEvent {
                        message: format!("Failed to persist plan: {error}"),
                    }),
                )
                .await;
        }

        // A split plan (see `## Parts` table, `plan_mode_injector`) writes its index and
        // each part through this same tool. Only the call that leaves no pending rows
        // is the real terminal submission — an index/part call with pending rows must
        // not end the Plan-mode turn, or the remaining parts are never written (the
        // model gets stranded outside Plan mode after the first `submit_plan` call).
        let has_pending_parts = parse_parts_manifest(&args.plan)
            .manifest
            .is_some_and(|manifest| {
                manifest
                    .rows
                    .iter()
                    .any(|row| row.status == RowStatus::Pending)
            });

        session
            .emit_turn_item_completed(
                turn.as_ref(),
                TurnItem::Plan(PlanItem {
                    id: item_id,
                    text: args.plan,
                    plan_file_path,
                }),
            )
            .await;

        let message = if has_pending_parts {
            PLAN_PART_SAVED_MESSAGE
        } else {
            artifact.mark_submitted();
            PLAN_SUBMITTED_MESSAGE
        };

        Ok(boxed_tool_output(FunctionToolOutput::from_text(
            message.to_string(),
            Some(true),
        )))
    }
}
