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

const DEFAULT_SERPER_URL: &str = "https://google.serper.dev/search";

pub struct SerperFactory;

impl WebSearchProviderFactory for SerperFactory {
    fn name(&self) -> &str {
        "serper"
    }

    fn create(
        &self,
        mut config: WebSearchProviderConfig,
        http_client: reqwest::Client,
    ) -> Result<SharedWebSearchProvider, WebSearchError> {
        validate_options(&config, &["base_url"])?;
        let api_key = require_api_key(&config)?;
        let base_url = take_base_url(&mut config.options)
            .unwrap_or_else(|| DEFAULT_SERPER_URL.to_string());
        let timeout = config
            .timeout_ms
            .map(Duration::from_millis)
            .unwrap_or_else(|| Duration::from_secs(10));
        Ok(std::sync::Arc::new(SerperProvider {
            client: http_client,
            api_key,
            base_url,
            timeout,
        }))
    }
}

pub struct SerperProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    timeout: Duration,
}

impl std::fmt::Debug for SerperProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SerperProvider")
            .field("base_url", &self.base_url)
            .field("timeout", &self.timeout)
            .field("api_key", &"***")
            .finish()
    }
}

#[async_trait]
impl WebSearchProvider for SerperProvider {
    fn name(&self) -> &str {
        "serper"
    }

    async fn search(
        &self,
        query: &str,
        options: &WebSearchOptions,
    ) -> Result<Vec<WebSearchResult>, WebSearchError> {
        let limit = options.limit.map(|l| l.clamp(1, 100)).unwrap_or(10);
        let request = self
            .client
            .post(&self.base_url)
            .header("X-API-KEY", &self.api_key)
            .header("Content-Type", "application/json")
            .json(&json!({ "q": query, "num": limit }))
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
        parse_serper_response(&json, options.limit)
    }
}

fn parse_serper_response(
    json: &Value,
    requested_limit: Option<u32>,
) -> Result<Vec<WebSearchResult>, WebSearchError> {
    let mut results = Vec::new();
    if let Some(kg) = json.get("knowledgeGraph").and_then(|v| v.as_object()) {
        let title = kg
            .get("title")
            .and_then(|v| v.as_str())
            .map_or_else(String::new, String::from);
        let url = kg
            .get("link")
            .or_else(|| kg.get("url"))
            .and_then(|v| v.as_str())
            .map_or_else(String::new, String::from);
        let snippet = kg
            .get("snippet")
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
    let empty = Vec::new();
    let items = json
        .get("organic")
        .and_then(|v| v.as_array())
        .unwrap_or(&empty);
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
    async fn parses_serper_response_with_knowledge_graph_and_organic(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        let body = json!({
            "knowledgeGraph": { "title": "KG", "link": "https://kg.example", "snippet": "kg snippet" },
            "organic": [
                { "title": "T", "link": "https://t.example", "snippet": "S", "date": "2024-04-04" }
            ]
        });
        Mock::given(method("POST"))
            .and(path("/search"))
            .and(header("X-API-KEY", "key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&body))
            .mount(&server)
            .await;

        let mut config = default_config();
        config.options.insert(
            "base_url".to_string(),
            Value::String(format!("{}/search", server.uri())),
        );
        let provider = SerperFactory.create(config, reqwest::Client::new())?;
        let results = provider
            .search("rust", &WebSearchOptions { limit: Some(3), include_content: None, tool_call_id: None })
            .await?;
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "KG");
        assert_eq!(results[1].title, "T");
        assert_eq!(results[1].date.as_deref(), Some("2024-04-04"));
        Ok(())
    }

    fn default_config() -> WebSearchProviderConfig {
        WebSearchProviderConfig {
            provider: WebSearchProviderName::Serper,
            api_key: Some("key".to_string()),
            timeout_ms: None,
            options: std::collections::HashMap::new(),
        }
    }
}
