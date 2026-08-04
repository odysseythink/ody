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
    let edits = build_model_selection_edits("work-kimi", "kimi-k2", Some(ReasoningEffort::Medium));
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
    let edits = build_model_selection_edits("work-kimi", "kimi-k2", None::<ReasoningEffort>);
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

#[test]
fn build_design_review_edits_writes_all_nine_keys() {
    let state = DesignReviewEditState {
        enable: true,
        review_model: Some("provider/model".to_string()),
        debate_enable: true,
        rounds: Some(2),
        advocate_model: Some("adv".to_string()),
        skeptic_model: Some("skp".to_string()),
        judge_model: Some("jdg".to_string()),
        contest_critic: true,
        usability_lens: UsabilityLensToml::Ask,
    };
    let edits = build_design_review_edits(&state);
    assert_eq!(edits.len(), 9);
    assert_eq!(edits[0].key_path, "design_review.enable");
    assert_eq!(edits[0].value, serde_json::json!(true));
    assert_eq!(edits[1].key_path, "design_review.review_model");
    assert_eq!(edits[1].value, serde_json::json!("provider/model"));
    assert_eq!(edits[2].key_path, "design_review.debate.enable");
    assert_eq!(edits[3].key_path, "design_review.debate.rounds");
    assert_eq!(edits[3].value, serde_json::json!(2));
    assert_eq!(edits[7].key_path, "design_review.debate.contest_critic");
    assert_eq!(edits[8].key_path, "design_review.debate.usability_lens");
    assert_eq!(edits[8].value, serde_json::json!("ask"));
}

#[test]
fn build_design_review_edits_clears_optional_strings_and_rounds() {
    let state = DesignReviewEditState {
        enable: false,
        review_model: None,
        debate_enable: false,
        rounds: None,
        advocate_model: None,
        skeptic_model: None,
        judge_model: None,
        contest_critic: false,
        usability_lens: UsabilityLensToml::Off,
    };
    let edits = build_design_review_edits(&state);
    assert_eq!(edits.len(), 9);
    assert_eq!(edits[1].value, serde_json::Value::Null);
    assert_eq!(edits[3].value, serde_json::Value::Null);
    assert_eq!(edits[4].value, serde_json::Value::Null);
    assert_eq!(edits[5].value, serde_json::Value::Null);
    assert_eq!(edits[6].value, serde_json::Value::Null);
}

#[test]
fn design_review_edit_state_apply_from_toml_round_trips() {
    use ody_config::config_toml::DesignReviewToml;

    let toml = DesignReviewToml {
        enable: true,
        review_model: Some("m".to_string()),
        debate: None,
    };
    let mut state = DesignReviewEditState::default();
    state.apply_from_toml(&toml);
    assert!(state.enable);
    assert_eq!(state.review_model, Some("m".to_string()));
    assert!(!state.debate_enable);
    assert_eq!(state.rounds, None);
    assert_eq!(state.usability_lens, UsabilityLensToml::Off);
}

#[test]
fn design_review_edit_state_apply_from_toml_resets_debate_when_absent() {
    use ody_config::config_toml::DesignReviewToml;

    let mut state = DesignReviewEditState {
        debate_enable: true,
        rounds: Some(3),
        advocate_model: Some("a".to_string()),
        skeptic_model: Some("s".to_string()),
        judge_model: Some("j".to_string()),
        contest_critic: true,
        usability_lens: UsabilityLensToml::On,
        ..Default::default()
    };
    state.apply_from_toml(&DesignReviewToml {
        enable: false,
        review_model: None,
        debate: None,
    });
    assert!(!state.debate_enable);
    assert_eq!(state.rounds, None);
    assert_eq!(state.advocate_model, None);
    assert_eq!(state.skeptic_model, None);
    assert_eq!(state.judge_model, None);
    assert!(!state.contest_critic);
    assert_eq!(state.usability_lens, UsabilityLensToml::Off);
}

#[test]
fn design_review_edit_state_apply_from_toml_reads_debate() {
    use ody_config::config_toml::DesignReviewDebateToml;
    use ody_config::config_toml::DesignReviewToml;

    let toml = DesignReviewToml {
        enable: true,
        review_model: None,
        debate: Some(DesignReviewDebateToml {
            enable: true,
            rounds: Some(1),
            advocate_model: Some("a".to_string()),
            skeptic_model: Some("s".to_string()),
            judge_model: Some("j".to_string()),
            contest_critic: true,
            usability_lens: UsabilityLensToml::Ask,
        }),
    };
    let mut state = DesignReviewEditState::default();
    state.apply_from_toml(&toml);
    assert!(state.debate_enable);
    assert_eq!(state.rounds, Some(1));
    assert_eq!(state.advocate_model, Some("a".to_string()));
    assert_eq!(state.skeptic_model, Some("s".to_string()));
    assert_eq!(state.judge_model, Some("j".to_string()));
    assert!(state.contest_critic);
    assert_eq!(state.usability_lens, UsabilityLensToml::Ask);
}

#[tokio::test]
async fn design_review_edit_state_from_config_seeds_from_resolved_fields() {
    use ody_config::config_toml::DesignReviewDebateToml;

    let mut config =
        Config::load_default_with_cli_overrides_for_ody_home(std::env::temp_dir(), Vec::new())
            .await
            .expect("config");
    config.design_review_enabled = true;
    config.design_review_model = Some("resolved/model".to_string());
    config.design_review_debate = Some(DesignReviewDebateToml {
        enable: true,
        rounds: Some(3),
        advocate_model: None,
        skeptic_model: None,
        judge_model: None,
        contest_critic: false,
        usability_lens: UsabilityLensToml::default(),
    });

    let state = DesignReviewEditState::from_config(&config);
    assert!(state.enable);
    assert_eq!(state.review_model, Some("resolved/model".to_string()));
    assert!(state.debate_enable);
    assert_eq!(state.rounds, Some(3));
}
