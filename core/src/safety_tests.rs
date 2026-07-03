use super::*;
use ody_config::config_toml::PlanEnforcement;
use ody_protocol::config_types::{CollaborationMode, ModeKind, Settings};
use ody_protocol::models::PermissionProfile;
use ody_protocol::permissions::NetworkSandboxPolicy;
use ody_protocol::protocol::FileSystemAccessMode;
use ody_protocol::protocol::FileSystemPath;
use ody_protocol::protocol::FileSystemSandboxEntry;
use ody_protocol::protocol::FileSystemSpecialPath;
use ody_protocol::protocol::GranularApprovalConfig;
use ody_utils_absolute_path::AbsolutePathBuf;
use ody_utils_path_uri::PathUri;
use core_test_support::PathExt;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

#[test]
fn test_writable_roots_constraint() {
    // Use a temporary directory as our workspace to avoid touching
    // the real current working directory.
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().abs();
    let cwd_uri = PathUri::from_abs_path(&cwd);
    let parent = cwd.parent().unwrap();

    // Helper to build a single‑entry patch that adds a file at `p`.
    let make_add_change = |p: AbsolutePathBuf| {
        ApplyPatchAction::new_add_for_test(&PathUri::from_abs_path(&p), "".to_string())
    };

    let add_inside = make_add_change(cwd.join("inner.txt"));
    let add_outside = make_add_change(parent.join("outside.txt"));

    // Policy limited to the workspace only; exclude system temp roots so
    // only `cwd` is writable by default.
    let workspace_only_file_system_policy = FileSystemSandboxPolicy::workspace_write(
        &[],
        /*exclude_tmpdir_env_var*/ true,
        /*exclude_slash_tmp*/ true,
    );

    assert!(is_write_patch_constrained_to_writable_paths(
        &add_inside,
        &workspace_only_file_system_policy,
        &cwd_uri,
    ));

    assert!(!is_write_patch_constrained_to_writable_paths(
        &add_outside,
        &workspace_only_file_system_policy,
        &cwd_uri,
    ));

    // With the parent dir explicitly added as a writable root, the
    // outside write should be permitted.
    let file_system_policy_with_parent = FileSystemSandboxPolicy::workspace_write(
        std::slice::from_ref(&parent),
        /*exclude_tmpdir_env_var*/ true,
        /*exclude_slash_tmp*/ true,
    );
    assert!(is_write_patch_constrained_to_writable_paths(
        &add_outside,
        &file_system_policy_with_parent,
        &cwd_uri,
    ));
}

#[test]
fn external_sandbox_auto_approves_in_on_request() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().abs();
    let cwd_uri = PathUri::from_abs_path(&cwd);
    let add_inside_path = cwd.join("inner.txt");
    let add_inside = ApplyPatchAction::new_add_for_test(
        &PathUri::from_abs_path(&add_inside_path),
        "".to_string(),
    );

    let permission_profile = PermissionProfile::External {
        network: NetworkSandboxPolicy::Enabled,
    };
    let file_system_sandbox_policy = FileSystemSandboxPolicy::external_sandbox();

    assert_eq!(
        assess_patch_safety(
            &add_inside,
            AskForApproval::OnRequest,
            &permission_profile,
            &file_system_sandbox_policy,
            &cwd_uri,
            WindowsSandboxLevel::Disabled
        ),
        SafetyCheck::AutoApprove {
            sandbox_type: SandboxType::None,
            user_explicitly_approved: false,
        }
    );
}

#[test]
fn granular_with_all_flags_true_matches_on_request_for_out_of_root_patch() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().abs();
    let cwd_uri = PathUri::from_abs_path(&cwd);
    let parent = cwd.parent().unwrap();
    let outside_path = parent.join("outside.txt");
    let add_outside =
        ApplyPatchAction::new_add_for_test(&PathUri::from_abs_path(&outside_path), "".to_string());
    let permission_profile = PermissionProfile::workspace_write_with(
        &[],
        NetworkSandboxPolicy::Restricted,
        /*exclude_tmpdir_env_var*/ true,
        /*exclude_slash_tmp*/ true,
    );
    let file_system_sandbox_policy = permission_profile.file_system_sandbox_policy();

    assert_eq!(
        assess_patch_safety(
            &add_outside,
            AskForApproval::OnRequest,
            &permission_profile,
            &file_system_sandbox_policy,
            &cwd_uri,
            WindowsSandboxLevel::Disabled,
        ),
        SafetyCheck::AskUser,
    );
    assert_eq!(
        assess_patch_safety(
            &add_outside,
            AskForApproval::Granular(GranularApprovalConfig {
                sandbox_approval: true,
                rules: true,
                skill_approval: true,
                request_permissions: true,
                mcp_elicitations: true,
            }),
            &permission_profile,
            &file_system_sandbox_policy,
            &cwd_uri,
            WindowsSandboxLevel::Disabled,
        ),
        SafetyCheck::AskUser,
    );
}

#[test]
fn granular_sandbox_approval_false_rejects_out_of_root_patch() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().abs();
    let cwd_uri = PathUri::from_abs_path(&cwd);
    let parent = cwd.parent().unwrap();
    let outside_path = parent.join("outside.txt");
    let add_outside =
        ApplyPatchAction::new_add_for_test(&PathUri::from_abs_path(&outside_path), "".to_string());
    let permission_profile = PermissionProfile::workspace_write_with(
        &[],
        NetworkSandboxPolicy::Restricted,
        /*exclude_tmpdir_env_var*/ true,
        /*exclude_slash_tmp*/ true,
    );
    let file_system_sandbox_policy = permission_profile.file_system_sandbox_policy();

    assert_eq!(
        assess_patch_safety(
            &add_outside,
            AskForApproval::Granular(GranularApprovalConfig {
                sandbox_approval: false,
                rules: true,
                skill_approval: true,
                request_permissions: true,
                mcp_elicitations: true,
            }),
            &permission_profile,
            &file_system_sandbox_policy,
            &cwd_uri,
            WindowsSandboxLevel::Disabled,
        ),
        SafetyCheck::Reject {
            reason: PATCH_REJECTED_OUTSIDE_PROJECT_REASON.to_string(),
        },
    );
}

#[test]
fn read_only_policy_rejects_patch_with_read_only_reason() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().abs();
    let cwd_uri = PathUri::from_abs_path(&cwd);
    let inside_path = cwd.join("inside.txt");
    let action =
        ApplyPatchAction::new_add_for_test(&PathUri::from_abs_path(&inside_path), "".to_string());
    let permission_profile = PermissionProfile::read_only();
    let file_system_sandbox_policy = permission_profile.file_system_sandbox_policy();

    assert!(!is_write_patch_constrained_to_writable_paths(
        &action,
        &file_system_sandbox_policy,
        &cwd_uri,
    ));
    assert_eq!(
        assess_patch_safety(
            &action,
            AskForApproval::Never,
            &permission_profile,
            &file_system_sandbox_policy,
            &cwd_uri,
            WindowsSandboxLevel::Disabled,
        ),
        SafetyCheck::Reject {
            reason: PATCH_REJECTED_READ_ONLY_REASON.to_string(),
        },
    );
}
#[test]
fn explicit_unreadable_paths_prevent_auto_approval_for_external_sandbox() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().abs();
    let cwd_uri = PathUri::from_abs_path(&cwd);
    let blocked_path = cwd.join("blocked.txt");
    let blocked_absolute = blocked_path;
    let action = ApplyPatchAction::new_add_for_test(
        &PathUri::from_abs_path(&blocked_absolute),
        "".to_string(),
    );
    let permission_profile = PermissionProfile::External {
        network: NetworkSandboxPolicy::Restricted,
    };
    let file_system_sandbox_policy = FileSystemSandboxPolicy::restricted(vec![
        FileSystemSandboxEntry {
            path: FileSystemPath::Special {
                value: FileSystemSpecialPath::Root,
            },
            access: FileSystemAccessMode::Write,
        },
        FileSystemSandboxEntry {
            path: FileSystemPath::Path {
                path: blocked_absolute,
            },
            access: FileSystemAccessMode::Deny,
        },
    ]);

    assert!(!is_write_patch_constrained_to_writable_paths(
        &action,
        &file_system_sandbox_policy,
        &cwd_uri,
    ));
    assert_eq!(
        assess_patch_safety(
            &action,
            AskForApproval::OnRequest,
            &permission_profile,
            &file_system_sandbox_policy,
            &cwd_uri,
            WindowsSandboxLevel::Disabled,
        ),
        SafetyCheck::AskUser,
    );
}

#[test]
fn explicit_read_only_subpaths_prevent_auto_approval_for_external_sandbox() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().abs();
    let cwd_uri = PathUri::from_abs_path(&cwd);
    let blocked_path = cwd.join("docs").join("blocked.txt");
    let blocked_absolute = blocked_path;
    let docs_absolute = AbsolutePathBuf::resolve_path_against_base("docs", &cwd);
    let action = ApplyPatchAction::new_add_for_test(
        &PathUri::from_abs_path(&blocked_absolute),
        "".to_string(),
    );
    let permission_profile = PermissionProfile::External {
        network: NetworkSandboxPolicy::Restricted,
    };
    let file_system_sandbox_policy = FileSystemSandboxPolicy::restricted(vec![
        FileSystemSandboxEntry {
            path: FileSystemPath::Special {
                value: FileSystemSpecialPath::project_roots(/*subpath*/ None),
            },
            access: FileSystemAccessMode::Write,
        },
        FileSystemSandboxEntry {
            path: FileSystemPath::Path {
                path: docs_absolute,
            },
            access: FileSystemAccessMode::Read,
        },
    ]);

    assert!(!is_write_patch_constrained_to_writable_paths(
        &action,
        &file_system_sandbox_policy,
        &cwd_uri,
    ));
    assert_eq!(
        assess_patch_safety(
            &action,
            AskForApproval::OnRequest,
            &permission_profile,
            &file_system_sandbox_policy,
            &cwd_uri,
            WindowsSandboxLevel::Disabled,
        ),
        SafetyCheck::AskUser,
    );
}

#[test]
fn missing_project_dot_ody_config_requires_approval() {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().abs();
    let cwd_uri = PathUri::from_abs_path(&cwd);
    let config_path = cwd.join(".ody").join("config.toml");
    let action =
        ApplyPatchAction::new_add_for_test(&PathUri::from_abs_path(&config_path), "".to_string());
    let permission_profile = PermissionProfile::workspace_write_with(
        &[],
        NetworkSandboxPolicy::Restricted,
        /*exclude_tmpdir_env_var*/ true,
        /*exclude_slash_tmp*/ true,
    );
    let mut file_system_sandbox_policy = permission_profile.file_system_sandbox_policy();
    file_system_sandbox_policy
        .entries
        .push(FileSystemSandboxEntry {
            path: FileSystemPath::Path {
                path: cwd.join(".ody"),
            },
            access: FileSystemAccessMode::Read,
        });

    assert!(!is_write_patch_constrained_to_writable_paths(
        &action,
        &file_system_sandbox_policy,
        &cwd_uri,
    ));
    assert_eq!(
        assess_patch_safety(
            &action,
            AskForApproval::OnRequest,
            &permission_profile,
            &file_system_sandbox_policy,
            &cwd_uri,
            WindowsSandboxLevel::Disabled,
        ),
        SafetyCheck::AskUser,
    );
}

fn plan_mode() -> CollaborationMode {
    CollaborationMode {
        mode: ModeKind::Plan,
        settings: Settings {
            model: "test".to_string(),
            reasoning_effort: None,
            developer_instructions: None,
        },
    }
}

fn default_mode() -> CollaborationMode {
    CollaborationMode {
        mode: ModeKind::Default,
        settings: Settings {
            model: "test".to_string(),
            reasoning_effort: None,
            developer_instructions: None,
        },
    }
}

#[test]
fn plan_gate_strict_denies_patch_in_plan_mode() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("file.txt");
    let action = ApplyPatchAction::new_add_for_test(
        &PathUri::from_abs_path(&path.abs()),
        "hello".to_string(),
    );
    let decision = plan_mode_gate_for_patch(&plan_mode(), PlanEnforcement::Strict, &action, None);
    assert!(matches!(decision, PlanGateDecision::Deny { .. }));
}

#[test]
fn plan_gate_ask_forces_approval_in_plan_mode() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("file.txt");
    let action = ApplyPatchAction::new_add_for_test(
        &PathUri::from_abs_path(&path.abs()),
        "hello".to_string(),
    );
    let decision = plan_mode_gate_for_patch(&plan_mode(), PlanEnforcement::Ask, &action, None);
    assert!(matches!(decision, PlanGateDecision::Ask { .. }));
}

#[test]
fn plan_gate_advisory_allows_patch_in_plan_mode() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("file.txt");
    let action = ApplyPatchAction::new_add_for_test(
        &PathUri::from_abs_path(&path.abs()),
        "hello".to_string(),
    );
    let decision = plan_mode_gate_for_patch(&plan_mode(), PlanEnforcement::Advisory, &action, None);
    assert_eq!(decision, PlanGateDecision::Allow);
}

#[test]
fn plan_gate_allows_default_mode_regardless_of_enforcement() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("file.txt");
    let action = ApplyPatchAction::new_add_for_test(
        &PathUri::from_abs_path(&path.abs()),
        "hello".to_string(),
    );
    for enforcement in [PlanEnforcement::Strict, PlanEnforcement::Ask, PlanEnforcement::Advisory] {
        let decision = plan_mode_gate_for_patch(&default_mode(), enforcement, &action, None);
        assert_eq!(decision, PlanGateDecision::Allow, "Default mode should never be gated");
    }
}

fn vec_str(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn plan_gate_exec_read_only_allowed_in_strict() {
    let cases = vec![
        vec_str(&["ls"]),
        vec_str(&["cat", "file.txt"]),
        vec_str(&["grep", "TODO", "src"]),
        vec_str(&["rg", "TODO"]),
        vec_str(&["find", ".", "-name", "x"]),
        vec_str(&["git", "status"]),
        vec_str(&["git", "diff"]),
        vec_str(&["git", "log", "-1"]),
        vec_str(&["sed", "-n", "1,5p", "file.txt"]),
        vec_str(&["bash", "-lc", "git status && grep TODO src"]),
        vec_str(&["bash", "-lc", "ls | wc -l"]),
    ];
    for cmd in cases {
        assert_eq!(
            plan_mode_gate_for_exec(&plan_mode(), PlanEnforcement::Strict, &cmd),
            PlanGateDecision::Allow,
            "expected {cmd:?} to be read-only"
        );
    }
}

#[test]
fn plan_gate_exec_known_write_strict_denies() {
    let cases = vec![
        vec_str(&["rm", "-rf", "/"]),
        vec_str(&["rm", "-f", "x"]),
        vec_str(&["cp", "a", "b"]),
        vec_str(&["mv", "a", "b"]),
        vec_str(&["bash", "-lc", "echo x > file.txt"]),
        vec_str(&["bash", "-lc", "echo x >> file.txt"]),
        vec_str(&["git", "commit", "-m", "x"]),
        vec_str(&["git", "checkout", "main"]),
        vec_str(&["git", "apply", "patch.diff"]),
    ];
    for cmd in cases {
        assert!(
            matches!(
                plan_mode_gate_for_exec(&plan_mode(), PlanEnforcement::Strict, &cmd),
                PlanGateDecision::Deny { .. }
            ),
            "expected {cmd:?} to be denied in strict"
        );
    }
}

#[test]
fn plan_gate_exec_indeterminate_strict_asks() {
    let cases = vec![
        vec_str(&["cargo", "check"]),
        vec_str(&["python", "script.py"]),
        vec_str(&["bash", "-lc", "some-tool --analyze"]),
    ];
    for cmd in cases {
        assert!(
            matches!(
                plan_mode_gate_for_exec(&plan_mode(), PlanEnforcement::Strict, &cmd),
                PlanGateDecision::Ask { .. }
            ),
            "expected {cmd:?} to require approval in strict"
        );
    }
}

#[test]
fn plan_gate_exec_ask_enforcement_asks_for_non_readonly() {
    for cmd in [vec_str(&["cp", "a", "b"]), vec_str(&["cargo", "check"])] {
        assert!(
            matches!(
                plan_mode_gate_for_exec(&plan_mode(), PlanEnforcement::Ask, &cmd),
                PlanGateDecision::Ask { .. }
            ),
            "expected {cmd:?} to require approval in ask enforcement"
        );
    }
}

#[test]
fn plan_gate_exec_advisory_allows_everything() {
    for cmd in [vec_str(&["rm", "-rf", "/"]), vec_str(&["cargo", "check"])] {
        assert_eq!(
            plan_mode_gate_for_exec(&plan_mode(), PlanEnforcement::Advisory, &cmd),
            PlanGateDecision::Allow,
            "advisory should behave like the legacy prompt-only plan mode"
        );
    }
}

#[test]
fn plan_gate_exec_default_mode_zero_regression() {
    for enforcement in [
        PlanEnforcement::Strict,
        PlanEnforcement::Ask,
        PlanEnforcement::Advisory,
    ] {
        for cmd in [vec_str(&["rm", "-rf", "/"]), vec_str(&["ls"])] {
            assert_eq!(
                plan_mode_gate_for_exec(&default_mode(), enforcement, &cmd),
                PlanGateDecision::Allow,
                "Default mode must never gate exec"
            );
        }
    }
}
