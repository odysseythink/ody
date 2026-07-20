//! Tests for TUI `/login` config-edit builders.

use ody_app_server_protocol::MergeStrategy;
use ody_config::config_toml::OdyCodeModelConfig;
use ody_model_provider_info::LoginProvider;
use std::collections::HashMap;

use crate::login::config::{
    build_login_model_edits, build_login_provider_edits, build_logout_provider_edits,
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
    assert_eq!(edits.len(), 4);
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
    assert_eq!(edits[3].value, serde_json::json!("work-kimi/kimi-k2"));
}

#[test]
fn build_login_model_edits_omits_display_name_when_none() {
    let edits = build_login_model_edits("work-kimi", LoginProvider::Kimi, "kimi-k2", None);
    assert_eq!(edits.len(), 3);
    assert_eq!(edits[2].key_path, "model");
    assert_eq!(edits[2].value, serde_json::json!("work-kimi/kimi-k2"));
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
    assert_eq!(edits[1].key_path, "model");
    assert_eq!(edits[1].value, serde_json::Value::Null);
}

#[test]
fn build_logout_provider_edits_keeps_default_model_when_not_owned_by_provider() {
    let aliases = vec!["work-kimi".to_string()];
    let edits = build_logout_provider_edits(
        &aliases,
        &std::collections::HashMap::new(),
        Some("other/gpt-5"),
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
    models.insert("other/gpt-5".to_string(), OdyCodeModelConfig::default());
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
    assert_eq!(edits[2].key_path, "model");
}
