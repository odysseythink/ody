use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::config::WebSearchProviderConfig;
use crate::error::WebSearchError;
use crate::provider::{
    SharedWebSearchProvider, WebSearchOptions, WebSearchProvider, WebSearchProviderFactory,
    WebSearchResult,
};
use crate::providers::{require_api_key, take_base_url, take_string_option, validate_options};

const DEFAULT_TAVILY_URL: &str = "https://api.tavily.com/search";

pub struct TavilyFactory;

impl WebSearchProviderFactory for TavilyFactory {
    fn name(&self) -> &str {
        "tavily"
    }

    fn create(
        &self,
        mut config: WebSearchProviderConfig,
        http_client: reqwest::Client,
    ) -> Result<SharedWebSearchProvider, WebSearchError> {
        validate_options(&config, &["base_url", "search_depth"])?;
        let api_key = require_api_key(&config)?;
        let base_url =
            take_base_url(&mut config.options).unwrap_or_else(|| DEFAULT_TAVILY_URL.to_string());
        let search_depth = take_string_option(&mut config.options, "search_depth")
            .unwrap_or_else(|| "basic".to_string());
        let timeout = config
            .timeout_ms
            .map(Duration::from_millis)
            .unwrap_or_else(|| Duration::from_secs(10));
        Ok(std::sync::Arc::new(TavilyProvider {
            client: http_client,
            api_key,
            base_url,
            search_depth,
            timeout,
        }))
    }
}

pub struct TavilyProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    search_depth: String,
    timeout: Duration,
}

impl std::fmt::Debug for TavilyProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TavilyProvider")
            .field("base_url", &self.base_url)
            .field("search_depth", &self.search_depth)
            .field("timeout", &self.timeout)
            .field("api_key", &"***")
            .finish()
    }
}

#[async_trait]
impl WebSearchProvider for TavilyProvider {
    fn name(&self) -> &str {
        "tavily"
    }

    async fn search(
        &self,
        query: &str,
        options: &WebSearchOptions,
    ) -> Result<Vec<WebSearchResult>, WebSearchError> {
        let limit = options.limit.map(|l| l.clamp(1, 100)).unwrap_or(10);
        let body = json!({
            "api_key": self.api_key,
            "query": query,
            "search_depth": self.search_depth,
            "max_results": limit,
        });
        let request = self
            .client
            .post(&self.base_url)
            .header("Content-Type", "application/json")
            .json(&body)
            .timeout(self.timeout);
        let response = request
            .send()
            .await
            .map_err(|e| WebSearchError::from_reqwest(&e))?;
        let status = response.status();
        if !status.is_success() {
            let body = match response.text().await {
                Ok(body) => body,
                Err(_) => String::new(),
            };
            return Err(WebSearchError::from_http_status(status, &body));
        }
        let json: Value = response
            .json()
            .await
            .map_err(|e| WebSearchError::from_reqwest(&e))?;
        parse_tavily_response(&json, options.limit)
    }
}

fn parse_tavily_response(
    json: &Value,
    requested_limit: Option<u32>,
) -> Result<Vec<WebSearchResult>, WebSearchError> {
    let items = json
        .get("results")
        .and_then(|v| v.as_array())
        .ok_or_else(|| WebSearchError::Unexpected {
            message: "tavily response missing results".to_string(),
        })?;
    let mut results = Vec::new();
    for item in items {
        let title = item
            .get("title")
            .and_then(|v| v.as_str())
            .map_or_else(String::new, String::from);
        let url = item
            .get("url")
            .and_then(|v| v.as_str())
            .map_or_else(String::new, String::from);
        let snippet = item
            .get("content")
            .and_then(|v| v.as_str())
            .map_or_else(String::new, String::from);
        if !title.is_empty() && !url.is_empty() {
            results.push(WebSearchResult {
                title,
                url,
                snippet,
                date: None,
                content: None,
            });
        }
    }
    if let Some(limit) = requested_limit {
        results.truncate(limit as usize);
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{WebSearchProviderConfig, WebSearchProviderName};
    use crate::provider::WebSearchOptions;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn parses_tavily_results() -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        let body = json!({
            "results": [
                { "title": "T", "url": "https://t.example", "content": "C" }
            ]
        });
        Mock::given(method("POST"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&body))
            .mount(&server)
            .await;

        let mut options = std::collections::HashMap::new();
        options.insert(
            "base_url".to_string(),
            Value::String(format!("{}/search", server.uri())),
        );
        options.insert(
            "search_depth".to_string(),
            Value::String("advanced".to_string()),
        );
        let config = WebSearchProviderConfig {
            provider: WebSearchProviderName::Tavily,
            api_key: Some("key".to_string()),
            timeout_ms: None,
            options,
        };
        let provider = TavilyFactory.create(config, reqwest::Client::new())?;
        let results = provider
            .search(
                "rust",
                &WebSearchOptions {
                    limit: Some(3),
                    include_content: None,
                    tool_call_id: None,
                },
            )
            .await?;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "T");
        Ok(())
    }
}
