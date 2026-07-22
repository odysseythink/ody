use super::*;
use color_eyre::eyre::WrapErr;
use ody_protocol::model_metadata::ReasoningEffort;
use pretty_assertions::assert_eq;
use std::path::Path;

#[test]
fn app_scoped_key_path_quotes_dotted_app_ids() {
    assert_eq!(
        app_scoped_key_path("plugin.linear", "enabled"),
        "apps.\"plugin.linear\".enabled"
    );
}

#[test]
fn trusted_project_edit_targets_project_trust_level() {
    assert_eq!(
        trusted_project_edit(Path::new("/workspace/team.project")),
        ConfigEdit {
            key_path: "projects.\"/workspace/team.project\".trust_level".to_string(),
            value: serde_json::json!("trusted"),
            merge_strategy: MergeStrategy::Replace,
        }
    );
}

#[test]
fn build_model_selection_edits_writes_default_model_and_clears_legacy_model() {
    let edits = build_model_selection_edits(
        "work-kimi",
        "kimi-k2",
        Some(ReasoningEffort::Medium),
    );
    assert_eq!(edits.len(), 3);
    assert_eq!(edits[0].key_path, "model");
    assert_eq!(edits[0].value, serde_json::Value::Null);
    assert_eq!(edits[1].key_path, "default_model");
    assert_eq!(edits[1].value, serde_json::json!("work-kimi/kimi-k2"));
    assert_eq!(edits[2].key_path, "model_reasoning_effort");
    assert_eq!(edits[2].value, serde_json::json!("medium"));
}

#[test]
fn build_model_selection_edits_clears_reasoning_effort_when_none() {
    let edits = build_model_selection_edits(
        "work-kimi",
        "kimi-k2",
        None::<ReasoningEffort>,
    );
    assert_eq!(edits.len(), 3);
    assert_eq!(edits[2].key_path, "model_reasoning_effort");
    assert_eq!(edits[2].value, serde_json::Value::Null);
}

#[test]
fn format_config_error_preserves_server_validation_message() {
    let err = Err::<(), _>(color_eyre::eyre::eyre!(
        "config/batchWrite failed: Invalid configuration: features.fast_mode=true violates \
         managed requirements; allowed set [fast_mode=false]"
    ))
    .wrap_err("config/batchWrite failed in TUI")
    .unwrap_err();

    assert_eq!(
        format_config_error(&err),
        "config/batchWrite failed in TUI: config/batchWrite failed: Invalid configuration: \
         features.fast_mode=true violates managed requirements; allowed set [fast_mode=false]"
    );
}
