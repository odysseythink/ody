use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::config::WebSearchProviderConfig;
use crate::error::WebSearchError;

/// One normalized web search result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WebSearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub date: Option<String>,
    pub content: Option<String>,
}

/// Options that `WebSearchTool` passes to a provider for a single search call.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WebSearchOptions {
    pub limit: Option<u32>,
    pub include_content: Option<bool>,
    pub tool_call_id: Option<String>,
}

/// Structured output of `WebSearchTool`, consumed by both the model and the TUI chip.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WebSearchToolOutput {
    pub result_count: usize,
    pub text: String,
}

#[async_trait::async_trait]
pub trait WebSearchProvider: Send + Sync + std::fmt::Debug {
    fn name(&self) -> &str;

    async fn search(
        &self,
        query: &str,
        options: &WebSearchOptions,
    ) -> Result<Vec<WebSearchResult>, WebSearchError>;
}

pub type SharedWebSearchProvider = Arc<dyn WebSearchProvider>;

/// Factory used by the registry to create a provider from TOML config + a shared HTTP client.
#[async_trait::async_trait]
pub trait WebSearchProviderFactory: Send + Sync {
    fn name(&self) -> &str;

    fn create(
        &self,
        config: WebSearchProviderConfig,
        http_client: reqwest::Client,
    ) -> Result<SharedWebSearchProvider, WebSearchError>;
}
