use std::collections::HashMap;

use serde_json::Value;

use crate::config::WebSearchProviderConfig;
use crate::error::WebSearchError;
use crate::registry::WebSearchProviderRegistry;

pub mod baidu;
pub mod bing;
pub mod duckduckgo;
pub mod exa;
pub mod moonshot;
pub mod perplexity;
pub mod searchapi;
pub mod searxng;
pub mod serpapi;
pub mod serper;
pub mod serply;
pub mod tavily;

/// Create a registry with all 12 web search providers registered.
pub fn create_default_registry() -> WebSearchProviderRegistry {
    let mut registry = WebSearchProviderRegistry::new();
    registry.register(Box::new(bing::BingFactory));
    registry.register(Box::new(duckduckgo::DuckDuckGoFactory));
    registry.register(Box::new(serpapi::SerpApiFactory));
    registry.register(Box::new(searchapi::SearchApiFactory));
    registry.register(Box::new(moonshot::MoonshotFactory));
    registry.register(Box::new(serper::SerperFactory));
    registry.register(Box::new(baidu::BaiduFactory));
    registry.register(Box::new(serply::SerplyFactory));
    registry.register(Box::new(searxng::SearXNGFactory));
    registry.register(Box::new(tavily::TavilyFactory));
    registry.register(Box::new(exa::ExaFactory));
    registry.register(Box::new(perplexity::PerplexityFactory));
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

pub fn take_string_option(options: &mut HashMap<String, Value>, key: &str) -> Option<String> {
    options.remove(key).and_then(|v| match v {
        Value::String(s) => Some(s),
        _ => None,
    })
}

pub fn take_u32_option(
    options: &mut HashMap<String, Value>,
    key: &str,
    default: u32,
    min: u32,
    max: u32,
) -> u32 {
    options
        .remove(key)
        .and_then(|v| match v {
            Value::Number(n) => n.as_u64().map(|u| u as u32),
            _ => None,
        })
        .map(|v| v.clamp(min, max))
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{WebSearchProviderConfig, WebSearchProviderName};

    #[test]
    fn all_providers_are_implemented() {
        let registry = create_default_registry();
        for name in [
            "bing",
            "duckduckgo",
            "serpapi",
            "searchapi",
            "serper",
            "baidu",
            "serply",
            "searxng",
            "tavily",
            "exa",
            "perplexity",
            "moonshot",
        ] {
            let provider_name: WebSearchProviderName = name.parse().expect("valid provider name");
            let mut options = HashMap::new();
            if name == "searxng" {
                options.insert(
                    "base_url".to_string(),
                    Value::String("https://searxng.example/search".to_string()),
                );
            }
            let config = WebSearchProviderConfig {
                provider: provider_name,
                api_key: Some("test-key".to_string()),
                timeout_ms: None,
                options,
            };
            let result = registry.create(&config, reqwest::Client::new());
            assert!(
                result.is_ok(),
                "{} should be implemented but got {:?}",
                name,
                result
            );
        }
    }
}
