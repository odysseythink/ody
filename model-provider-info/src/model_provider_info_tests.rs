use super::*;
use ody_utils_absolute_path::AbsolutePathBuf;
use ody_utils_absolute_path::AbsolutePathBufGuard;
use pretty_assertions::assert_eq;
use std::collections::HashMap;
use std::num::NonZeroU64;
use tempfile::tempdir;

#[test]
fn test_deserialize_example_model_provider_toml() {
    let azure_provider_toml = r#"
name = "Example"
base_url = "https://example.com"
env_key = "API_KEY"
http_headers = { "X-Example-Header" = "example-value" }
env_http_headers = { "X-Example-Env-Header" = "EXAMPLE_ENV_VAR" }
        "#;
    let expected_provider = ModelProviderInfo {
        name: "Example".into(),
        base_url: Some("https://example.com".into()),
        env_key: Some("API_KEY".into()),
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        wire_api: WireApi::Responses,
        query_params: None,
        http_headers: Some(maplit::hashmap! {
            "X-Example-Header".to_string() => "example-value".to_string(),
        }),
        env_http_headers: Some(maplit::hashmap! {
            "X-Example-Env-Header".to_string() => "EXAMPLE_ENV_VAR".to_string(),
        }),
        request_max_retries: None,
        stream_max_retries: None,
        stream_idle_timeout_ms: None,
        websocket_connect_timeout_ms: None,
        supports_websockets: false,
        capabilities: ProviderCapabilities::default(),
    };

    let provider: ModelProviderInfo = toml::from_str(azure_provider_toml).unwrap();
    assert_eq!(expected_provider, provider);
}

#[test]
fn test_deserialize_chat_wire_api() {
    let provider_toml = r#"
name = "Kimi"
base_url = "https://api.moonshot.ai/v1"
env_key = "KIMI_API_KEY"
wire_api = "chat"
        "#;

    let provider = toml::from_str::<ModelProviderInfo>(provider_toml).unwrap();
    assert_eq!(provider.wire_api, WireApi::Chat);
}

#[test]
fn built_in_chat_providers_are_registered() {
    let providers = built_in_model_providers();
    for (id, env_key) in [
        ("kimi", "KIMI_API_KEY"),
        ("deepseek", "DEEPSEEK_API_KEY"),
        ("glm", "GLM_API_KEY"),
    ] {
        let provider = providers
            .get(id)
            .unwrap_or_else(|| panic!("missing built-in provider {id}"));
        assert_eq!(provider.wire_api, WireApi::Chat);
        assert_eq!(provider.env_key.as_deref(), Some(env_key));
        assert!(provider.base_url.is_some());
    }
    assert!(providers["kimi"].is_kimi());
    assert!(providers["deepseek"].is_deepseek());
    assert!(providers["glm"].is_glm());
}

#[test]
fn test_deserialize_websocket_connect_timeout() {
    let provider_toml = r#"
name = "OpenAI"
base_url = "https://api.odysseythink.com/v1"
websocket_connect_timeout_ms = 15000
supports_websockets = true
        "#;

    let provider: ModelProviderInfo = toml::from_str(provider_toml).unwrap();
    assert_eq!(provider.websocket_connect_timeout_ms, Some(15_000));
}

#[test]
fn test_supports_remote_compaction_for_azure_name() {
    let provider = ModelProviderInfo {
        name: "Azure".into(),
        base_url: Some("https://example.com/odysseythink".into()),
        env_key: Some("AZURE_OPENAI_API_KEY".into()),
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        wire_api: WireApi::Responses,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: None,
        stream_max_retries: None,
        stream_idle_timeout_ms: None,
        websocket_connect_timeout_ms: None,
        supports_websockets: false,
        capabilities: ProviderCapabilities::default(),
    };

    assert!(provider.supports_remote_compaction());
}

#[test]
fn test_supports_remote_compaction_for_non_odysseythink_non_azure_provider() {
    let provider = ModelProviderInfo {
        name: "Example".into(),
        base_url: Some("https://example.com/v1".into()),
        env_key: Some("API_KEY".into()),
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        wire_api: WireApi::Responses,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: None,
        stream_max_retries: None,
        stream_idle_timeout_ms: None,
        websocket_connect_timeout_ms: None,
        supports_websockets: false,
        capabilities: ProviderCapabilities::default(),
    };

    assert!(!provider.supports_remote_compaction());
}

#[test]
fn test_deserialize_provider_auth_config_defaults() {
    let base_dir = tempdir().unwrap();
    let provider_toml = r#"
name = "Corp"

[auth]
command = "./scripts/print-token"
args = ["--format=text"]
        "#;

    let provider: ModelProviderInfo = {
        let _guard = AbsolutePathBufGuard::new(base_dir.path());
        toml::from_str(provider_toml).unwrap()
    };

    assert_eq!(
        provider.auth,
        Some(ModelProviderAuthInfo {
            command: "./scripts/print-token".to_string(),
            args: vec!["--format=text".to_string()],
            timeout_ms: NonZeroU64::new(5_000).unwrap(),
            refresh_interval_ms: 300_000,
            cwd: AbsolutePathBuf::resolve_path_against_base(".", base_dir.path()),
        })
    );
}

#[test]
fn test_merge_configured_model_providers_adds_custom_provider() {
    let custom_provider = ModelProviderInfo {
        name: "Custom".to_string(),
        base_url: Some("https://example.com/v1".to_string()),
        ..ModelProviderInfo::default()
    };
    let configured_model_providers =
        std::collections::HashMap::from([("custom".to_string(), custom_provider.clone())]);

    let mut expected = built_in_model_providers();
    let mut expected_custom = custom_provider;
    expected_custom.normalize_capabilities();
    expected.insert("custom".to_string(), expected_custom);

    assert_eq!(
        merge_configured_model_providers(built_in_model_providers(), configured_model_providers,),
        Ok(expected)
    );
}

#[test]
fn provider_capabilities_default_is_conservative() {
    let caps = ProviderCapabilities::default();
    assert!(!caps.supports_websockets);
    assert!(!caps.supports_remote_compaction);
    assert!(!caps.namespace_tools);
    assert!(!caps.image_generation);
    assert!(!caps.web_search);
    assert!(!caps.command_auth);
    assert!(!caps.attestation);
}

#[test]
fn built_in_kimi_provider_capabilities() {
    let provider = create_kimi_provider();
    assert!(!provider.capabilities.supports_websockets);
    assert!(!provider.capabilities.supports_remote_compaction);
    assert!(!provider.capabilities.namespace_tools);
    assert!(!provider.capabilities.image_generation);
    assert!(!provider.capabilities.web_search);
    assert!(!provider.capabilities.command_auth);
    assert!(!provider.capabilities.attestation);
}

#[test]
fn is_kimi_matches_case_insensitive_name_alias_and_base_url() {
    // Exact canonical name.
    assert!(create_kimi_provider().is_kimi());

    // Case-insensitive name.
    let mut lower = create_kimi_provider();
    lower.name = "kimi".to_string();
    assert!(lower.is_kimi());

    // Common alias.
    let mut moonshot = create_kimi_provider();
    moonshot.name = "Moonshot".to_string();
    assert!(moonshot.is_kimi());

    // Detected via base_url even when the name is a custom label.
    let mut custom = create_kimi_provider();
    custom.name = "My Coding Model".to_string();
    custom.base_url = Some("https://api.moonshot.ai/v1".to_string());
    assert!(custom.is_kimi());

    // The coding endpoint `api.kimi.com` must be detected from the base_url
    // alone, so a provider named e.g. `kimi_gyy` still resolves to the Kimi
    // catalog (real context window) instead of falling back to synthetic
    // metadata that disables auto-compaction.
    let mut coding = create_kimi_provider();
    coding.name = "kimi_gyy".to_string();
    coding.base_url = Some("https://api.kimi.com/coding/v1".to_string());
    assert!(coding.is_kimi());

    // Unrelated provider is not misclassified.
    let mut other = create_kimi_provider();
    other.name = "OpenAI".to_string();
    other.base_url = Some("https://api.openai.com/v1".to_string());
    assert!(!other.is_kimi());
}

#[test]
fn deepseek_and_glm_detection_is_relaxed() {
    let mut deepseek = create_deepseek_provider();
    deepseek.name = "deepseek".to_string();
    assert!(deepseek.is_deepseek());

    let mut glm = create_glm_provider();
    glm.name = "zhipu".to_string();
    assert!(glm.is_glm());

    let mut glm_url = create_glm_provider();
    glm_url.name = "Custom".to_string();
    glm_url.base_url = Some("https://open.bigmodel.cn/api/paas/v4".to_string());
    assert!(glm_url.is_glm());
}

#[test]
fn test_deserialize_provider_capabilities() {
    let provider_toml = r#"
name = "Custom"
base_url = "https://example.com/v1"
wire_api = "responses"

[capabilities]
supports_websockets = true
web_search = false
namespace_tools = true
"#;
    let provider: ModelProviderInfo = toml::from_str(provider_toml).unwrap();
    assert!(provider.capabilities.supports_websockets);
    assert!(!provider.capabilities.web_search);
    assert!(provider.capabilities.namespace_tools);
    assert!(!provider.capabilities.image_generation);
    assert!(provider.validate().is_ok());
}

#[test]
fn default_provider_capabilities_for_responses() {
    let caps = default_provider_capabilities_for_wire_api(WireApi::Responses);
    assert!(caps.supports_websockets);
    assert!(caps.supports_remote_compaction);
    assert!(caps.namespace_tools);
    assert!(caps.image_generation);
    assert!(caps.web_search);
}

#[test]
fn default_provider_capabilities_for_chat_is_conservative() {
    let caps = default_provider_capabilities_for_wire_api(WireApi::Chat);
    assert!(!caps.supports_websockets);
    assert!(!caps.namespace_tools);
    assert!(!caps.image_generation);
    assert!(!caps.web_search);
}

#[test]
fn test_deserialize_provider_auth_config_allows_zero_refresh_interval() {
    let base_dir = tempdir().unwrap();
    let provider_toml = r#"
name = "Corp"

[auth]
command = "./scripts/print-token"
refresh_interval_ms = 0
        "#;

    let provider: ModelProviderInfo = {
        let _guard = AbsolutePathBufGuard::new(base_dir.path());
        toml::from_str(provider_toml).unwrap()
    };

    let auth = provider.auth.expect("auth config should deserialize");
    assert_eq!(auth.refresh_interval_ms, 0);
    assert_eq!(auth.refresh_interval(), None);
}

#[test]
fn normalize_capabilities_fills_responses_defaults() {
    let mut provider = ModelProviderInfo {
        wire_api: WireApi::Responses,
        capabilities: ProviderCapabilities::default(),
        ..ModelProviderInfo::default()
    };
    provider.normalize_capabilities();
    assert!(provider.capabilities.supports_websockets);
    assert!(provider.capabilities.supports_remote_compaction);
    assert!(provider.capabilities.namespace_tools);
    assert!(provider.capabilities.image_generation);
    assert!(provider.capabilities.web_search);
    assert!(!provider.capabilities.command_auth);
    assert!(!provider.capabilities.attestation);
}

#[test]
fn normalize_capabilities_respects_explicit_values() {
    // With all-false replacement semantics, explicit choices are preserved as
    // long as the capability struct is no longer the all-false default.
    let mut provider = ModelProviderInfo {
        wire_api: WireApi::Responses,
        capabilities: ProviderCapabilities {
            supports_websockets: false,
            web_search: false,
            image_generation: true,
            ..ProviderCapabilities::default()
        },
        ..ModelProviderInfo::default()
    };
    provider.normalize_capabilities();
    assert!(!provider.capabilities.supports_websockets);
    assert!(!provider.capabilities.web_search);
    assert!(provider.capabilities.image_generation);
    assert!(!provider.capabilities.supports_remote_compaction);
    assert!(!provider.capabilities.namespace_tools);
    assert!(!provider.capabilities.command_auth);
    assert!(!provider.capabilities.attestation);
}

#[test]
fn user_defined_provider_gets_wire_api_default_capabilities() {
    let built_in = built_in_model_providers();
    let mut configured = HashMap::new();
    configured.insert(
        "my-responses".to_string(),
        ModelProviderInfo {
            name: "My Responses".to_string(),
            wire_api: WireApi::Responses,
            ..Default::default()
        },
    );
    let merged = merge_configured_model_providers(built_in, configured).unwrap();
    let my_responses = merged
        .get("my-responses")
        .expect("user provider should be present");
    assert_eq!(my_responses.wire_api, WireApi::Responses);
    // Responses 的 provider 级推断默认值包含多项 true；实现前 capabilities 为全 false，会失败。
    assert_eq!(
        my_responses.capabilities,
        default_provider_capabilities_for_wire_api(WireApi::Responses)
    );
}

#[test]
fn user_defined_provider_with_explicit_capabilities_is_preserved() {
    let built_in = built_in_model_providers();
    let mut configured = HashMap::new();
    configured.insert(
        "my-chat".to_string(),
        ModelProviderInfo {
            name: "My Chat".to_string(),
            wire_api: WireApi::Chat,
            capabilities: ProviderCapabilities {
                web_search: true,
                ..Default::default()
            },
            ..Default::default()
        },
    );
    let merged = merge_configured_model_providers(built_in, configured).unwrap();
    let my_chat = merged.get("my-chat").unwrap();
    assert!(my_chat.capabilities.web_search);
}
