//! Tests for TUI `/login` config-edit builders.

use ody_app_server_protocol::MergeStrategy;
use ody_config::config_toml::OdyCodeModelConfig;
use ody_model_provider_info::LoginProvider;
use std::collections::HashMap;

use crate::login::config::{
    build_login_model_edits, build_login_models_edits, build_login_provider_edits,
    build_logout_provider_edits,
};

#[test]
fn build_login_provider_edits_writes_type_and_api_key() {
    let edits = build_login_provider_edits("work-kimi", LoginProvider::Kimi, "secret-key", None);
    assert_eq!(edits.len(), 2);
    assert_eq!(edits[0].key_path, "providers.work-kimi.type");
    assert_eq!(edits[0].value, serde_json::json!("kimi"));
    assert_eq!(edits[0].merge_strategy, MergeStrategy::Replace);
    assert_eq!(edits[1].key_path, "providers.work-kimi.api_key");
    assert_eq!(edits[1].value, serde_json::json!("secret-key"));
}

#[test]
fn build_login_provider_edits_includes_base_url_when_given() {
    let edits = build_login_provider_edits(
        "work-kimi",
        LoginProvider::Kimi,
        "secret-key",
        Some("https://example.com/v1"),
    );
    assert_eq!(edits.len(), 3);
    assert_eq!(edits[2].key_path, "providers.work-kimi.base_url");
    assert_eq!(edits[2].value, serde_json::json!("https://example.com/v1"));
}

#[test]
fn build_login_model_edits_writes_model_and_default() {
    let edits =
        build_login_model_edits("work-kimi", LoginProvider::Kimi, "kimi-k2", Some("Kimi K2"));
    assert_eq!(edits.len(), 5);
    assert_eq!(edits[0].key_path, r#"models."work-kimi/kimi-k2".provider"#);
    assert_eq!(edits[0].value, serde_json::json!("work-kimi"));
    assert_eq!(edits[1].key_path, r#"models."work-kimi/kimi-k2".model"#);
    assert_eq!(edits[1].value, serde_json::json!("kimi-k2"));
    assert_eq!(
        edits[2].key_path,
        r#"models."work-kimi/kimi-k2".display_name"#
    );
    assert_eq!(edits[2].value, serde_json::json!("Kimi K2"));
    assert_eq!(edits[3].key_path, "model");
    assert_eq!(edits[3].value, serde_json::Value::Null);
    assert_eq!(edits[4].key_path, "default_model");
    assert_eq!(edits[4].value, serde_json::json!("work-kimi/kimi-k2"));
}

#[test]
fn build_login_model_edits_omits_display_name_when_none() {
    let edits = build_login_model_edits("work-kimi", LoginProvider::Kimi, "kimi-k2", None);
    assert_eq!(edits.len(), 4);
    assert_eq!(edits[2].key_path, "model");
    assert_eq!(edits[2].value, serde_json::Value::Null);
    assert_eq!(edits[3].key_path, "default_model");
    assert_eq!(edits[3].value, serde_json::json!("work-kimi/kimi-k2"));
}

#[test]
fn build_login_models_edits_writes_all_fetched_models_and_default() {
    use ody_model_provider::login::LoginModelInfo;

    let models = vec![
        LoginModelInfo {
            id: "model-a".to_string(),
            display_name: "Model A".to_string(),
        },
        LoginModelInfo {
            id: "model-b".to_string(),
            display_name: "Model B".to_string(),
        },
    ];
    let edits =
        build_login_models_edits("work-kimi", LoginProvider::Kimi, &models, "model-a");

    assert_eq!(edits.len(), 8);
    assert_eq!(edits[0].key_path, r#"models."work-kimi/model-a".provider"#);
    assert_eq!(edits[0].value, serde_json::json!("work-kimi"));
    assert_eq!(edits[1].key_path, r#"models."work-kimi/model-a".model"#);
    assert_eq!(edits[1].value, serde_json::json!("model-a"));
    assert_eq!(edits[2].key_path, r#"models."work-kimi/model-a".display_name"#);
    assert_eq!(edits[2].value, serde_json::json!("Model A"));
    assert_eq!(edits[3].key_path, r#"models."work-kimi/model-b".provider"#);
    assert_eq!(edits[3].value, serde_json::json!("work-kimi"));
    assert_eq!(edits[4].key_path, r#"models."work-kimi/model-b".model"#);
    assert_eq!(edits[4].value, serde_json::json!("model-b"));
    assert_eq!(edits[5].key_path, r#"models."work-kimi/model-b".display_name"#);
    assert_eq!(edits[5].value, serde_json::json!("Model B"));
    assert_eq!(edits[6].key_path, "model");
    assert_eq!(edits[6].value, serde_json::Value::Null);
    assert_eq!(edits[7].key_path, "default_model");
    assert_eq!(edits[7].value, serde_json::json!("work-kimi/model-a"));
}

#[test]
fn build_login_models_edits_skips_display_name_when_same_as_id() {
    use ody_model_provider::login::LoginModelInfo;

    let models = vec![LoginModelInfo {
        id: "model-a".to_string(),
        display_name: "model-a".to_string(),
    }];
    let edits =
        build_login_models_edits("work-kimi", LoginProvider::Kimi, &models, "model-a");

    assert_eq!(edits.len(), 4);
    assert_eq!(edits[0].key_path, r#"models."work-kimi/model-a".provider"#);
    assert_eq!(edits[1].key_path, r#"models."work-kimi/model-a".model"#);
    assert_eq!(edits[2].key_path, "model");
    assert_eq!(edits[2].value, serde_json::Value::Null);
    assert_eq!(edits[3].key_path, "default_model");
}

#[test]
fn build_logout_provider_edits_clears_provider_table() {
    let aliases = vec!["work-kimi".to_string()];
    let edits = build_logout_provider_edits(&aliases, &std::collections::HashMap::new(), None);
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].key_path, "providers.work-kimi");
    assert_eq!(edits[0].value, serde_json::Value::Null);
    assert_eq!(edits[0].merge_strategy, MergeStrategy::Replace);
}

#[test]
fn build_logout_provider_edits_clears_default_model_when_owned_by_provider() {
    let aliases = vec!["work-kimi".to_string()];
    let edits = build_logout_provider_edits(
        &aliases,
        &std::collections::HashMap::new(),
        Some("work-kimi/kimi-k2"),
    );
    assert_eq!(edits.len(), 2);
    assert_eq!(edits[1].key_path, "default_model");
    assert_eq!(edits[1].value, serde_json::Value::Null);
}

#[test]
fn build_logout_provider_edits_keeps_default_model_when_not_owned_by_provider() {
    let aliases = vec!["work-kimi".to_string()];
    let edits = build_logout_provider_edits(
        &aliases,
        &std::collections::HashMap::new(),
        Some("other/kimi-k2.5"),
    );
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].key_path, "providers.work-kimi");
}

#[test]
fn build_logout_provider_edits_returns_empty_when_no_aliases() {
    let edits: Vec<ody_app_server_protocol::ConfigEdit> =
        build_logout_provider_edits(&[], &std::collections::HashMap::new(), None);
    assert!(edits.is_empty());
}

#[test]
fn build_logout_provider_edits_clears_matching_models() {
    let aliases = vec!["work-kimi".to_string()];
    let mut models = HashMap::new();
    models.insert(
        "work-kimi/kimi-k2".to_string(),
        OdyCodeModelConfig::default(),
    );
    models.insert("other/kimi-k2.5".to_string(), OdyCodeModelConfig::default());
    let edits = build_logout_provider_edits(&aliases, &models, None);
    assert_eq!(edits.len(), 2);
    assert_eq!(edits[0].key_path, "providers.work-kimi");
    assert_eq!(edits[1].key_path, r#"models."work-kimi/kimi-k2""#);
    assert_eq!(edits[1].value, serde_json::Value::Null);
}

#[test]
fn build_logout_provider_edits_clears_default_model_and_matching_models() {
    let aliases = vec!["work-kimi".to_string()];
    let mut models = HashMap::new();
    models.insert(
        "work-kimi/kimi-k2".to_string(),
        OdyCodeModelConfig::default(),
    );
    let edits = build_logout_provider_edits(&aliases, &models, Some("work-kimi/kimi-k2"));
    assert_eq!(edits.len(), 3);
    assert_eq!(edits[0].key_path, "providers.work-kimi");
    assert_eq!(edits[1].key_path, r#"models."work-kimi/kimi-k2""#);
    assert_eq!(edits[2].key_path, "default_model");
}

#[test]
fn build_login_model_edits_writes_model_metadata_from_bundled_catalog() {
    // deepseek-v4-pro is present in models.json and has known metadata.
    let edits = build_login_model_edits(
        "work-deepseek",
        LoginProvider::Deepseek,
        "deepseek-v4-pro",
        None,
    );

    // provider, model, max_context_size, max_output_size, capabilities, clear model, default_model
    assert_eq!(edits.len(), 7);
    assert_eq!(edits[0].key_path, r#"models."work-deepseek/deepseek-v4-pro".provider"#);
    assert_eq!(edits[1].key_path, r#"models."work-deepseek/deepseek-v4-pro".model"#);
    assert_eq!(edits[2].key_path, r#"models."work-deepseek/deepseek-v4-pro".max_context_size"#);
    assert_eq!(edits[2].value, serde_json::json!(1_000_000));
    assert_eq!(edits[3].key_path, r#"models."work-deepseek/deepseek-v4-pro".max_output_size"#);
    assert_eq!(edits[3].value, serde_json::json!(384_000));
    assert_eq!(edits[4].key_path, r#"models."work-deepseek/deepseek-v4-pro".capabilities"#);
    assert_eq!(edits[4].value, serde_json::json!(["tool_use", "thinking"]));
    assert_eq!(edits[5].key_path, "model");
    assert_eq!(edits[6].key_path, "default_model");
    assert_eq!(edits[6].value, serde_json::json!("work-deepseek/deepseek-v4-pro"));
}
