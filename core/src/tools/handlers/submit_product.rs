use crate::function_tool::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::submit_product_spec::SUBMIT_PRODUCT_TOOL_NAME;
use crate::tools::handlers::submit_product_spec::create_submit_product_tool;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use ody_protocol::config_types::ModeKind;
use ody_protocol::items::PlanItem;
use ody_protocol::items::TurnItem;
use ody_tools::ToolName;
use ody_tools::ToolSpec;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SubmitProductArgs {}

#[derive(Debug)]
pub struct SubmitProductHandler;

impl ToolExecutor<ToolInvocation> for SubmitProductHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(SUBMIT_PRODUCT_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_submit_product_tool()
    }

    fn handle(&self, invocation: ToolInvocation) -> ody_tools::ToolExecutorFuture<'_> {
        Box::pin(self.handle_call(invocation))
    }
}

impl CoreToolRuntime for SubmitProductHandler {}

impl SubmitProductHandler {
    async fn handle_call(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            payload,
            ..
        } = invocation;

        let arguments = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(format!(
                    "{SUBMIT_PRODUCT_TOOL_NAME} handler received unsupported payload"
                )));
            }
        };
        let _args: SubmitProductArgs = parse_arguments(&arguments)?;

        // 1. Mode guard — same contract as submit_plan/submit_design.
        if turn.collaboration_mode.mode != ModeKind::Product {
            return Err(FunctionCallError::RespondToModel(format!(
                "{SUBMIT_PRODUCT_TOOL_NAME} is only available in product mode"
            )));
        }

        // 2. Artifact guard.
        let Some(artifact) = turn.plan_artifact.as_ref() else {
            return Err(FunctionCallError::RespondToModel(format!(
                "{SUBMIT_PRODUCT_TOOL_NAME} unavailable: no product artifact"
            )));
        };

        // 3. The requirements document must already exist on disk: product
        //    mode writes it via `write_file` per the template, so there is no
        //    markdown argument. A restored artifact is Finalized at the real
        //    document path; a Temporary one points at the thread's tmp path.
        let Some(path) = artifact.path() else {
            return Err(FunctionCallError::RespondToModel(format!(
                "{SUBMIT_PRODUCT_TOOL_NAME} rejected: there is no persisted requirements document to submit"
            )));
        };
        let markdown = tokio::fs::read_to_string(&path).await.map_err(|err| {
            FunctionCallError::RespondToModel(format!(
                "{SUBMIT_PRODUCT_TOOL_NAME} rejected: the requirements document at {} could not be read ({err}). Write the document first (`.ody-code/products/<date>-<topic>.md`) and only call this tool after the user confirmed it is complete.",
                path.display()
            ))
        })?;
        if markdown.trim().is_empty() {
            return Err(FunctionCallError::RespondToModel(format!(
                "{SUBMIT_PRODUCT_TOOL_NAME} rejected: the requirements document at {} is empty. Complete the document first, confirm with the user via `request_user_input`, then call this tool again.",
                path.display()
            )));
        }

        // 4. Terminal: mark submitted and emit the finalized plan item. The
        //    TUI turns `finalized: true` into the post-product handoff menu;
        //    the mode switch itself is the client's job (same ownership split
        //    as the post-design next-step menu).
        artifact.mark_submitted();
        let item_id = format!("{}-{}", turn.sub_id, "product");
        session
            .emit_turn_item_completed(
                turn.as_ref(),
                TurnItem::Plan(PlanItem {
                    id: item_id,
                    text: markdown,
                    plan_file_path: Some(path.clone()),
                    finalized: true,
                }),
            )
            .await;

        Ok(crate::tools::context::boxed_tool_output(
            crate::tools::context::FunctionToolOutput::from_text(
                format!(
                    "Requirements document submitted ({}). The client will now offer the handoff menu: Enter Plan mode, Enter Design mode, or stay in Product mode.",
                    path.display()
                ),
                Some(true),
            ),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_tool_name_is_submit_product() {
        let handler = SubmitProductHandler;
        assert_eq!(handler.tool_name().name.as_str(), SUBMIT_PRODUCT_TOOL_NAME);
    }

    #[test]
    fn handler_spec_has_no_properties() {
        match SubmitProductHandler.spec() {
            ToolSpec::Function(tool) => {
                let props = tool.parameters.properties.clone().unwrap_or_default();
                assert!(props.is_empty(), "submit_product must take no arguments");
            }
            _ => panic!("expected Function variant"),
        }
    }

    #[test]
    fn submit_product_args_rejects_unknown_fields() {
        let err = serde_json::from_str::<SubmitProductArgs>(r#"{"plan": "y"}"#).unwrap_err();
        assert!(
            err.to_string().contains("unknown field"),
            "SubmitProductArgs must deny_unknown_fields: {err}"
        );
    }

    #[test]
    fn submit_product_args_accepts_empty_object() {
        let args: SubmitProductArgs =
            serde_json::from_str("{}").expect("empty arguments must parse");
        let _ = args;
    }
}
