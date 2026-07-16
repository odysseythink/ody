use crate::function_tool::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::submit_artifact::DESIGN_WORDING;
use crate::tools::handlers::submit_artifact::handle_submit_artifact;
use crate::tools::handlers::submit_design_spec::SUBMIT_DESIGN_TOOL_NAME;
use crate::tools::handlers::submit_design_spec::create_submit_design_tool;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use ody_protocol::config_types::ModeKind;
use ody_tools::ToolName;
use ody_tools::ToolSpec;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SubmitDesignArgs {
    /// The design index markdown to persist and submit.
    pub design: String,
    /// Whether this submission finalizes the design and exits Design mode.
    /// Defaults to `false`, which checkpoints a partial/skeleton design without
    /// ending the turn. Only `true` runs the C1–C8 completeness gate and can be
    /// terminal.
    #[serde(default, rename = "final")]
    pub is_final: bool,
}

#[derive(Debug)]
pub struct SubmitDesignHandler;

impl ToolExecutor<ToolInvocation> for SubmitDesignHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(SUBMIT_DESIGN_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_submit_design_tool()
    }

    fn handle(&self, invocation: ToolInvocation) -> ody_tools::ToolExecutorFuture<'_> {
        Box::pin(self.handle_call(invocation))
    }
}

impl CoreToolRuntime for SubmitDesignHandler {}

impl SubmitDesignHandler {
    async fn handle_call(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        let arguments = match &invocation.payload {
            ToolPayload::Function { arguments } => arguments.clone(),
            _ => {
                return Err(FunctionCallError::RespondToModel(format!(
                    "{SUBMIT_DESIGN_TOOL_NAME} handler received unsupported payload"
                )));
            }
        };

        let args: SubmitDesignArgs = parse_arguments(&arguments)?;
        handle_submit_artifact(
            invocation,
            ModeKind::Design,
            &DESIGN_WORDING,
            args.design,
            args.is_final,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_tool_name_is_submit_design() {
        let handler = SubmitDesignHandler;
        assert_eq!(
            handler.tool_name().name.as_str(),
            "submit_design"
        );
    }

    #[test]
    fn handler_spec_design_field_exists() {
        let spec = SubmitDesignHandler.spec();
        match spec {
            ToolSpec::Function(tool) => {
                assert_eq!(tool.name, "submit_design");
                let props = tool.parameters.properties.as_ref().expect("must have properties");
                assert!(
                    props.contains_key("design"),
                    "spec must have 'design' property field"
                );
                assert!(
                    !props.contains_key("plan"),
                    "spec must NOT have 'plan' property field"
                );
            }
            _ => panic!("expected Function variant"),
        }
    }

    #[test]
    fn submit_design_args_deserializes_design_field() {
        let json = r#"{"design": "hello world"}"#;
        let args: SubmitDesignArgs = serde_json::from_str(json).expect("valid JSON");
        assert_eq!(args.design, "hello world");
    }

    #[test]
    fn submit_design_args_rejects_unknown_fields() {
        let json = r#"{"design": "x", "plan": "y"}"#;
        let err = serde_json::from_str::<SubmitDesignArgs>(json).unwrap_err();
        assert!(
            err.to_string().contains("unknown field"),
            "SubmitDesignArgs must deny_unknown_fields: {err}"
        );
    }

    #[test]
    fn submit_design_args_rejects_missing_design_field() {
        let json = r#"{}"#;
        let err = serde_json::from_str::<SubmitDesignArgs>(json).unwrap_err();
        assert!(
            err.to_string().contains("design"),
            "missing 'design' field must produce an error mentioning it: {err}"
        );
    }

    #[test]
    fn submit_design_args_final_defaults_false() {
        // Omitting `final` must checkpoint (non-terminal), not finalize.
        let args: SubmitDesignArgs =
            serde_json::from_str(r#"{"design": "x"}"#).expect("valid JSON");
        assert!(!args.is_final, "final must default to false (checkpoint)");
    }

    #[test]
    fn submit_design_args_parses_final_true() {
        let args: SubmitDesignArgs =
            serde_json::from_str(r#"{"design": "x", "final": true}"#).expect("valid JSON");
        assert!(args.is_final, "final=true must deserialize to is_final=true");
    }

    #[test]
    fn submit_design_spec_exposes_optional_final_flag() {
        let spec = SubmitDesignHandler.spec();
        match spec {
            ToolSpec::Function(tool) => {
                let props = tool.parameters.properties.as_ref().expect("must have properties");
                assert!(props.contains_key("final"), "spec must expose the 'final' flag");
                let required = tool.parameters.required.clone().unwrap_or_default();
                assert!(
                    !required.iter().any(|r| r == "final"),
                    "'final' must be optional (defaults to false)"
                );
            }
            _ => panic!("expected Function variant"),
        }
    }
}
