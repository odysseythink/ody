use crate::function_tool::FunctionCallError;
use crate::safety::PlanGateDecision;
use crate::safety::SafetyCheck;
use crate::safety::assess_patch_safety;
use crate::safety::plan_mode_gate_for_patch;
use crate::safety::plan_mode_write_denied_message;
use crate::session::turn_context::TurnContext;
use crate::tools::sandboxing::ExecApprovalRequirement;
use ody_apply_patch::ApplyPatchAction;
use ody_apply_patch::ApplyPatchFileChange;
use ody_config::config_toml::PlanEnforcement;
use ody_protocol::config_types::ModeKind;
use ody_protocol::protocol::FileChange;
use ody_protocol::protocol::FileSystemSandboxPolicy;
use ody_utils_path_uri::PathUri;
use std::collections::HashMap;
use std::path::PathBuf;

pub(crate) enum InternalApplyPatchInvocation {
    /// The `apply_patch` call was handled programmatically, without any sort
    /// of sandbox, because the user explicitly approved it. This is the
    /// result to use with the `shell` function call that contained `apply_patch`.
    Output(Result<String, FunctionCallError>),

    /// The `apply_patch` call was approved, either automatically because it
    /// appears that it should be allowed based on the user's sandbox policy
    /// *or* because the user explicitly approved it. The runtime realizes the
    /// patch through the selected environment filesystem.
    DelegateToRuntime(ApplyPatchRuntimeInvocation),
}

#[derive(Debug)]
pub(crate) struct ApplyPatchRuntimeInvocation {
    pub(crate) action: ApplyPatchAction,
    pub(crate) auto_approved: bool,
    pub(crate) exec_approval_requirement: ExecApprovalRequirement,
}

fn resolve_plan_enforcement(config: &crate::config::Config) -> PlanEnforcement {
    config
        .plan_mode
        .as_ref()
        .and_then(|pm| pm.enforcement)
        .unwrap_or_default()
}

/// Converts the plan-mode gate decision into an `InternalApplyPatchInvocation` when the
/// gate wants to short-circuit `apply_patch`. Returns `None` when the patch should proceed
/// to the normal safety assessment.
///
/// This helper is `#[cfg(test)]` because production code in `apply_patch` needs to keep the
/// owned `ApplyPatchAction` available for the Allow/advisory fallthrough path; it therefore
/// calls `plan_mode_gate_for_patch` directly and constructs invocations inline. The helper
/// remains as a pure, testable seam for the conversion logic.
#[cfg(test)]
pub(crate) fn apply_plan_mode_patch_gate(
    mode: &ody_protocol::config_types::CollaborationMode,
    enforcement: PlanEnforcement,
    action: ApplyPatchAction,
    plan_artifact: Option<&crate::plan_artifact::PlanArtifact>,
) -> Option<InternalApplyPatchInvocation> {
    if mode.mode != ModeKind::Plan {
        return None;
    }
    match plan_mode_gate_for_patch(mode, enforcement, &action, plan_artifact) {
        PlanGateDecision::Deny { reason } => Some(InternalApplyPatchInvocation::Output(Err(
            FunctionCallError::RespondToModel(format!("Plan mode: {reason}")),
        ))),
        PlanGateDecision::Ask { reason } => Some(InternalApplyPatchInvocation::DelegateToRuntime(
            ApplyPatchRuntimeInvocation {
                action,
                auto_approved: false,
                exec_approval_requirement: ExecApprovalRequirement::NeedsApproval {
                    reason: Some(reason),
                    proposed_execpolicy_amendment: None,
                },
            },
        )),
        PlanGateDecision::Allow => None,
    }
}

pub(crate) async fn apply_patch(
    turn_context: &TurnContext,
    file_system_sandbox_policy: &FileSystemSandboxPolicy,
    action: ApplyPatchAction,
) -> InternalApplyPatchInvocation {
    if turn_context.collaboration_mode.mode == ModeKind::Plan {
        let enforcement = resolve_plan_enforcement(&turn_context.config);
        match plan_mode_gate_for_patch(
            &turn_context.collaboration_mode,
            enforcement,
            &action,
            turn_context.plan_artifact.as_deref(),
        ) {
            PlanGateDecision::Deny { .. } => {
                let path = action
                    .changes()
                    .keys()
                    .next()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| PathBuf::from("unknown file"));
                let message = plan_mode_write_denied_message(&path);
                return InternalApplyPatchInvocation::Output(Err(
                    FunctionCallError::RespondToModel(message),
                ));
            }
            PlanGateDecision::Ask { reason } => {
                return InternalApplyPatchInvocation::DelegateToRuntime(
                    ApplyPatchRuntimeInvocation {
                        action,
                        auto_approved: false,
                        exec_approval_requirement: ExecApprovalRequirement::NeedsApproval {
                            reason: Some(reason),
                            proposed_execpolicy_amendment: None,
                        },
                    },
                );
            }
            PlanGateDecision::Allow => {}
        }
    }

    match assess_patch_safety(
        &action,
        turn_context.approval_policy.value(),
        &turn_context.permission_profile(),
        file_system_sandbox_policy,
        &action.cwd,
        turn_context.windows_sandbox_level,
    ) {
        SafetyCheck::AutoApprove {
            user_explicitly_approved,
            ..
        } => InternalApplyPatchInvocation::DelegateToRuntime(ApplyPatchRuntimeInvocation {
            action,
            auto_approved: !user_explicitly_approved,
            exec_approval_requirement: ExecApprovalRequirement::Skip {
                bypass_sandbox: false,
                proposed_execpolicy_amendment: None,
            },
        }),
        SafetyCheck::AskUser => {
            // Delegate the approval prompt (including cached approvals) to the
            // tool runtime, consistent with how shell/unified_exec approvals
            // are orchestrator-driven.
            InternalApplyPatchInvocation::DelegateToRuntime(ApplyPatchRuntimeInvocation {
                action,
                auto_approved: false,
                exec_approval_requirement: ExecApprovalRequirement::NeedsApproval {
                    reason: None,
                    proposed_execpolicy_amendment: None,
                },
            })
        }
        SafetyCheck::Reject { reason } => InternalApplyPatchInvocation::Output(Err(
            FunctionCallError::RespondToModel(format!("patch rejected: {reason}")),
        )),
    }
}

pub(crate) fn convert_apply_patch_to_protocol(
    action: &ApplyPatchAction,
) -> HashMap<PathBuf, FileChange> {
    let mut result = HashMap::with_capacity(action.changes().len());
    for (path, change) in action.changes() {
        let protocol_change = match change {
            ApplyPatchFileChange::Add { content, .. } => FileChange::Add {
                content: content.clone(),
            },
            ApplyPatchFileChange::Delete { content } => FileChange::Delete {
                content: content.clone(),
            },
            ApplyPatchFileChange::Update {
                unified_diff,
                move_path,
                new_content: _new_content,
            } => FileChange::Update {
                unified_diff: unified_diff.clone(),
                move_path: move_path.as_ref().map(PathUri::to_path_buf),
            },
        };
        // TODO(anp): Carry PathUri through patch protocol events once app-server and rollout
        // compatibility no longer require path-flavored strings.
        result.insert(path.to_path_buf(), protocol_change);
    }
    result
}

#[cfg(test)]
#[path = "apply_patch_tests.rs"]
mod tests;
