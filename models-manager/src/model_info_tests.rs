use super::*;
use crate::ModelsManagerConfig;
use ody_model_provider_info::ModelProviderInfo;
use ody_protocol::model_metadata::InputModality;
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
    expected.capabilities.supports_reasoning_summaries = true;

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
    expected.capabilities.context_window = expected.context_window;

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
fn model_catalog_for_provider_lists_known_chat_vendors() {
    use ody_protocol::model_metadata::ModelVisibility;

    for (provider, expected_slug) in [
        ("kimi", "kimi-k2-0711"),
        ("deepseek", "deepseek-reasoner"),
        ("glm", "glm-4.6"),
    ] {
        let info = ModelProviderInfo {
            wire_api: WireApi::Chat,
            ..Default::default()
        };
        let catalog = model_catalog_for_provider(provider, &info)
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
}

#[test]
fn deepseek_reasoner_supports_thinking() {
    let info = ModelProviderInfo {
        wire_api: WireApi::Chat,
        ..Default::default()
    };
    let catalog = model_catalog_for_provider("deepseek", &info).unwrap();
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

mod capability_tests {
    use super::default_model_capabilities_for_wire_api;
    use super::resolve_model_capabilities;
    use super::ModelCapabilities;
    use super::ProviderCapabilities;
    use super::WireApi;
    use ody_protocol::model_metadata::InputModality;
    use ody_protocol::model_metadata::WebSearchToolType;
    use pretty_assertions::assert_eq;

    #[test]
    fn default_model_capabilities_for_wire_api_chat() {
        let caps = default_model_capabilities_for_wire_api(WireApi::Chat);
        assert!(caps.supports_tools);
        assert!(caps.supports_vision);
        assert_eq!(
            caps.input_modalities,
            vec![InputModality::Text, InputModality::Image]
        );
        assert!(!caps.supports_turn_pause);
    }

    #[test]
    fn default_model_capabilities_for_wire_api_responses() {
        let caps = default_model_capabilities_for_wire_api(WireApi::Responses);
        assert!(caps.supports_tools);
        assert!(caps.supports_vision);
        assert!(caps.supports_multiple_system_messages);
        assert_eq!(
            caps.input_modalities,
            vec![InputModality::Text, InputModality::Image]
        );
    }

    #[test]
    fn default_model_capabilities_for_wire_api_anthropic() {
        let caps = default_model_capabilities_for_wire_api(WireApi::AnthropicMessages);
        assert!(caps.supports_tools);
        assert!(caps.supports_vision);
        assert!(caps.supports_turn_pause);
        assert_eq!(
            caps.input_modalities,
            vec![InputModality::Text, InputModality::Image]
        );
    }

    #[test]
    fn model_capabilities_clamped_by_provider_web_search() {
        let provider_caps = ProviderCapabilities {
            web_search: false,
            ..Default::default()
        };
        let model_caps = ModelCapabilities {
            supports_search_tool: true,
            web_search_tool_type: WebSearchToolType::TextAndImage,
            ..Default::default()
        };
        let resolved = resolve_model_capabilities(
            &provider_caps,
            WireApi::Chat,
            None,
            Some(&model_caps),
            "test-model",
        );
        assert!(!resolved.supports_search_tool);
        assert_eq!(resolved.web_search_tool_type, WebSearchToolType::Text);
    }

    #[test]
    fn model_capabilities_clamps_context_window_to_max() {
        let model_caps = ModelCapabilities {
            context_window: Some(300_000),
            max_context_window: Some(200_000),
            ..Default::default()
        };
        let resolved = resolve_model_capabilities(
            &ProviderCapabilities::default(),
            WireApi::Chat,
            None,
            Some(&model_caps),
            "test-model",
        );
        assert_eq!(resolved.context_window, Some(200_000));
    }

    #[test]
    fn model_capabilities_clamps_auto_compact_to_ninety_percent() {
        let model_caps = ModelCapabilities {
            context_window: Some(100_000),
            auto_compact_token_limit: Some(95_000),
            ..Default::default()
        };
        let resolved = resolve_model_capabilities(
            &ProviderCapabilities::default(),
            WireApi::Chat,
            None,
            Some(&model_caps),
            "test-model",
        );
        assert_eq!(resolved.auto_compact_token_limit, Some(90_000));
    }

    #[test]
    fn model_capabilities_configured_takes_precedence() {
        let built_in = ModelCapabilities {
            context_window: Some(100_000),
            ..Default::default()
        };
        let configured = ModelCapabilities {
            context_window: Some(200_000),
            ..Default::default()
        };
        let resolved = resolve_model_capabilities(
            &ProviderCapabilities::default(),
            WireApi::Chat,
            Some(&configured),
            Some(&built_in),
            "test-model",
        );
        assert_eq!(resolved.context_window, Some(200_000));
    }

    #[test]
    fn model_capabilities_falls_back_to_wire_api_inference() {
        let resolved = resolve_model_capabilities(
            &ProviderCapabilities::default(),
            WireApi::AnthropicMessages,
            None,
            None,
            "test-model",
        );
        assert!(resolved.supports_tools);
        assert!(resolved.supports_vision);
        assert!(resolved.supports_turn_pause);
    }
}

#[test]
fn model_catalog_for_custom_chat_returns_fallback() {
    let info = ModelProviderInfo {
        wire_api: WireApi::Chat,
        ..Default::default()
    };
    let catalog = model_catalog_for_provider("custom", &info)
        .expect("custom chat provider should return a fallback catalog");
    assert!(!catalog.models.is_empty());
    assert!(
        catalog
            .models
            .iter()
            .any(|m| m.used_fallback_model_metadata),
        "custom chat fallback model should be marked as fallback"
    );
}


#[test]
fn model_catalog_for_unknown_chat_returns_fallback() {
    let info = ModelProviderInfo {
        wire_api: WireApi::Chat,
        ..Default::default()
    };
    let catalog = model_catalog_for_provider("unknown", &info)
        .expect("unknown chat provider should return a fallback catalog");
    assert!(!catalog.models.is_empty());
    assert!(
        catalog
            .models
            .iter()
            .any(|m| m.used_fallback_model_metadata),
        "unknown chat fallback model should be marked as fallback"
    );
}

#[test]
fn unknown_model_slug_has_conservative_fallback_capabilities() {
    let model = model_info_from_slug("totally-unknown-model");
    assert!(model.used_fallback_model_metadata);
    assert!(model.capabilities.supports_tools);
    assert_eq!(model.capabilities.input_modalities, vec![InputModality::Text]);
    assert!(!model.capabilities.supports_vision);
    assert!(!model.capabilities.supports_search_tool);
}

#[test]
fn unknown_chat_provider_has_fallback_catalog_with_capabilities() {
    let info = ModelProviderInfo {
        name: "Unknown Chat".to_string(),
        wire_api: WireApi::Chat,
        ..Default::default()
    };
    let catalog = model_catalog_for_provider("unknown-chat", &info)
        .expect("Chat provider should always have a fallback catalog");
    assert_eq!(catalog.models.len(), 1);
    let model = &catalog.models[0];
    assert!(model.capabilities.supports_tools);
    assert!(model.capabilities.supports_vision);
    assert_eq!(model.capabilities.input_modalities, vec![InputModality::Text, InputModality::Image]);
    assert!(model.context_window.is_some());
    assert!(model.max_context_window.is_some());
}
