use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::config::WebSearchProviderConfig;
use crate::error::WebSearchError;
use crate::provider::{
    SharedWebSearchProvider, WebSearchOptions, WebSearchProvider, WebSearchProviderFactory,
    WebSearchResult,
};
use crate::providers::{require_api_key, take_base_url, validate_options};

const DEFAULT_MOONSHOT_URL: &str = "https://api.moonshot.cn/v1/search";

pub struct MoonshotFactory;

impl WebSearchProviderFactory for MoonshotFactory {
    fn name(&self) -> &str {
        "moonshot"
    }

    fn create(
        &self,
        mut config: WebSearchProviderConfig,
        http_client: reqwest::Client,
    ) -> Result<SharedWebSearchProvider, WebSearchError> {
        validate_options(&config, &["base_url"])?;
        let api_key = require_api_key(&config)?;
        let base_url = take_base_url(&mut config.options)
            .unwrap_or_else(|| DEFAULT_MOONSHOT_URL.to_string());
        let timeout = config
            .timeout_ms
            .map(Duration::from_millis)
            .unwrap_or_else(|| Duration::from_secs(10));
        Ok(std::sync::Arc::new(MoonshotProvider {
            client: http_client,
            api_key,
            timeout,
            base_url,
        }))
    }
}

pub struct MoonshotProvider {
    client: reqwest::Client,
    api_key: String,
    timeout: Duration,
    base_url: String,
}

impl std::fmt::Debug for MoonshotProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MoonshotProvider")
            .field("base_url", &self.base_url)
            .field("timeout", &self.timeout)
            .field("api_key", &"***")
            .finish()
    }
}

#[async_trait]
impl WebSearchProvider for MoonshotProvider {
    fn name(&self) -> &str {
        "moonshot"
    }

    async fn search(
        &self,
        query: &str,
        options: &WebSearchOptions,
    ) -> Result<Vec<WebSearchResult>, WebSearchError> {
        let limit = options.limit.map(|l| l.clamp(1, 50)).unwrap_or(10);
        let body = json!({
            "query": query,
            "top_n": limit,
        });
        let request = self
            .client
            .post(&self.base_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .timeout(self.timeout);
        let response = request.send().await.map_err(|e| WebSearchError::from_reqwest(&e))?;
        let status = response.status();
        if !status.is_success() {
            let body = match response.text().await {
                Ok(body) => body,
                Err(_) => String::new(),
            };
            return Err(WebSearchError::from_http_status(status, &body));
        }
        let json: Value = response.json().await.map_err(|e| WebSearchError::from_reqwest(&e))?;
        parse_moonshot_response(&json, options.limit)
    }
}

fn parse_moonshot_response(
    json: &Value,
    requested_limit: Option<u32>,
) -> Result<Vec<WebSearchResult>, WebSearchError> {
    let items = json
        .get("results")
        .and_then(|v| v.as_array())
        .ok_or_else(|| WebSearchError::Unexpected {
            message: "moonshot response missing results".to_string(),
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
        let date = item
            .get("date")
            .and_then(|v| v.as_str())
            .map(String::from);
        let content = item
            .get("content")
            .and_then(|v| v.as_str())
            .map(String::from);
        results.push(WebSearchResult {
            title,
            url,
            snippet,
            date,
            content,
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
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn parses_moonshot_results() -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        let body = json!({
            "results": [
                {
                    "title": "M",
                    "link": "https://m.example",
                    "snippet": "N",
                    "date": "2024-03-03",
                    "content": "full content"
                }
            ]
        });
        Mock::given(method("POST"))
            .and(path("/v1/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&body))
            .mount(&server)
            .await;

        let mut options = std::collections::HashMap::new();
        options.insert(
            "base_url".to_string(),
            Value::String(format!("{}/v1/search", server.uri())),
        );
        let config = WebSearchProviderConfig {
            provider: WebSearchProviderName::Moonshot,
            api_key: Some("key".to_string()),
            timeout_ms: None,
            options,
        };
        let provider = MoonshotFactory.create(config, reqwest::Client::new())?;
        let results = provider
            .search("hello", &WebSearchOptions { limit: Some(4), include_content: None, tool_call_id: None })
            .await?;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "M");
        assert_eq!(results[0].content.as_deref(), Some("full content"));
        Ok(())
    }
}
