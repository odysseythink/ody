use super::*;
use crate::ModelsManagerConfig;
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

mod capability_tests {
    use super::ModelCapabilities;
    use super::ProviderCapabilities;
    use super::WireApi;
    use super::default_model_capabilities_for_wire_api;
    use super::resolve_model_capabilities;
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

    #[test]
    fn model_capabilities_zero_truncation_limit_is_clamped() {
        use super::DEFAULT_TRUNCATION_POLICY;
        use ody_protocol::model_metadata::TruncationPolicyConfig;

        let zero = ModelCapabilities {
            truncation_policy: TruncationPolicyConfig::bytes(0),
            ..Default::default()
        };
        let resolved = resolve_model_capabilities(
            &ProviderCapabilities::default(),
            WireApi::Chat,
            Some(&zero),
            None,
            "test-model",
        );
        assert_eq!(resolved.truncation_policy, DEFAULT_TRUNCATION_POLICY);

        let explicit = ModelCapabilities {
            truncation_policy: TruncationPolicyConfig::tokens(1_234),
            ..Default::default()
        };
        let resolved = resolve_model_capabilities(
            &ProviderCapabilities::default(),
            WireApi::Chat,
            Some(&explicit),
            None,
            "test-model",
        );
        assert_eq!(
            resolved.truncation_policy,
            TruncationPolicyConfig::tokens(1_234)
        );
    }
}

#[test]
fn unknown_model_slug_has_conservative_fallback_capabilities() {
    let model = model_info_from_slug("totally-unknown-model");
    assert!(model.used_fallback_model_metadata);
    assert!(model.capabilities.supports_tools);
    assert_eq!(
        model.capabilities.input_modalities,
        vec![InputModality::Text, InputModality::Image]
    );
    assert!(model.capabilities.supports_vision);
    assert!(!model.capabilities.supports_search_tool);
}

#[test]
fn unknown_model_slug_has_nonzero_truncation_budget() {
    // Regression: a zero truncation budget used to truncate every tool output
    // down to the bare "…N chars truncated…" marker, hiding all shell output
    // from the model.
    let model = model_info_from_slug("totally-unknown-model");
    assert!(model.used_fallback_model_metadata);
    assert_eq!(model.truncation_policy, DEFAULT_TRUNCATION_POLICY);
    assert_eq!(
        model.capabilities.truncation_policy,
        DEFAULT_TRUNCATION_POLICY
    );
}

#[test]
fn configured_model_catalog_returns_none_without_matching_provider() {
    let entries = vec![ConfiguredModelSpec {
        provider: "other".to_string(),
        model: "m".to_string(),
        ..Default::default()
    }];
    assert!(
        configured_model_catalog_for_provider(
            "kimi_ranweiwei",
            WireApi::Chat,
            &ProviderCapabilities::default(),
            &entries,
        )
        .is_none()
    );
}

#[test]
fn configured_model_catalog_uses_declared_metadata() {
    let entries = vec![ConfiguredModelSpec {
        provider: "kimi_ranweiwei".to_string(),
        model: "kimi-for-coding".to_string(),
        max_context_size: Some(262_144),
        max_output_size: Some(8_192),
        capabilities: vec!["tool_use".to_string(), "image_in".to_string()],
        display_name: Some("Kimi for Coding".to_string()),
    }];
    let catalog = configured_model_catalog_for_provider(
        "kimi_ranweiwei",
        WireApi::Chat,
        &ProviderCapabilities::default(),
        &entries,
    )
    .expect("matching provider should produce a catalog");

    assert_eq!(catalog.models.len(), 1);
    let model = &catalog.models[0];
    assert_eq!(model.slug, "kimi-for-coding");
    assert_eq!(model.display_name, "Kimi for Coding");
    assert!(!model.used_fallback_model_metadata);
    assert_eq!(model.visibility, ModelVisibility::List);
    assert_eq!(model.context_window, Some(262_144));
    assert_eq!(model.max_context_window, Some(262_144));
    assert_eq!(model.capabilities.max_output_tokens, Some(8_192));
    assert!(model.capabilities.supports_tools);
    assert!(model.capabilities.supports_vision);
    assert!(!model.capabilities.supports_thinking);
    assert_eq!(
        model.capabilities.input_modalities,
        vec![InputModality::Text, InputModality::Image]
    );
    assert!(model.truncation_policy.limit > 0);
}

#[test]
fn configured_model_catalog_defaults_capabilities_from_wire_api() {
    let entries = vec![ConfiguredModelSpec {
        provider: "kimi_ranweiwei".to_string(),
        model: "kimi-for-coding".to_string(),
        max_context_size: Some(262_144),
        ..Default::default()
    }];
    let catalog = configured_model_catalog_for_provider(
        "kimi_ranweiwei",
        WireApi::Chat,
        &ProviderCapabilities::default(),
        &entries,
    )
    .expect("matching provider should produce a catalog");

    let model = &catalog.models[0];
    assert_eq!(model.display_name, "kimi-for-coding");
    assert!(model.capabilities.supports_tools);
    assert!(model.capabilities.supports_vision);
    assert_eq!(model.context_window, Some(262_144));
}
