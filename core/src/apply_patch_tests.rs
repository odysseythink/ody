use super::*;
use ody_config::config_toml::PlanEnforcement;
use ody_protocol::config_types::{CollaborationMode, ModeKind, Settings};
use ody_utils_path_uri::PathUri;
use pretty_assertions::assert_eq;

use tempfile::tempdir;

#[test]
fn convert_apply_patch_maps_add_variant() {
    let tmp = tempdir().expect("tmp");
    let path = tmp.path().join("a.txt");
    let path_uri = PathUri::from_host_native_path(&path).expect("absolute test path");
    let action = ApplyPatchAction::new_add_for_test(&path_uri, "hello".to_string());

    let got = convert_apply_patch_to_protocol(&action);

    assert_eq!(
        got.get(path.as_path()),
        Some(&FileChange::Add {
            content: "hello".to_string()
        })
    );
}

fn collaboration_mode(mode: ModeKind) -> CollaborationMode {
    CollaborationMode {
        mode,
        settings: Settings {
            model: "test".to_string(),
            reasoning_effort: None,
            developer_instructions: None,
        },
    }
}

#[test]
fn apply_plan_mode_patch_gate_denies_in_plan_mode_strict() {
    let tmp = tempdir().expect("tmp");
    let path = tmp.path().join("a.txt");
    let path_uri = PathUri::from_host_native_path(&path).expect("absolute test path");
    let action = ApplyPatchAction::new_add_for_test(&path_uri, "hello".to_string());

    let result = apply_plan_mode_patch_gate(
        &collaboration_mode(ModeKind::Plan),
        PlanEnforcement::Strict,
        action,
    );

    assert!(
        matches!(
            result,
            Some(InternalApplyPatchInvocation::Output(Err(FunctionCallError::RespondToModel(_))))
        ),
        "Plan mode strict should reject patch with model-readable error"
    );
}

#[test]
fn apply_plan_mode_patch_gate_asks_in_plan_mode_ask() {
    let tmp = tempdir().expect("tmp");
    let path = tmp.path().join("a.txt");
    let path_uri = PathUri::from_host_native_path(&path).expect("absolute test path");
    let action = ApplyPatchAction::new_add_for_test(&path_uri, "hello".to_string());

    let result = apply_plan_mode_patch_gate(
        &collaboration_mode(ModeKind::Plan),
        PlanEnforcement::Ask,
        action,
    );

    assert!(
        matches!(
            result,
            Some(InternalApplyPatchInvocation::DelegateToRuntime(ApplyPatchRuntimeInvocation {
                auto_approved: false,
                exec_approval_requirement: ExecApprovalRequirement::NeedsApproval { .. },
                ..
            }))
        ),
        "Plan mode ask should force user approval"
    );
}

#[test]
fn apply_plan_mode_patch_gate_allows_in_default_mode() {
    let tmp = tempdir().expect("tmp");
    let path = tmp.path().join("a.txt");
    let path_uri = PathUri::from_host_native_path(&path).expect("absolute test path");
    let action = ApplyPatchAction::new_add_for_test(&path_uri, "hello".to_string());

    let result = apply_plan_mode_patch_gate(
        &collaboration_mode(ModeKind::Default),
        PlanEnforcement::Strict,
        action,
    );

    assert!(
        result.is_none(),
        "Default mode should not be gated"
    );
}
