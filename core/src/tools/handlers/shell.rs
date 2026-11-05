use ody_features::Feature;
use ody_protocol::models::ShellCommandToolCallParams;
use serde_json::Value as JsonValue;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::exec::ExecExpiration;
use crate::exec::ExecParams;
use crate::exec_policy::ExecApprovalRequest;
use crate::function_tool::FunctionCallError;
use crate::session::turn_context::TurnContext;
use crate::shell::ShellType;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::events::ToolEmitter;
use crate::tools::events::ToolEventCtx;
use crate::tools::handlers::apply_granted_turn_permissions;
use crate::tools::handlers::apply_patch::intercept_apply_patch;
use crate::tools::handlers::implicit_granted_permissions;
use crate::tools::handlers::normalize_and_validate_additional_permissions;
use crate::tools::handlers::parse_arguments;
use crate::tools::orchestrator::ToolOrchestrator;
use crate::tools::runtimes::shell::ShellRequest;
use crate::tools::runtimes::shell::ShellRuntime;
use crate::tools::runtimes::shell::ShellRuntimeBackend;
use crate::tools::sandboxing::ToolCtx;
use ody_protocol::models::AdditionalPermissionProfile;
use ody_protocol::protocol::AskForApproval;
use ody_protocol::protocol::ExecCommandSource;
use ody_tools::ToolName;
use ody_utils_path_uri::PathUri;

mod background;
mod shell_command;

pub use shell_command::ShellCommandHandler;
pub(crate) use shell_command::ShellCommandHandlerOptions;

fn shell_command_payload_command(payload: &ToolPayload) -> Option<String> {
    let ToolPayload::Function { arguments } = payload else {
        return None;
    };

    parse_arguments::<ShellCommandToolCallParams>(arguments)
        .ok()
        .map(|params| params.command)
}

struct RunExecLikeArgs {
    tool_name: ToolName,
    exec_params: ExecParams,
    cancellation_token: CancellationToken,
    hook_command: String,
    shell_type: Option<ShellType>,
    additional_permissions: Option<AdditionalPermissionProfile>,
    prefix_rule: Option<Vec<String>>,
    session: Arc<crate::session::session::Session>,
    turn: Arc<TurnContext>,
    tracker: crate::tools::context::SharedTurnDiffTracker,
    call_id: String,
    shell_runtime_backend: ShellRuntimeBackend,
    /// Overrides the `timeout_ms`-derived expiration. Background execution
    /// sets this so an omitted timeout does not fall back to the 10s default.
    expiration_override: Option<ExecExpiration>,
}

/// Everything the two shell execution paths (foreground and background) share:
/// permission preprocessing, the approval-policy guard, and the assembled
/// [`ShellRequest`]. The apply_patch intercept intentionally lives with the
/// callers — patch application is interactive by nature and is skipped for
/// background execution.
struct PreparedShellExec {
    emitter: ToolEmitter,
    tool_ctx: ToolCtx,
    req: ShellRequest,
    turn: Arc<TurnContext>,
    approval_policy: AskForApproval,
    turn_environment: crate::session::turn_context::TurnEnvironment,
    shell_runtime_backend: ShellRuntimeBackend,
}

async fn prepare_shell_exec(args: RunExecLikeArgs) -> Result<PreparedShellExec, FunctionCallError> {
    let RunExecLikeArgs {
        tool_name,
        exec_params,
        cancellation_token,
        hook_command,
        shell_type,
        additional_permissions,
        prefix_rule,
        session,
        turn,
        tracker: _,
        call_id,
        shell_runtime_backend,
        expiration_override,
    } = args;

    let Some(turn_environment) = turn.environments.primary().cloned() else {
        return Err(FunctionCallError::RespondToModel(
            "shell is unavailable in this session".to_string(),
        ));
    };

    let explicit_env_overrides = turn
        .config
        .permissions
        .shell_environment_policy
        .r#set
        .clone();
    let exec_permission_approvals_enabled =
        session.features().enabled(Feature::ExecPermissionApprovals);
    let requested_additional_permissions = additional_permissions.clone();
    let effective_additional_permissions = apply_granted_turn_permissions(
        session.as_ref(),
        &turn_environment.environment_id,
        exec_params.cwd.as_path(),
        exec_params.sandbox_permissions,
        additional_permissions,
    )
    .await;
    let additional_permissions_allowed = exec_permission_approvals_enabled
        || (session.features().enabled(Feature::RequestPermissionsTool)
            && effective_additional_permissions.permissions_preapproved);
    let normalized_additional_permissions = implicit_granted_permissions(
        exec_params.sandbox_permissions,
        requested_additional_permissions.as_ref(),
        &effective_additional_permissions,
    )
    .map_or_else(
        || {
            normalize_and_validate_additional_permissions(
                additional_permissions_allowed,
                turn.approval_policy.value(),
                effective_additional_permissions.sandbox_permissions,
                effective_additional_permissions.additional_permissions,
                effective_additional_permissions.permissions_preapproved,
                &exec_params.cwd,
            )
        },
        |permissions| Ok(Some(permissions)),
    )
    .map_err(FunctionCallError::RespondToModel)?;

    // Approval policy guard for explicit escalation in non-OnRequest modes.
    // Sticky turn permissions have already been approved, so they should
    // continue through the normal exec approval flow for the command.
    if effective_additional_permissions
        .sandbox_permissions
        .requests_sandbox_override()
        && !effective_additional_permissions.permissions_preapproved
        && !matches!(
            turn.approval_policy.value(),
            ody_protocol::protocol::AskForApproval::OnRequest
        )
    {
        let approval_policy = turn.approval_policy.value();
        return Err(FunctionCallError::RespondToModel(format!(
            "approval policy is {approval_policy:?}; reject command — you should not ask for escalated permissions if the approval policy is {approval_policy:?}"
        )));
    }

    let emitter = ToolEmitter::shell(
        exec_params.command.clone(),
        exec_params.cwd.clone(),
        ExecCommandSource::Agent,
    );
    let tool_ctx = ToolCtx {
        session: session.clone(),
        turn: turn.clone(),
        call_id: call_id.clone(),
        tool_name,
    };

    let exec_approval_requirement = session
        .services
        .exec_policy
        .create_exec_approval_requirement_for_command(ExecApprovalRequest {
            command: &exec_params.command,
            approval_policy: turn.approval_policy.value(),
            permission_profile: turn.permission_profile(),
            windows_sandbox_level: turn.windows_sandbox_level,
            sandbox_permissions: if effective_additional_permissions.permissions_preapproved {
                ody_protocol::models::SandboxPermissions::UseDefault
            } else {
                effective_additional_permissions.sandbox_permissions
            },
            prefix_rule,
        })
        .await;

    let req = ShellRequest {
        command: exec_params.command.clone(),
        turn_environment: turn_environment.clone(),
        shell_type,
        hook_command,
        cwd: exec_params.cwd.clone(),
        timeout_ms: exec_params.expiration.timeout_ms(),
        expiration_override,
        cancellation_token,
        env: exec_params.env.clone(),
        explicit_env_overrides,
        network: exec_params.network.clone(),
        sandbox_permissions: effective_additional_permissions.sandbox_permissions,
        additional_permissions: normalized_additional_permissions,
        #[cfg(unix)]
        additional_permissions_preapproved: effective_additional_permissions
            .permissions_preapproved,
        justification: exec_params.justification.clone(),
        exec_approval_requirement,
    };

    let approval_policy = turn.approval_policy.value();
    Ok(PreparedShellExec {
        emitter,
        tool_ctx,
        req,
        turn,
        approval_policy,
        turn_environment,
        shell_runtime_backend,
    })
}

/// Dispatches a prepared-or-not shell execution to the foreground or the
/// background path. `background` comes from the tool call's `background`
/// parameter.
async fn run_shell_exec(
    args: RunExecLikeArgs,
    background: bool,
) -> Result<FunctionToolOutput, FunctionCallError> {
    if !background {
        return run_exec_like(args).await;
    }
    let task_id = format!("bg_{}", uuid::Uuid::new_v4().simple());
    let log_path = args
        .exec_params
        .cwd
        .as_path()
        .to_path_buf()
        .join(".ody-code")
        .join("background")
        .join(format!("{task_id}.log"));
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| {
            FunctionCallError::RespondToModel(format!(
                "failed to create background log directory {}: {err}",
                parent.display()
            ))
        })?;
    }
    let prepared = prepare_shell_exec(args).await?;
    background::run_background_exec(prepared, task_id, log_path).await
}

async fn run_exec_like(args: RunExecLikeArgs) -> Result<FunctionToolOutput, FunctionCallError> {
    let RunExecLikeArgs {
        call_id,
        session,
        turn,
        tracker,
        ..
    } = &args;
    let call_id = call_id.clone();
    let session = Arc::clone(session);
    let turn = Arc::clone(turn);
    let tracker = tracker.clone();

    let prepared = prepare_shell_exec(args).await?;
    let PreparedShellExec {
        emitter,
        tool_ctx,
        req,
        turn,
        approval_policy,
        turn_environment,
        shell_runtime_backend,
    } = prepared;

    let fs = turn_environment.environment.get_filesystem();

    // Intercept apply_patch if present.
    let apply_patch_cwd = PathUri::from_abs_path(&req.cwd);
    if let Some(output) = intercept_apply_patch(
        &req.command,
        &apply_patch_cwd,
        fs.as_ref(),
        turn_environment.clone(),
        tool_ctx.session.clone(),
        tool_ctx.turn.clone(),
        /*tracker*/ Some(&tracker),
        &call_id,
        tool_ctx.tool_name.name.as_str(),
    )
    .await?
    {
        return Ok(output);
    }

    let event_ctx = ToolEventCtx::new(
        tool_ctx.session.as_ref(),
        tool_ctx.turn.as_ref(),
        &call_id,
        /*turn_diff_tracker*/ None,
    );
    emitter.begin(event_ctx).await;

    let mut orchestrator = ToolOrchestrator::new();
    let mut runtime = ShellRuntime::for_shell_command(shell_runtime_backend);
    let out = orchestrator
        .run(&mut runtime, &req, &tool_ctx, &turn, approval_policy)
        .await
        .map(|result| result.output);
    let event_ctx = ToolEventCtx::new(
        tool_ctx.session.as_ref(),
        tool_ctx.turn.as_ref(),
        &call_id,
        /*turn_diff_tracker*/ None,
    );
    let post_tool_use_response = out
        .as_ref()
        .ok()
        .map(|output| {
            crate::tools::format_exec_output_str(output, turn.model_info.truncation_policy.into())
        })
        .map(JsonValue::String);
    let content = emitter
        .finish(event_ctx, out, /*applied_patch_delta*/ None)
        .await?;
    Ok(FunctionToolOutput {
        body: vec![
            ody_protocol::models::FunctionCallOutputContentItem::InputText { text: content },
        ],
        success: Some(true),
        post_tool_use_response,
    })
}

#[cfg(test)]
#[path = "shell_tests.rs"]
mod tests;
