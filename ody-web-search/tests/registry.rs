use std::collections::HashMap;
use std::sync::Arc;

use ody_web_search::config::{WebSearchProviderConfig, WebSearchProviderName};
use ody_web_search::error::WebSearchError;
use ody_web_search::provider::{
    SharedWebSearchProvider, WebSearchOptions, WebSearchProvider, WebSearchProviderFactory,
    WebSearchResult,
};
use ody_web_search::providers::create_default_registry;
use ody_web_search::registry::WebSearchProviderRegistry;

#[derive(Debug)]
struct AlwaysOkFactory;
impl WebSearchProviderFactory for AlwaysOkFactory {
    fn name(&self) -> &str {
        "always_ok"
    }
    fn create(
        &self,
        _config: WebSearchProviderConfig,
        _http_client: reqwest::Client,
    ) -> Result<SharedWebSearchProvider, WebSearchError> {
        Ok(Arc::new(AlwaysOkProvider))
    }
}

#[derive(Debug)]
struct AlwaysOkProvider;
#[async_trait::async_trait]
impl WebSearchProvider for AlwaysOkProvider {
    fn name(&self) -> &str {
        "always_ok"
    }
    async fn search(
        &self,
        _query: &str,
        _options: &WebSearchOptions,
    ) -> Result<Vec<WebSearchResult>, WebSearchError> {
        Ok(Vec::new())
    }
}

fn config_for(name: &str) -> WebSearchProviderConfig {
    let provider: WebSearchProviderName = name.parse().expect("valid provider name");
    let mut options = HashMap::new();
    if name == "searxng" {
        options.insert(
            "base_url".to_string(),
            serde_json::Value::String("https://searxng.example/search".to_string()),
        );
    }
    WebSearchProviderConfig {
        provider,
        api_key: Some("test-key".to_string()),
        timeout_ms: None,
        options,
    }
}

#[test]
fn unknown_provider_returns_error() {
    let mut registry = WebSearchProviderRegistry::new();
    registry.register(Box::new(AlwaysOkFactory));
    let config = config_for("bing");
    let result = registry.create(&config, reqwest::Client::new());
    match result {
        Err(WebSearchError::Unexpected { message }) => {
            assert!(message.contains("unknown web search provider"));
            assert!(message.contains("bing"));
        }
        other => panic!("expected unknown provider error, got {:?}", other),
    }
}

#[test]
fn default_registry_contains_all_twelve_providers() {
    let registry = create_default_registry();
    for name in [
        "bing",
        "serpapi",
        "searchapi",
        "moonshot",
        "duckduckgo",
        "serper",
        "baidu",
        "serply",
        "searxng",
        "tavily",
        "exa",
        "perplexity",
    ] {
        let config = config_for(name);
        let result = registry.create(&config, reqwest::Client::new());
        assert!(
            !matches!(
                result,
                Err(WebSearchError::Unexpected { message }) if message.contains("unknown")
            ),
            "{} should be known to the registry",
            name
        );
    }
}

#[test]
fn first_batch_providers_are_implemented() {
    let registry = create_default_registry();
    for name in ["bing", "serpapi", "searchapi", "moonshot"] {
        let config = config_for(name);
        let result = registry.create(&config, reqwest::Client::new());
        assert!(
            result.is_ok(),
            "{} should create a provider, got {:?}",
            name,
            result
        );
    }
}

#[test]
fn second_batch_providers_are_implemented() {
    let registry = create_default_registry();
    for name in [
        "duckduckgo",
        "serper",
        "baidu",
        "serply",
        "searxng",
        "tavily",
        "exa",
        "perplexity",
    ] {
        let config = config_for(name);
        let result = registry.create(&config, reqwest::Client::new());
        assert!(
            result.is_ok(),
            "expected provider {} to be implemented, got {:?}",
            name,
            result
        );
    }
}
