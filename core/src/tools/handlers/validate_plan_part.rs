use crate::function_tool::FunctionCallError;
use crate::plan_mode_injector::parts_manifest::parse_parts_manifest;
use crate::plan_mode_injector::parts_manifest::task_part_preflight_violations;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::validate_plan_part_spec::VALIDATE_PLAN_PART_TOOL_NAME;
use crate::tools::handlers::validate_plan_part_spec::create_validate_plan_part_tool;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use ody_tools::ToolName;
use ody_tools::ToolSpec;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ValidatePlanPartArgs {
    task_id: String,
}

#[derive(Debug)]
pub struct ValidatePlanPartHandler;

impl ToolExecutor<ToolInvocation> for ValidatePlanPartHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(VALIDATE_PLAN_PART_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_validate_plan_part_tool()
    }

    fn handle(&self, invocation: ToolInvocation) -> ody_tools::ToolExecutorFuture<'_> {
        Box::pin(async move {
            let arguments = match &invocation.payload {
                ToolPayload::Function { arguments } => arguments,
                _ => {
                    return Err(FunctionCallError::RespondToModel(
                        "validate_plan_part received unsupported payload".to_string(),
                    ));
                }
            };
            let args: ValidatePlanPartArgs = parse_arguments(arguments)?;
            let artifact = invocation.turn.plan_artifact.as_ref().ok_or_else(|| {
                FunctionCallError::RespondToModel(
                    "validate_plan_part requires an active Plan-mode artifact".to_string(),
                )
            })?;
            let markdown = artifact.last_plan_text().ok_or_else(|| {
                FunctionCallError::RespondToModel(
                    "validate_plan_part requires a persisted split-plan index".to_string(),
                )
            })?;
            let parsed = parse_parts_manifest(&markdown);
            if !parsed.diagnostics.is_empty() {
                return Err(FunctionCallError::RespondToModel(format!(
                    "validate_plan_part could not read the frozen manifest: {}",
                    parsed.diagnostics.join("; ")
                )));
            }
            let manifest = parsed.manifest.ok_or_else(|| {
                FunctionCallError::RespondToModel(
                    "validate_plan_part requires a task-mode `## Parts` manifest".to_string(),
                )
            })?;
            let stem_dir = artifact.stem_dir().ok_or_else(|| {
                FunctionCallError::RespondToModel(
                    "validate_plan_part could not resolve the plan part directory".to_string(),
                )
            })?;
            let max_part_bytes = invocation
                .turn
                .config
                .plan_mode
                .as_ref()
                .and_then(|config| config.max_part_bytes)
                .unwrap_or(0);
            let violations = task_part_preflight_violations(
                &stem_dir,
                &manifest,
                &args.task_id,
                1,
                max_part_bytes,
            )
            .ok_or_else(|| {
                FunctionCallError::RespondToModel(format!(
                    "validate_plan_part could not find task `{}` in a task-mode manifest",
                    args.task_id
                ))
            })?;
            let output = if violations.is_empty() {
                format!(
                    "validation passed for task `{}`; call submit_plan with {{\"task_id\":\"{}\",\"status\":\"done\"}}",
                    args.task_id, args.task_id
                )
            } else {
                format!(
                    "validation failed for task `{}`: {}. Repair the same part and call validate_plan_part again; the frozen index was not changed.",
                    args.task_id,
                    violations.join("; ")
                )
            };
            Ok(boxed_tool_output(FunctionToolOutput::from_text(
                output,
                Some(violations.is_empty()),
            )))
        })
    }
}

impl CoreToolRuntime for ValidatePlanPartHandler {}
