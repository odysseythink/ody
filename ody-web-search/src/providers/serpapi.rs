use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;

use crate::config::WebSearchProviderConfig;
use crate::error::WebSearchError;
use crate::provider::{
    SharedWebSearchProvider, WebSearchOptions, WebSearchProvider, WebSearchProviderFactory,
    WebSearchResult,
};
use crate::providers::{require_api_key, take_base_url, validate_options};

const DEFAULT_SERPAPI_URL: &str = "https://serpapi.com/search";

pub struct SerpApiFactory;

impl WebSearchProviderFactory for SerpApiFactory {
    fn name(&self) -> &str {
        "serpapi"
    }

    fn create(
        &self,
        mut config: WebSearchProviderConfig,
        http_client: reqwest::Client,
    ) -> Result<SharedWebSearchProvider, WebSearchError> {
        validate_options(&config, &["base_url"])?;
        let api_key = require_api_key(&config)?;
        let base_url =
            take_base_url(&mut config.options).unwrap_or_else(|| DEFAULT_SERPAPI_URL.to_string());
        let timeout = config
            .timeout_ms
            .map(Duration::from_millis)
            .unwrap_or_else(|| Duration::from_secs(10));
        Ok(std::sync::Arc::new(SerpApiProvider {
            client: http_client,
            api_key,
            timeout,
            base_url,
        }))
    }
}

pub struct SerpApiProvider {
    client: reqwest::Client,
    api_key: String,
    timeout: Duration,
    base_url: String,
}

impl std::fmt::Debug for SerpApiProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SerpApiProvider")
            .field("base_url", &self.base_url)
            .field("timeout", &self.timeout)
            .field("api_key", &"***")
            .finish()
    }
}

#[async_trait]
impl WebSearchProvider for SerpApiProvider {
    fn name(&self) -> &str {
        "serpapi"
    }

    async fn search(
        &self,
        query: &str,
        options: &WebSearchOptions,
    ) -> Result<Vec<WebSearchResult>, WebSearchError> {
        let limit = options.limit.map(|l| l.clamp(1, 100)).unwrap_or(10);
        let count = limit.to_string();
        let request = self
            .client
            .get(&self.base_url)
            .query(&[("q", query), ("num", &count), ("api_key", &self.api_key)])
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
        parse_serpapi_response(&json, options.limit)
    }
}

fn parse_serpapi_response(
    json: &Value,
    requested_limit: Option<u32>,
) -> Result<Vec<WebSearchResult>, WebSearchError> {
    let items = json
        .get("organic_results")
        .and_then(|v| v.as_array())
        .ok_or_else(|| WebSearchError::Unexpected {
            message: "serpapi response missing organic_results".to_string(),
        })?;
    let mut results = Vec::new();
    for item in items {
        let title = item
            .get("title")
            .and_then(|v| v.as_str())
            .map_or_else(String::new, String::from);
        let url = item
            .get("link")
            .and_then(|v| v.as_str())
            .map_or_else(String::new, String::from);
        let snippet = item
            .get("snippet")
            .and_then(|v| v.as_str())
            .map_or_else(String::new, String::from);
        let date = item.get("date").and_then(|v| v.as_str()).map(String::from);
        results.push(WebSearchResult {
            title,
            url,
            snippet,
            date,
            content: None,
        });
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
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn parses_organic_results() -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        let body = json!({
            "organic_results": [
                { "title": "T", "link": "https://t.example", "snippet": "S", "date": "2024-02-02" }
            ]
        });
        Mock::given(method("GET"))
            .and(path("/search"))
            .and(query_param("q", "rust"))
            .and(query_param("num", "3"))
            .and(query_param("api_key", "key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&body))
            .mount(&server)
            .await;

        let mut options = std::collections::HashMap::new();
        options.insert(
            "base_url".to_string(),
            Value::String(format!("{}/search", server.uri())),
        );
        let config = WebSearchProviderConfig {
            provider: WebSearchProviderName::Serpapi,
            api_key: Some("key".to_string()),
            timeout_ms: None,
            options,
        };
        let provider = SerpApiFactory.create(config, reqwest::Client::new())?;
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
        assert_eq!(results[0].url, "https://t.example");
        Ok(())
    }
}
