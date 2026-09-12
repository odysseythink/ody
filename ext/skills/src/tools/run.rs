use ody_core_skills::SkillType;
use ody_extension_api::FlowRunError;
use ody_extension_api::FlowRunner;
use ody_extension_api::FunctionCallError;
use ody_extension_api::ToolCall;
use ody_extension_api::ToolExecutor;
use ody_extension_api::ToolExecutorFuture;
use ody_extension_api::ToolName;
use ody_extension_api::ToolSpec;
use ody_protocol::config_types::ModeKind;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;

use super::MAX_HANDLE_BYTES;
use super::SkillToolAuthority;
use super::SkillToolContext;
use super::external_json_output;
use super::parse_args;
use super::skill_function_tool;
use super::skill_tool_name;
use super::validate_handle;

const TOOL_NAME: &str = "flow__run";

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RunArgs {
    name: String,
    #[serde(default)]
    args: serde_json::Map<String, serde_json::Value>,
}

/// Tool-facing response shape. Duplicates [`FlowRunOutput`] so this crate's
/// schemars 0.8 derive stays local (extension-api intentionally does not
/// depend on schemars).
#[derive(Debug, Eq, JsonSchema, PartialEq, Serialize)]
#[schemars(deny_unknown_fields)]
struct RunResponse {
    outputs: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone)]
pub(super) struct RunTool {
    pub(super) context: SkillToolContext,
}

impl ToolExecutor<ToolCall> for RunTool {
    fn tool_name(&self) -> ToolName {
        skill_tool_name(TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        skill_function_tool::<RunArgs, RunResponse>(
            TOOL_NAME,
            "Run one enabled flow skill (host authority) through the host flow runtime. Flow skills are YAML-defined multi-agent plans listed by skills.list; they are not readable via skills.read. Requires a one-shot guardian approval showing the plan summary unless approval_policy is 'never'. Returns the flow's named outputs as JSON.",
        )
    }

    fn handle(&self, call: ToolCall) -> ToolExecutorFuture<'_> {
        Box::pin(async move {
            let args: RunArgs = parse_args(&call)?;
            validate_handle("name", &args.name, MAX_HANDLE_BYTES)?;

            let catalog = self
                .context
                .catalog(&call.turn_id, SkillToolAuthority::Host)
                .await;
            let mode = self
                .context
                .thread_state
                .mode()
                .unwrap_or(ModeKind::Default);
            let flow_is_available = catalog.entries.iter().any(|entry| {
                entry.authority.id == "host"
                    && entry.name == args.name
                    && matches!(entry.skill_type, SkillType::Flow)
                    && entry.is_model_invocable(mode)
            });
            if !flow_is_available {
                return Err(FunctionCallError::RespondToModel(format!(
                    "no model-invocable flow skill named '{}' is available from the host authority",
                    args.name
                )));
            }

            let Some(runner) = self.context.flow_runner.clone() else {
                return Err(FunctionCallError::RespondToModel(
                    "flow execution is not available in this host".to_string(),
                ));
            };
            let output = runner
                .run_flow(&call.turn_id, &args.name, args.args)
                .await
                .map_err(|err| match err {
                    FlowRunError::NotFound { name } => FunctionCallError::RespondToModel(format!(
                        "flow skill '{name}' was not found by the host runtime"
                    )),
                    FlowRunError::InactiveTurn { .. } | FlowRunError::SessionUnavailable => {
                        FunctionCallError::RespondToModel(
                            "flow execution is not available for this turn".to_string(),
                        )
                    }
                    FlowRunError::Failed { reason } => {
                        FunctionCallError::RespondToModel(reason)
                    }
                })?;

            external_json_output(&RunResponse {
                outputs: output.outputs,
            })
        })
    }
}
