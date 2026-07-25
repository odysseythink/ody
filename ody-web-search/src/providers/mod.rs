use std::collections::HashMap;

use serde_json::Value;

use crate::config::WebSearchProviderConfig;
use crate::error::WebSearchError;
use crate::provider::{SharedWebSearchProvider, WebSearchProviderFactory};
use crate::registry::WebSearchProviderRegistry;

pub mod bing;
pub mod moonshot;
pub mod searchapi;
pub mod serpapi;

pub struct NotImplementedFactory(&'static str);

impl WebSearchProviderFactory for NotImplementedFactory {
    fn name(&self) -> &str {
        self.0
    }

    fn create(
        &self,
        config: WebSearchProviderConfig,
        _http_client: reqwest::Client,
    ) -> Result<SharedWebSearchProvider, WebSearchError> {
        Err(WebSearchError::Unexpected {
            message: format!(
                "{} provider is not implemented in this release",
                config.provider
            ),
        })
    }
}

/// Create a registry with all 12 provider names registered.
/// First-batch providers are fully implemented; second-batch providers return
/// `WebSearchError::Unexpected { "not implemented" }` at runtime.
pub fn create_default_registry() -> WebSearchProviderRegistry {
    let mut registry = WebSearchProviderRegistry::new();
    registry.register(Box::new(bing::BingFactory));
    registry.register(Box::new(serpapi::SerpApiFactory));
    registry.register(Box::new(searchapi::SearchApiFactory));
    registry.register(Box::new(moonshot::MoonshotFactory));
    registry.register(Box::new(NotImplementedFactory("duckduckgo")));
    registry.register(Box::new(NotImplementedFactory("serper")));
    registry.register(Box::new(NotImplementedFactory("baidu")));
    registry.register(Box::new(NotImplementedFactory("serply")));
    registry.register(Box::new(NotImplementedFactory("searxng")));
    registry.register(Box::new(NotImplementedFactory("tavily")));
    registry.register(Box::new(NotImplementedFactory("exa")));
    registry.register(Box::new(NotImplementedFactory("perplexity")));
    registry
}

pub fn validate_options(
    config: &WebSearchProviderConfig,
    allowed: &[&str],
) -> Result<(), WebSearchError> {
    for key in config.options.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(WebSearchError::Unexpected {
                message: format!(
                    "unknown option '{}' for provider '{}'",
                    key, config.provider
                ),
            });
        }
    }
    Ok(())
}

pub fn require_api_key(config: &WebSearchProviderConfig) -> Result<String, WebSearchError> {
    if let Some(key) = config.api_key.clone() {
        return Ok(key);
    }
    let env_name = format!("{}_API_KEY", config.provider.to_string().to_uppercase());
    if let Ok(key) = std::env::var(&env_name) {
        return Ok(key);
    }
    Err(WebSearchError::Unexpected {
        message: format!(
            "provider {} requires an api_key or {} env var",
            config.provider, env_name
        ),
    })
}

pub fn take_base_url(options: &mut HashMap<String, Value>) -> Option<String> {
    options.remove("base_url").and_then(|v| match v {
        Value::String(s) => Some(s),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{WebSearchProviderConfig, WebSearchProviderName};
    use crate::error::WebSearchError;

    #[test]
    fn not_implemented_factory_returns_expected_error() {
        let registry = create_default_registry();
        for (name, expected_impl) in [
            ("bing", true),
            ("serpapi", true),
            ("searchapi", true),
            ("moonshot", true),
            ("duckduckgo", false),
            ("tavily", false),
        ] {
            let provider_name: WebSearchProviderName = name.parse().expect("valid provider name");
            let config = WebSearchProviderConfig {
                provider: provider_name,
                api_key: Some("test-key".to_string()),
                timeout_ms: None,
                options: HashMap::new(),
            };
            let result = registry.create(&config, reqwest::Client::new());
            if expected_impl {
                assert!(
                    result.is_ok(),
                    "{} should be implemented but got {:?}",
                    name,
                    result
                );
            } else {
                match result {
                    Err(WebSearchError::Unexpected { message }) => {
                        assert!(message.contains("not implemented"), "message: {message}");
                    }
                    other => panic!("expected not implemented error for {name}, got {:?}", other),
                }
            }
        }
    }
}
