use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;

use crate::config::WebSearchProviderConfig;
use crate::error::WebSearchError;
use crate::provider::{
    SharedWebSearchProvider, WebSearchOptions, WebSearchProvider, WebSearchProviderFactory,
    WebSearchResult,
};
use crate::providers::{take_base_url, validate_options};

pub struct SearXNGFactory;

impl WebSearchProviderFactory for SearXNGFactory {
    fn name(&self) -> &str {
        "searxng"
    }

    fn create(
        &self,
        mut config: WebSearchProviderConfig,
        http_client: reqwest::Client,
    ) -> Result<SharedWebSearchProvider, WebSearchError> {
        validate_options(&config, &["base_url"])?;
        let base_url =
            take_base_url(&mut config.options).ok_or_else(|| WebSearchError::Unexpected {
                message: "searxng provider requires a base_url option".to_string(),
            })?;
        let timeout = config
            .timeout_ms
            .map(Duration::from_millis)
            .unwrap_or_else(|| Duration::from_secs(10));
        Ok(std::sync::Arc::new(SearXNGProvider {
            client: http_client,
            base_url,
            timeout,
        }))
    }
}

pub struct SearXNGProvider {
    client: reqwest::Client,
    base_url: String,
    timeout: Duration,
}

impl std::fmt::Debug for SearXNGProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearXNGProvider")
            .field("base_url", &self.base_url)
            .field("timeout", &self.timeout)
            .finish()
    }
}

#[async_trait]
impl WebSearchProvider for SearXNGProvider {
    fn name(&self) -> &str {
        "searxng"
    }

    async fn search(
        &self,
        query: &str,
        options: &WebSearchOptions,
    ) -> Result<Vec<WebSearchResult>, WebSearchError> {
        let request = self
            .client
            .get(&self.base_url)
            .query(&[("q", query), ("format", "json")])
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
        parse_searxng_response(&json, options.limit)
    }
}

fn parse_searxng_response(
    json: &Value,
    requested_limit: Option<u32>,
) -> Result<Vec<WebSearchResult>, WebSearchError> {
    let items = json
        .get("results")
        .and_then(|v| v.as_array())
        .ok_or_else(|| WebSearchError::Unexpected {
            message: "searxng response missing results".to_string(),
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
        let date = item
            .get("publishedDate")
            .and_then(|v| v.as_str())
            .map(String::from);
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
        let limit = limit.clamp(1, 100) as usize;
        results.truncate(limit);
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{WebSearchProviderConfig, WebSearchProviderName};
    use crate::provider::WebSearchOptions;
    use serde_json::json;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn parses_searxng_results() -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        let body = json!({
            "results": [
                { "title": "T", "url": "https://t.example", "content": "C", "publishedDate": "2024-06-06" }
            ]
        });
        Mock::given(method("GET"))
            .and(path("/search"))
            .and(query_param("q", "rust"))
            .and(query_param("format", "json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&body))
            .mount(&server)
            .await;

        let mut options = std::collections::HashMap::new();
        options.insert(
            "base_url".to_string(),
            Value::String(format!("{}/search", server.uri())),
        );
        let config = WebSearchProviderConfig {
            provider: WebSearchProviderName::Searxng,
            api_key: None,
            timeout_ms: None,
            options,
        };
        let provider = SearXNGFactory.create(config, reqwest::Client::new())?;
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
        assert_eq!(results[0].date.as_deref(), Some("2024-06-06"));
        Ok(())
    }

    #[test]
    fn create_requires_base_url() {
        let config = WebSearchProviderConfig {
            provider: WebSearchProviderName::Searxng,
            api_key: None,
            timeout_ms: None,
            options: std::collections::HashMap::new(),
        };
        let result = SearXNGFactory.create(config, reqwest::Client::new());
        assert!(matches!(result, Err(WebSearchError::Unexpected { .. })));
    }
}
