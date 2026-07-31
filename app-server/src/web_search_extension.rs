use std::sync::Arc;

use ody_core::config::Config;
use ody_extension_api::{
    ConfigContributor, ExtensionData, ExtensionFuture, ExtensionRegistryBuilder,
    ThreadLifecycleContributor, ThreadStartInput, ToolCall, ToolContributor,
};
use ody_web_search::{
    config::ServicesConfig, fallback::FallbackWebSearchProvider, http_client::default_http_client,
    provider::SharedWebSearchProvider, providers::create_default_registry, tool::WebSearchTool,
};

#[derive(Clone)]
struct WebSearchProviderHandle(SharedWebSearchProvider);

#[derive(Clone)]
struct WebSearchExtension;

type StoredProvider = WebSearchProviderHandle;

impl WebSearchExtension {
    fn create_provider(services: &ServicesConfig) -> Option<StoredProvider> {
        let web_search_config = services.web_search.as_ref()?;
        let registry = create_default_registry();
        let client = default_http_client();
        let primary = registry
            .create(&web_search_config.primary, client.clone())
            .ok()?;
        let secondary = web_search_config
            .secondary
            .as_ref()
            .and_then(|cfg| registry.create(cfg, client.clone()).ok());
        let provider: SharedWebSearchProvider =
            Arc::new(FallbackWebSearchProvider::new(primary, secondary));
        Some(WebSearchProviderHandle(provider))
    }
}

impl ThreadLifecycleContributor<Config> for WebSearchExtension {
    fn on_thread_start<'a>(
        &'a self,
        input: ThreadStartInput<'a, Config>,
    ) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            if let Some(services) = input.config.services.as_ref() {
                if let Some(provider) = Self::create_provider(services) {
                    input.thread_store.insert(provider);
                }
            }
        })
    }
}

impl ConfigContributor<Config> for WebSearchExtension {
    fn on_config_changed(
        &self,
        _session_store: &ExtensionData,
        thread_store: &ExtensionData,
        _previous_config: &Config,
        new_config: &Config,
    ) {
        if let Some(services) = new_config.services.as_ref() {
            if let Some(provider) = Self::create_provider(services) {
                thread_store.insert(provider);
            }
        } else {
            let _: Option<Arc<StoredProvider>> = thread_store.remove();
        }
    }
}

impl ToolContributor for WebSearchExtension {
    fn tools(
        &self,
        session_store: &ExtensionData,
        thread_store: &ExtensionData,
    ) -> Vec<Arc<dyn ody_extension_api::ToolExecutor<ToolCall>>> {
        let Some(handle) = thread_store.get::<StoredProvider>() else {
            return Vec::new();
        };
        vec![Arc::new(WebSearchTool::new(
            session_store.level_id().to_string(),
            handle.0.clone(),
        ))]
    }
}

pub fn install(registry: &mut ExtensionRegistryBuilder<Config>) {
    let extension = Arc::new(WebSearchExtension);
    registry.thread_lifecycle_contributor(extension.clone());
    registry.config_contributor(extension.clone());
    registry.tool_contributor(extension);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ody_extension_api::ExtensionDataInit;
    use ody_web_search::config::{WebSearchConfig, WebSearchProviderConfig, WebSearchProviderName};
    use std::collections::HashMap;

    fn services_config() -> ServicesConfig {
        ServicesConfig {
            web_search: Some(WebSearchConfig {
                primary: WebSearchProviderConfig {
                    provider: WebSearchProviderName::Duckduckgo,
                    api_key: None,
                    timeout_ms: None,
                    options: HashMap::new(),
                },
                secondary: None,
            }),
        }
    }

    #[test]
    fn create_provider_returns_provider_for_duckduckgo() {
        let services = services_config();
        // DuckDuckGo requires no API key and is fully implemented.
        assert!(WebSearchExtension::create_provider(&services).is_some());
    }

    #[test]
    fn create_provider_returns_provider_for_implemented_provider() {
        let services = ServicesConfig {
            web_search: Some(WebSearchConfig {
                primary: WebSearchProviderConfig {
                    provider: WebSearchProviderName::Moonshot,
                    api_key: Some("test-key".to_string()),
                    timeout_ms: None,
                    options: HashMap::new(),
                },
                secondary: None,
            }),
        };
        assert!(WebSearchExtension::create_provider(&services).is_some());
    }

    #[test]
    fn tools_returns_empty_when_no_provider_in_thread_store() {
        let session_store = ExtensionData::new("session");
        let thread_store = ExtensionData::new("thread");
        let extension = WebSearchExtension;
        let tools = extension.tools(&session_store, &thread_store);
        assert!(tools.is_empty());
    }

    #[test]
    fn tools_returns_web_search_when_provider_present() {
        let session_store = ExtensionData::new("session");
        let mut thread_store = ExtensionData::new_with_init("thread", ExtensionDataInit::new());
        let provider = WebSearchExtension::create_provider(&ServicesConfig {
            web_search: Some(WebSearchConfig {
                primary: WebSearchProviderConfig {
                    provider: WebSearchProviderName::Moonshot,
                    api_key: Some("test-key".to_string()),
                    timeout_ms: None,
                    options: HashMap::new(),
                },
                secondary: None,
            }),
        })
        .expect("should create provider");
        thread_store.insert(provider);
        let extension = WebSearchExtension;
        let tools = extension.tools(&session_store, &thread_store);
        assert_eq!(tools.len(), 1);
        assert_eq!(
            tools[0].tool_name(),
            ody_tools::ToolName::plain("WebSearch")
        );
    }
}
