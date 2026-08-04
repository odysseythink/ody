use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::config::WebSearchProviderConfig;
use crate::error::WebSearchError;
use crate::provider::{
    SharedWebSearchProvider, WebSearchOptions, WebSearchProvider, WebSearchProviderFactory,
    WebSearchResult,
};
use crate::providers::{require_api_key, take_base_url, take_u32_option, validate_options};

const DEFAULT_PERPLEXITY_URL: &str = "https://api.perplexity.ai/search";

pub struct PerplexityFactory;

impl WebSearchProviderFactory for PerplexityFactory {
    fn name(&self) -> &str {
        "perplexity"
    }

    fn create(
        &self,
        mut config: WebSearchProviderConfig,
        http_client: reqwest::Client,
    ) -> Result<SharedWebSearchProvider, WebSearchError> {
        validate_options(&config, &["base_url", "max_results", "max_tokens_per_page"])?;
        let api_key = require_api_key(&config)?;
        let base_url = take_base_url(&mut config.options)
            .unwrap_or_else(|| DEFAULT_PERPLEXITY_URL.to_string());
        let max_results = take_u32_option(&mut config.options, "max_results", 5, 1, 20);
        let max_tokens_per_page = take_u32_option(
            &mut config.options,
            "max_tokens_per_page",
            2048,
            1,
            u32::MAX,
        );
        let timeout = config
            .timeout_ms
            .map(Duration::from_millis)
            .unwrap_or_else(|| Duration::from_secs(10));
        Ok(std::sync::Arc::new(PerplexityProvider {
            client: http_client,
            api_key,
            base_url,
            max_results,
            max_tokens_per_page,
            timeout,
        }))
    }
}

pub struct PerplexityProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    max_results: u32,
    max_tokens_per_page: u32,
    timeout: Duration,
}

impl std::fmt::Debug for PerplexityProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PerplexityProvider")
            .field("base_url", &self.base_url)
            .field("max_results", &self.max_results)
            .field("max_tokens_per_page", &self.max_tokens_per_page)
            .field("timeout", &self.timeout)
            .field("api_key", &"***")
            .finish()
    }
}

#[async_trait]
impl WebSearchProvider for PerplexityProvider {
    fn name(&self) -> &str {
        "perplexity"
    }

    async fn search(
        &self,
        query: &str,
        options: &WebSearchOptions,
    ) -> Result<Vec<WebSearchResult>, WebSearchError> {
        let limit = options
            .limit
            .map(|l| l.clamp(1, 20))
            .unwrap_or(self.max_results);
        let body = json!({
            "query": query,
            "max_results": limit,
            "max_tokens_per_page": self.max_tokens_per_page,
        });
        let request = self
            .client
            .post(&self.base_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
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
        parse_perplexity_response(&json, options.limit)
    }
}

fn parse_perplexity_response(
    json: &Value,
    requested_limit: Option<u32>,
) -> Result<Vec<WebSearchResult>, WebSearchError> {
    let items = json
        .get("results")
        .and_then(|v| v.as_array())
        .ok_or_else(|| WebSearchError::Unexpected {
            message: "perplexity response missing results".to_string(),
        })?;
    let mut results = Vec::new();
    for item in items {
        let title = item
            .get("title")
            .and_then(|v| v.as_str())
            .map_or_else(String::new, String::from);
        let url = item
            .get("url")
            .or_else(|| item.get("link"))
            .and_then(|v| v.as_str())
            .map_or_else(String::new, String::from);
        let snippet = item
            .get("snippet")
            .and_then(|v| v.as_str())
            .or_else(|| item.get("text").and_then(|v| v.as_str()))
            .map_or_else(String::new, String::from);
        let date = item.get("date").and_then(|v| v.as_str()).map(String::from);
        if !title.is_empty() && !url.is_empty() {
            results.push(WebSearchResult {
                title,
                url,
                snippet,
                date,
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
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn parses_perplexity_results() -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        let body = json!({
            "results": [
                { "title": "T", "url": "https://t.example", "snippet": "S", "date": "2024-08-08" }
            ]
        });
        Mock::given(method("POST"))
            .and(path("/search"))
            .and(header("Authorization", "Bearer key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&body))
            .mount(&server)
            .await;

        let mut options = std::collections::HashMap::new();
        options.insert(
            "base_url".to_string(),
            Value::String(format!("{}/search", server.uri())),
        );
        let config = WebSearchProviderConfig {
            provider: WebSearchProviderName::Perplexity,
            api_key: Some("key".to_string()),
            timeout_ms: None,
            options,
        };
        let provider = PerplexityFactory.create(config, reqwest::Client::new())?;
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
        assert_eq!(results[0].date.as_deref(), Some("2024-08-08"));
        Ok(())
    }
}
