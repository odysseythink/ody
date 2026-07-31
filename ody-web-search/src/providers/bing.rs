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

const DEFAULT_BING_URL: &str = "https://api.bing.microsoft.com/v7.0/search";

pub struct BingFactory;

impl WebSearchProviderFactory for BingFactory {
    fn name(&self) -> &str {
        "bing"
    }

    fn create(
        &self,
        mut config: WebSearchProviderConfig,
        http_client: reqwest::Client,
    ) -> Result<SharedWebSearchProvider, WebSearchError> {
        validate_options(&config, &["base_url"])?;
        let api_key = require_api_key(&config)?;
        let base_url =
            take_base_url(&mut config.options).unwrap_or_else(|| DEFAULT_BING_URL.to_string());
        let timeout = config
            .timeout_ms
            .map(Duration::from_millis)
            .unwrap_or_else(|| Duration::from_secs(10));
        Ok(std::sync::Arc::new(BingProvider {
            client: http_client,
            api_key,
            timeout,
            base_url,
        }))
    }
}

pub struct BingProvider {
    client: reqwest::Client,
    api_key: String,
    timeout: Duration,
    base_url: String,
}

impl std::fmt::Debug for BingProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BingProvider")
            .field("base_url", &self.base_url)
            .field("timeout", &self.timeout)
            .field("api_key", &"***")
            .finish()
    }
}

#[async_trait]
impl WebSearchProvider for BingProvider {
    fn name(&self) -> &str {
        "bing"
    }

    async fn search(
        &self,
        query: &str,
        options: &WebSearchOptions,
    ) -> Result<Vec<WebSearchResult>, WebSearchError> {
        let limit = options.limit.map(|l| l.clamp(1, 50)).unwrap_or(10);
        let count = limit.to_string();
        let request = self
            .client
            .get(&self.base_url)
            .header("Ocp-Apim-Subscription-Key", &self.api_key)
            .query(&[("q", query), ("count", &count)])
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
        parse_bing_response(&json, options.limit)
    }
}

fn parse_bing_response(
    json: &Value,
    requested_limit: Option<u32>,
) -> Result<Vec<WebSearchResult>, WebSearchError> {
    let pages = json
        .get("webPages")
        .and_then(|v| v.get("value"))
        .and_then(|v| v.as_array())
        .ok_or_else(|| WebSearchError::Unexpected {
            message: "bing response missing webPages.value".to_string(),
        })?;
    let mut results = Vec::new();
    for item in pages {
        let title = item
            .get("name")
            .and_then(|v| v.as_str())
            .map_or_else(String::new, String::from);
        let url = item
            .get("url")
            .and_then(|v| v.as_str())
            .map_or_else(String::new, String::from);
        let snippet = item
            .get("snippet")
            .and_then(|v| v.as_str())
            .map_or_else(String::new, String::from);
        let date = item
            .get("dateLastCrawled")
            .and_then(|v| v.as_str())
            .map(String::from);
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
    async fn parses_bing_web_pages() -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        let body = json!({
            "webPages": {
                "value": [
                    {
                        "name": "A",
                        "url": "https://a.example",
                        "snippet": "snippet A",
                        "dateLastCrawled": "2024-01-01"
                    },
                    {
                        "name": "B",
                        "url": "https://b.example",
                        "snippet": "snippet B"
                    }
                ]
            }
        });
        Mock::given(method("GET"))
            .and(path("/v7.0/search"))
            .and(query_param("q", "hello"))
            .and(query_param("count", "5"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&body))
            .mount(&server)
            .await;

        let mut options = std::collections::HashMap::new();
        options.insert(
            "base_url".to_string(),
            Value::String(format!("{}/v7.0/search", server.uri())),
        );
        let config = WebSearchProviderConfig {
            provider: WebSearchProviderName::Bing,
            api_key: Some("key".to_string()),
            timeout_ms: None,
            options,
        };
        let factory = BingFactory;
        let provider = factory.create(config, reqwest::Client::new())?;
        let results = provider
            .search(
                "hello",
                &WebSearchOptions {
                    limit: Some(5),
                    include_content: None,
                    tool_call_id: None,
                },
            )
            .await?;
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "A");
        assert_eq!(results[0].url, "https://a.example");
        assert_eq!(results[0].date.as_deref(), Some("2024-01-01"));
        Ok(())
    }

    #[tokio::test]
    async fn returns_error_on_missing_web_pages() -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v7.0/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&json!({})))
            .mount(&server)
            .await;

        let mut options = std::collections::HashMap::new();
        options.insert(
            "base_url".to_string(),
            Value::String(format!("{}/v7.0/search", server.uri())),
        );
        let config = WebSearchProviderConfig {
            provider: WebSearchProviderName::Bing,
            api_key: Some("key".to_string()),
            timeout_ms: None,
            options,
        };
        let provider = BingFactory.create(config, reqwest::Client::new())?;
        let result = provider.search("hello", &WebSearchOptions::default()).await;
        assert!(matches!(result, Err(WebSearchError::Unexpected { .. })));
        Ok(())
    }
}
