use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use ody_apply_patch::ApplyPatchAction;
use ody_apply_patch::ApplyPatchFileChange;
use ody_config::config_toml::PlanEnforcement;
use ody_protocol::config_types::CollaborationMode;
use ody_protocol::config_types::ModeKind;
use ody_protocol::config_types::WindowsSandboxLevel;
use ody_protocol::models::PermissionProfile;
use ody_protocol::parse_command::ParsedCommand;
use crate::plan_artifact::PlanArtifact;
use ody_protocol::permissions::FileSystemSandboxPolicy;
use ody_protocol::protocol::AskForApproval;
use ody_sandboxing::SandboxType;
use ody_sandboxing::get_platform_sandbox;
use ody_shell_command::bash::extract_bash_command;
use ody_utils_path_uri::PathUri;

const PATCH_REJECTED_OUTSIDE_PROJECT_REASON: &str =
    "writing outside of the project; rejected by user approval settings";
const PATCH_REJECTED_READ_ONLY_REASON: &str =
    "writing is blocked by read-only sandbox; rejected by user approval settings";

#[derive(Debug, PartialEq)]
pub enum SafetyCheck {
    AutoApprove {
        sandbox_type: SandboxType,
        user_explicitly_approved: bool,
    },
    AskUser,
    Reject {
        reason: String,
    },
}

#[derive(Debug, PartialEq)]
pub enum PlanGateDecision {
    /// The patch is allowed to proceed to the normal safety assessment.
    Allow,
    /// The patch is blocked in Plan mode; the caller should return this reason to the model.
    Deny { reason: String },
    /// The patch requires explicit user approval even if the policy would auto-approve.
    Ask { reason: String },
}

/// Stable marker appended to Plan-mode rejection messages so that downstream
/// consumers (e.g. the TUI footer) can detect them without parsing prose.
pub const PLAN_MODE_REJECTION_MARKER: &str = "[plan-mode-blocked]";

/// Stable marker appended to Design-mode rejection messages so that downstream
/// consumers can detect them without parsing prose. Kept distinct from the
/// Plan marker so Design-mode denials do not trigger Plan-mode-specific UI.
pub const DESIGN_MODE_REJECTION_MARKER: &str = "[design-mode-blocked]";

const PLAN_MODE_WRITE_DENIED_REASON: &str = "Plan mode is read-only by default. Finish planning and switch to Default mode to apply patches. [plan-mode-blocked]";

const DESIGN_MODE_WRITE_DENIED_REASON: &str = "Design mode is read-only. Finish designing and switch to Plan or Default mode to make changes. [design-mode-blocked]";

/// Returns a human-readable Plan-mode patch-denial message that includes the
/// rejected file path and the stable rejection marker.
pub fn plan_mode_write_denied_message(path: &std::path::Path) -> String {
    format!("{PLAN_MODE_WRITE_DENIED_REASON} (file: {})", path.display())
}

/// Returns a human-readable Design-mode patch-denial message that includes the
/// rejected file path and the stable rejection marker.
pub fn design_mode_write_denied_message(path: &std::path::Path) -> String {
    format!("{DESIGN_MODE_WRITE_DENIED_REASON} (file: {})", path.display())
}

/// Returns true for session modes that are read-only by default and must be
/// gated against patch writes and potentially-mutating exec commands.
pub(crate) const fn is_read_only_session_mode(m: ModeKind) -> bool {
    matches!(m, ModeKind::Plan | ModeKind::Design)
}

/// Plan-mode front gate for `apply_patch`. Runs before `assess_patch_safety` so that
/// `AskForApproval::Never` and future auto-approve paths cannot bypass Plan mode.
///
/// `plan_artifact` is the current session's `PlanArtifact`. When provided, writes to the
/// plan file itself or to `<stem>/*.md` files under the plan file's stem directory are
/// allowed even under `Strict` enforcement.
pub fn plan_mode_gate_for_patch(
    mode: &CollaborationMode,
    enforcement: PlanEnforcement,
    action: &ApplyPatchAction,
    plan_artifact: Option<&PlanArtifact>,
) -> PlanGateDecision {
    if !is_read_only_session_mode(mode.mode) {
        return PlanGateDecision::Allow;
    }
    if action.is_empty() {
        return PlanGateDecision::Allow;
    }

    let all_paths_whitelisted = action
        .changes()
        .keys()
        .all(|path_uri| {
            path_uri
                .to_abs_path()
                .ok()
                .map(|abs_path| {
                    plan_artifact
                        .is_some_and(|artifact| artifact.is_plan_file_path(abs_path.as_path()))
                })
                .unwrap_or(false)
        });

    if all_paths_whitelisted {
        return PlanGateDecision::Allow;
    }

    let denied_reason = match mode.mode {
        ModeKind::Design => DESIGN_MODE_WRITE_DENIED_REASON,
        _ => PLAN_MODE_WRITE_DENIED_REASON,
    };

    match enforcement {
        PlanEnforcement::Strict => PlanGateDecision::Deny {
            reason: denied_reason.to_string(),
        },
        PlanEnforcement::Ask => PlanGateDecision::Ask {
            reason: denied_reason.to_string(),
        },
        PlanEnforcement::Advisory => PlanGateDecision::Allow,
    }
}

const PLAN_MODE_EXEC_DENIED_REASON: &str = "Plan mode is read-only by default. This command may modify files; finish planning and switch to Default mode to run it. [plan-mode-blocked] If you were trying to save your plan: you don't need to — use the submit_plan tool to save your plan.";
const PLAN_MODE_EXEC_ASK_REASON: &str =
    "This command may modify files while in Plan mode. Please confirm before running.";

const DESIGN_MODE_EXEC_DENIED_REASON: &str = "Design mode is read-only. This command may modify files; finish designing and switch to Plan or Default mode to run it. [design-mode-blocked] If you were trying to save your design: use the file-write tool instead of a shell command — writes to the assigned design file path are allowed even in Design mode.";
const DESIGN_MODE_EXEC_ASK_REASON: &str =
    "This command may modify files while in Design mode. Please confirm before running.";

/// Returns a human-readable Plan-mode exec-denial message that includes the
/// rejected command and the stable rejection marker.
pub fn plan_mode_exec_denied_message(command: &str) -> String {
    format!("{PLAN_MODE_EXEC_DENIED_REASON} (command: {command})")
}

/// Returns a human-readable Design-mode exec-denial message that includes the
/// rejected command and the stable rejection marker.
pub fn design_mode_exec_denied_message(command: &str) -> String {
    format!("{DESIGN_MODE_EXEC_DENIED_REASON} (command: {command})")
}

#[derive(Debug, PartialEq)]
enum PlanModeExecClassification {
    ReadOnly,
    KnownWrite,
    Indeterminate,
}

/// Plan-mode front gate for exec commands. Runs before the normal exec approval
/// path so that `AskForApproval::Never` and future auto-approve paths cannot
/// bypass Plan mode.
pub fn plan_mode_gate_for_exec(
    mode: &CollaborationMode,
    enforcement: PlanEnforcement,
    command: &[String],
) -> PlanGateDecision {
    if !is_read_only_session_mode(mode.mode) {
        return PlanGateDecision::Allow;
    }

    let command_for_display = command.join(" ");
    let (denied_message, ask_reason) = match mode.mode {
        ModeKind::Design => (
            design_mode_exec_denied_message(&command_for_display),
            DESIGN_MODE_EXEC_ASK_REASON.to_string(),
        ),
        _ => (
            plan_mode_exec_denied_message(&command_for_display),
            PLAN_MODE_EXEC_ASK_REASON.to_string(),
        ),
    };

    match classify_command_for_plan_mode(command) {
        PlanModeExecClassification::ReadOnly => PlanGateDecision::Allow,
        PlanModeExecClassification::KnownWrite => match enforcement {
            PlanEnforcement::Strict => PlanGateDecision::Deny {
                reason: denied_message,
            },
            PlanEnforcement::Ask => PlanGateDecision::Ask {
                reason: ask_reason,
            },
            PlanEnforcement::Advisory => PlanGateDecision::Allow,
        },
        PlanModeExecClassification::Indeterminate => match enforcement {
            PlanEnforcement::Strict | PlanEnforcement::Ask => PlanGateDecision::Ask {
                reason: ask_reason,
            },
            PlanEnforcement::Advisory => PlanGateDecision::Allow,
        },
    }
}

fn classify_command_for_plan_mode(command: &[String]) -> PlanModeExecClassification {
    // For bash/zsh/sh wrappers, recurse into the parsed script so that
    // `bash -lc "echo x > file"` is caught as a write.
    if let Some((_, script)) = extract_bash_command(command) {
        let inner = ody_shell_command::parse_command::parse_shell_script(script);
        return inner
            .iter()
            .map(classify_parsed_command_for_plan_mode)
            .fold(PlanModeExecClassification::ReadOnly, merge_classification);
    }

    let parsed = ody_shell_command::parse_command::parse_command(command);
    parsed
        .iter()
        .map(classify_parsed_command_for_plan_mode)
        .fold(PlanModeExecClassification::ReadOnly, merge_classification)
}

fn classify_parsed_command_for_plan_mode(cmd: &ParsedCommand) -> PlanModeExecClassification {
    match cmd {
        ParsedCommand::Read { .. }
        | ParsedCommand::ListFiles { .. }
        | ParsedCommand::Search { .. } => PlanModeExecClassification::ReadOnly,
        ParsedCommand::Unknown { cmd } => classify_unknown_command_string(cmd),
    }
}

fn classify_unknown_command_string(cmd: &str) -> PlanModeExecClassification {
    let Some(tokens) = shlex::split(cmd) else {
        return PlanModeExecClassification::Indeterminate;
    };

    // A literal redirection token is a definite write.
    if tokens.iter().any(|t| t == ">" || t == ">>") {
        return PlanModeExecClassification::KnownWrite;
    }

    if ody_shell_command::is_safe_command::is_known_safe_command(&tokens) {
        return PlanModeExecClassification::ReadOnly;
    }

    if ody_shell_command::is_dangerous_command::command_might_be_dangerous(&tokens) {
        return PlanModeExecClassification::KnownWrite;
    }

    if is_known_write_command(&tokens) {
        return PlanModeExecClassification::KnownWrite;
    }

    PlanModeExecClassification::Indeterminate
}

fn executable_base_name(raw: &str) -> Option<String> {
    let name = Path::new(raw)
        .file_name()
        .and_then(|n| n.to_str())?
        .to_ascii_lowercase();

    #[cfg(windows)]
    {
        for suffix in [".exe", ".cmd", ".bat", ".com"] {
            if let Some(stripped) = name.strip_suffix(suffix) {
                return Some(stripped.to_string());
            }
        }
    }

    Some(name)
}

fn is_known_write_command(command: &[String]) -> bool {
    let Some(cmd0) = command.first().map(String::as_str) else {
        return false;
    };
    let key = executable_base_name(cmd0);

    match key.as_deref() {
        Some(
            "cp" | "mv" | "rm" | "rmdir" | "mkdir" | "touch" | "chmod" | "chown" | "ln" | "dd"
            | "tee",
        ) => true,
        Some("git") => {
            const WRITE_SUBCOMMANDS: &[&str] = &[
                "commit",
                "checkout",
                "apply",
                "reset",
                "clean",
                "revert",
                "merge",
                "rebase",
                "cherry-pick",
                "push",
                "pull",
            ];
            let mut skip_next = false;
            for arg in command.iter().skip(1).map(String::as_str) {
                if skip_next {
                    skip_next = false;
                    continue;
                }
                // Skip global options that take a value.
                if matches!(arg, "-C" | "-c" | "--git-dir" | "--work-tree") {
                    skip_next = true;
                    continue;
                }
                if arg.starts_with("--git-dir=")
                    || arg.starts_with("--work-tree=")
                    || arg.starts_with("-C")
                    || arg.starts_with("-c")
                    || arg.starts_with('-')
                {
                    continue;
                }
                return WRITE_SUBCOMMANDS.contains(&arg);
            }
            false
        }
        _ => false,
    }
}

fn merge_classification(
    a: PlanModeExecClassification,
    b: PlanModeExecClassification,
) -> PlanModeExecClassification {
    match (a, b) {
        (PlanModeExecClassification::KnownWrite, _)
        | (_, PlanModeExecClassification::KnownWrite) => PlanModeExecClassification::KnownWrite,
        (PlanModeExecClassification::Indeterminate, _)
        | (_, PlanModeExecClassification::Indeterminate) => {
            PlanModeExecClassification::Indeterminate
        }
        _ => PlanModeExecClassification::ReadOnly,
    }
}

pub fn assess_patch_safety(
    action: &ApplyPatchAction,
    policy: AskForApproval,
    permission_profile: &PermissionProfile,
    file_system_sandbox_policy: &FileSystemSandboxPolicy,
    cwd: &PathUri,
    windows_sandbox_level: WindowsSandboxLevel,
) -> SafetyCheck {
    if action.is_empty() {
        return SafetyCheck::Reject {
            reason: "empty patch".to_string(),
        };
    }

    match policy {
        AskForApproval::OnFailure
        | AskForApproval::Never
        | AskForApproval::OnRequest
        | AskForApproval::Granular(_) => {
            // Continue to see if this can be auto-approved.
        }
        // TODO(ragona): I'm not sure this is actually correct? I believe in this case
        // we want to continue to the writable paths check before asking the user.
        AskForApproval::UnlessTrusted => {
            return SafetyCheck::AskUser;
        }
    }

    let rejects_sandbox_approval = matches!(policy, AskForApproval::Never)
        || matches!(
            policy,
            AskForApproval::Granular(granular_config) if !granular_config.sandbox_approval
        );

    // Even though the patch appears to be constrained to writable paths, it is
    // possible that paths in the patch are hard links to files outside the
    // writable roots, so we should still run `apply_patch` in a sandbox in that case.
    if is_write_patch_constrained_to_writable_paths(action, file_system_sandbox_policy, cwd)
        || matches!(policy, AskForApproval::OnFailure)
    {
        if matches!(
            permission_profile,
            PermissionProfile::Disabled | PermissionProfile::External { .. }
        ) {
            // Disabled and External profiles intentionally do not apply an
            // outer Ody filesystem sandbox.
            SafetyCheck::AutoApprove {
                sandbox_type: SandboxType::None,
                user_explicitly_approved: false,
            }
        } else {
            // Only auto‑approve when we can actually enforce a sandbox. Otherwise
            // fall back to asking the user because the patch may touch arbitrary
            // paths outside the project.
            match get_platform_sandbox(windows_sandbox_level != WindowsSandboxLevel::Disabled) {
                Some(sandbox_type) => SafetyCheck::AutoApprove {
                    sandbox_type,
                    user_explicitly_approved: false,
                },
                None => {
                    if rejects_sandbox_approval {
                        SafetyCheck::Reject {
                            reason: patch_rejection_reason(
                                permission_profile,
                                file_system_sandbox_policy,
                                cwd,
                            )
                            .to_string(),
                        }
                    } else {
                        SafetyCheck::AskUser
                    }
                }
            }
        }
    } else if rejects_sandbox_approval {
        SafetyCheck::Reject {
            reason: patch_rejection_reason(permission_profile, file_system_sandbox_policy, cwd)
                .to_string(),
        }
    } else {
        SafetyCheck::AskUser
    }
}

fn patch_rejection_reason(
    permission_profile: &PermissionProfile,
    file_system_sandbox_policy: &FileSystemSandboxPolicy,
    cwd: &PathUri,
) -> &'static str {
    let has_no_writable_roots = cwd.to_abs_path().is_ok_and(|cwd| {
        file_system_sandbox_policy
            .get_writable_roots_with_cwd(cwd.as_path())
            .is_empty()
    });
    match permission_profile {
        PermissionProfile::Managed { .. }
            if !file_system_sandbox_policy.has_full_disk_write_access()
                && has_no_writable_roots =>
        {
            PATCH_REJECTED_READ_ONLY_REASON
        }
        PermissionProfile::Managed { .. }
        | PermissionProfile::Disabled
        | PermissionProfile::External { .. } => PATCH_REJECTED_OUTSIDE_PROJECT_REASON,
    }
}

fn is_write_patch_constrained_to_writable_paths(
    action: &ApplyPatchAction,
    file_system_sandbox_policy: &FileSystemSandboxPolicy,
    cwd: &PathUri,
) -> bool {
    // A full-disk policy permits every patch target, so no per-path writable-root check can
    // further constrain the result.
    if file_system_sandbox_policy.has_full_disk_write_access() {
        return true;
    }
    // TODO(anp): Make filesystem sandbox policies operate on PathUri.
    let Ok(native_cwd) = cwd.to_abs_path() else {
        return false;
    };
    // Normalize a path by removing `.` and resolving `..` without touching the
    // filesystem (works even if the file does not exist).
    fn normalize(path: &Path) -> Option<PathBuf> {
        let mut out = PathBuf::new();
        for comp in path.components() {
            match comp {
                Component::ParentDir => {
                    out.pop();
                }
                Component::CurDir => { /* skip */ }
                other => out.push(other.as_os_str()),
            }
        }
        Some(out)
    }

    // Determine whether `path` is inside **any** writable root. Both `path`
    // and roots are converted to absolute, normalized forms before the
    // prefix check.
    let is_path_writable = |path: &PathUri| {
        // TODO(anp): Make sandbox policy path checks accept PathUri without host projection.
        let Ok(path) = path.to_abs_path() else {
            return false;
        };
        let abs = path.into_path_buf();
        let abs = match normalize(&abs) {
            Some(v) => v,
            None => return false,
        };

        file_system_sandbox_policy.can_write_path_with_cwd(&abs, &native_cwd)
    };

    for (path, change) in action.changes() {
        match change {
            ApplyPatchFileChange::Add { .. } | ApplyPatchFileChange::Delete { .. } => {
                if !is_path_writable(path) {
                    return false;
                }
            }
            ApplyPatchFileChange::Update { move_path, .. } => {
                if !is_path_writable(path) {
                    return false;
                }
                if let Some(dest) = move_path
                    && !is_path_writable(dest)
                {
                    return false;
                }
            }
        }
    }

    true
}

#[cfg(test)]
#[path = "safety_tests.rs"]
mod tests;
