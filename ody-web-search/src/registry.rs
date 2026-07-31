use std::collections::HashMap;

use crate::config::WebSearchProviderConfig;
use crate::error::WebSearchError;
use crate::provider::{SharedWebSearchProvider, WebSearchProviderFactory};

pub struct WebSearchProviderRegistry {
    factories: HashMap<String, Box<dyn WebSearchProviderFactory>>,
}

impl WebSearchProviderRegistry {
    pub fn new() -> Self {
        Self {
            factories: HashMap::new(),
        }
    }

    pub fn register(&mut self, factory: Box<dyn WebSearchProviderFactory>) {
        self.factories.insert(factory.name().to_string(), factory);
    }

    pub fn create(
        &self,
        config: &WebSearchProviderConfig,
        http_client: reqwest::Client,
    ) -> Result<SharedWebSearchProvider, WebSearchError> {
        let name = config.provider.to_string();
        let factory = self
            .factories
            .get(&name)
            .ok_or_else(|| WebSearchError::Unexpected {
                message: format!("unknown web search provider: {}", name),
            })?;
        factory.create(config.clone(), http_client)
    }
}

impl Default for WebSearchProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WebSearchProviderConfig;
    use crate::error::WebSearchError;
    use crate::provider::{
        SharedWebSearchProvider, WebSearchOptions, WebSearchProvider, WebSearchProviderFactory,
        WebSearchResult,
    };
    use std::sync::Arc;

    #[derive(Debug)]
    struct BingFactory;
    impl WebSearchProviderFactory for BingFactory {
        fn name(&self) -> &str {
            "bing"
        }
        fn create(
            &self,
            _config: WebSearchProviderConfig,
            _http_client: reqwest::Client,
        ) -> Result<SharedWebSearchProvider, WebSearchError> {
            Ok(Arc::new(OkProvider))
        }
    }

    #[derive(Debug)]
    struct OkProvider;
    #[async_trait::async_trait]
    impl WebSearchProvider for OkProvider {
        fn name(&self) -> &str {
            "ok"
        }
        async fn search(
            &self,
            _query: &str,
            _options: &WebSearchOptions,
        ) -> Result<Vec<WebSearchResult>, WebSearchError> {
            Ok(Vec::new())
        }
    }

    fn bing_config() -> WebSearchProviderConfig {
        WebSearchProviderConfig {
            provider: crate::config::WebSearchProviderName::Bing,
            api_key: None,
            timeout_ms: None,
            options: HashMap::new(),
        }
    }

    #[test]
    fn unknown_provider_returns_error() {
        let registry = WebSearchProviderRegistry::new();
        let result = registry.create(&bing_config(), reqwest::Client::new());
        match result {
            Err(WebSearchError::Unexpected { message }) => {
                assert!(message.contains("unknown web search provider"));
                assert!(message.contains("bing"));
            }
            other => panic!("expected unknown provider error, got {:?}", other),
        }
    }

    #[test]
    fn registered_factory_creates_provider() {
        let mut registry = WebSearchProviderRegistry::new();
        registry.register(Box::new(BingFactory));
        let config = bing_config();
        let result = registry.create(&config, reqwest::Client::new());
        assert!(
            result.is_ok(),
            "expected registered provider to be created: {:?}",
            result
        );
    }
}
