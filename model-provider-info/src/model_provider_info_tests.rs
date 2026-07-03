use super::*;
use ody_utils_absolute_path::AbsolutePathBuf;
use ody_utils_absolute_path::AbsolutePathBufGuard;
use pretty_assertions::assert_eq;
use std::num::NonZeroU64;
use tempfile::tempdir;

#[test]
fn test_deserialize_ollama_model_provider_toml() {
    let azure_provider_toml = r#"
name = "Ollama"
base_url = "http://localhost:11434/v1"
        "#;
    let expected_provider = ModelProviderInfo {
        name: "Ollama".into(),
        base_url: Some("http://localhost:11434/v1".into()),
        env_key: None,
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
        requires_odysseythink_auth: false,
        supports_websockets: false,
            capabilities: ProviderCapabilities::default(),
    };

    let provider: ModelProviderInfo = toml::from_str(azure_provider_toml).unwrap();
    assert_eq!(expected_provider, provider);
}

#[test]
fn test_deserialize_azure_model_provider_toml() {
    let azure_provider_toml = r#"
name = "Azure"
base_url = "https://xxxxx.odysseythink.azure.com/odysseythink"
env_key = "AZURE_OPENAI_API_KEY"
query_params = { api-version = "2025-04-01-preview" }
        "#;
    let expected_provider = ModelProviderInfo {
        name: "Azure".into(),
        base_url: Some("https://xxxxx.odysseythink.azure.com/odysseythink".into()),
        env_key: Some("AZURE_OPENAI_API_KEY".into()),
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        wire_api: WireApi::Responses,
        query_params: Some(maplit::hashmap! {
            "api-version".to_string() => "2025-04-01-preview".to_string(),
        }),
        http_headers: None,
        env_http_headers: None,
        request_max_retries: None,
        stream_max_retries: None,
        stream_idle_timeout_ms: None,
        websocket_connect_timeout_ms: None,
        requires_odysseythink_auth: false,
        supports_websockets: false,
            capabilities: ProviderCapabilities::default(),
    };

    let provider: ModelProviderInfo = toml::from_str(azure_provider_toml).unwrap();
    assert_eq!(expected_provider, provider);
}

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
        requires_odysseythink_auth: false,
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
    let providers = built_in_model_providers(/*odysseythink_base_url*/ None);
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
fn test_supports_remote_compaction_for_odysseythink() {
    let provider = ModelProviderInfo::create_odysseythink_provider(/*base_url*/ None);

    assert!(provider.supports_remote_compaction());
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
        requires_odysseythink_auth: false,
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
        requires_odysseythink_auth: false,
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

    let mut expected = built_in_model_providers(/*odysseythink_base_url*/ None);
    expected.insert("custom".to_string(), custom_provider);

    assert_eq!(
        merge_configured_model_providers(
            built_in_model_providers(/*odysseythink_base_url*/ None),
            configured_model_providers,
        ),
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
fn built_in_odysseythink_provider_capabilities() {
    let provider = ModelProviderInfo::create_odysseythink_provider(None);
    assert!(provider.capabilities.supports_websockets);
    assert!(provider.capabilities.supports_remote_compaction);
    assert!(provider.capabilities.namespace_tools);
    assert!(provider.capabilities.image_generation);
    assert!(provider.capabilities.web_search);
    assert!(!provider.capabilities.command_auth);
    assert!(!provider.capabilities.attestation);
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
fn default_provider_capabilities_for_local_is_all_false() {
    let caps = default_provider_capabilities_for_wire_api(WireApi::Local);
    assert_eq!(caps, ProviderCapabilities::default());
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
