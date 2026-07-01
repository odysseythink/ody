use super::*;
use crate::ModelsManagerConfig;
use pretty_assertions::assert_eq;

#[test]
fn reasoning_summaries_override_true_enables_support() {
    let model = model_info_from_slug("unknown-model");
    let config = ModelsManagerConfig {
        model_supports_reasoning_summaries: Some(true),
        ..Default::default()
    };

    let updated = with_config_overrides(model.clone(), &config);
    let mut expected = model;
    expected.supports_reasoning_summaries = true;

    assert_eq!(updated, expected);
}

#[test]
fn reasoning_summaries_override_false_does_not_disable_support() {
    let mut model = model_info_from_slug("unknown-model");
    model.supports_reasoning_summaries = true;
    let config = ModelsManagerConfig {
        model_supports_reasoning_summaries: Some(false),
        ..Default::default()
    };

    let updated = with_config_overrides(model.clone(), &config);

    assert_eq!(updated, model);
}

#[test]
fn reasoning_summaries_override_false_is_noop_when_model_is_false() {
    let model = model_info_from_slug("unknown-model");
    let config = ModelsManagerConfig {
        model_supports_reasoning_summaries: Some(false),
        ..Default::default()
    };

    let updated = with_config_overrides(model.clone(), &config);

    assert_eq!(updated, model);
}

#[test]
fn model_context_window_override_clamps_to_max_context_window() {
    let mut model = model_info_from_slug("unknown-model");
    model.context_window = Some(273_000);
    model.max_context_window = Some(400_000);
    let config = ModelsManagerConfig {
        model_context_window: Some(500_000),
        ..Default::default()
    };

    let updated = with_config_overrides(model.clone(), &config);
    let mut expected = model;
    expected.context_window = Some(400_000);

    assert_eq!(updated, expected);
}

#[test]
fn model_context_window_uses_model_value_without_override() {
    let mut model = model_info_from_slug("unknown-model");
    model.context_window = Some(273_000);
    model.max_context_window = Some(400_000);
    let config = ModelsManagerConfig::default();

    let updated = with_config_overrides(model.clone(), &config);

    assert_eq!(updated, model);
}

#[test]
fn chat_provider_models_lists_known_vendors() {
    use ody_protocol::odysseythink_models::ModelVisibility;

    for (provider, expected_slug) in [
        ("kimi", "kimi-k2-0711"),
        ("deepseek", "deepseek-reasoner"),
        ("glm", "glm-4.6"),
    ] {
        let catalog = chat_provider_models(provider)
            .unwrap_or_else(|| panic!("missing catalog for {provider}"));
        assert!(
            catalog.models.iter().any(|m| m.slug == expected_slug),
            "expected {expected_slug} in {provider} catalog"
        );
        // Curated models should be visible in the picker.
        assert!(
            catalog
                .models
                .iter()
                .all(|m| m.visibility == ModelVisibility::List && !m.used_fallback_model_metadata)
        );
    }
    assert!(chat_provider_models("unknown").is_none());
}

#[test]
fn deepseek_reasoner_supports_thinking() {
    let catalog = chat_provider_models("deepseek").unwrap();
    let reasoner = catalog
        .models
        .iter()
        .find(|m| m.slug == "deepseek-reasoner")
        .unwrap();
    assert!(reasoner.supports_reasoning_summaries);
    let chat = catalog
        .models
        .iter()
        .find(|m| m.slug == "deepseek-chat")
        .unwrap();
    assert!(!chat.supports_reasoning_summaries);
}
