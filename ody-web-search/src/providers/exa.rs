use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::config::WebSearchProviderConfig;
use crate::error::WebSearchError;
use crate::provider::{
    SharedWebSearchProvider, WebSearchOptions, WebSearchProvider, WebSearchProviderFactory,
    WebSearchResult,
};
use crate::providers::{require_api_key, take_base_url, take_string_option, validate_options};

const DEFAULT_EXA_URL: &str = "https://api.exa.ai/search";

pub struct ExaFactory;

impl WebSearchProviderFactory for ExaFactory {
    fn name(&self) -> &str {
        "exa"
    }

    fn create(
        &self,
        mut config: WebSearchProviderConfig,
        http_client: reqwest::Client,
    ) -> Result<SharedWebSearchProvider, WebSearchError> {
        validate_options(&config, &["base_url", "type", "livecrawl"])?;
        let api_key = require_api_key(&config)?;
        let base_url = take_base_url(&mut config.options)
            .unwrap_or_else(|| DEFAULT_EXA_URL.to_string());
        let type_ = take_string_option(&mut config.options, "type").unwrap_or_else(|| "auto".to_string());
        let livecrawl = take_string_option(&mut config.options, "livecrawl")
            .unwrap_or_else(|| "fallback".to_string());
        let timeout = config
            .timeout_ms
            .map(Duration::from_millis)
            .unwrap_or_else(|| Duration::from_secs(10));
        Ok(std::sync::Arc::new(ExaProvider {
            client: http_client,
            api_key,
            base_url,
            type_,
            livecrawl,
            timeout,
        }))
    }
}

pub struct ExaProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    type_: String,
    livecrawl: String,
    timeout: Duration,
}

impl std::fmt::Debug for ExaProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExaProvider")
            .field("base_url", &self.base_url)
            .field("type", &self.type_)
            .field("livecrawl", &self.livecrawl)
            .field("timeout", &self.timeout)
            .field("api_key", &"***")
            .finish()
    }
}

#[async_trait]
impl WebSearchProvider for ExaProvider {
    fn name(&self) -> &str {
        "exa"
    }

    async fn search(
        &self,
        query: &str,
        options: &WebSearchOptions,
    ) -> Result<Vec<WebSearchResult>, WebSearchError> {
        let limit = options.limit.map(|l| l.clamp(1, 100)).unwrap_or(10);
        let include_content = options.include_content.unwrap_or(false);
        let body = json!({
            "query": query,
            "type": self.type_,
            "numResults": limit,
            "contents": { "text": include_content },
            "livecrawl": self.livecrawl,
        });
        let request = self
            .client
            .post(&self.base_url)
            .header("x-api-key", &self.api_key)
            .header("Content-Type", "application/json")
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
        parse_exa_response(&json, options.include_content)
    }
}

fn parse_exa_response(
    json: &Value,
    include_content: Option<bool>,
) -> Result<Vec<WebSearchResult>, WebSearchError> {
    let items = json
        .get("results")
        .and_then(|v| v.as_array())
        .ok_or_else(|| WebSearchError::Unexpected {
            message: "exa response missing results".to_string(),
        })?;
    let mut results = Vec::new();
    for item in items {
        let title = item
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let url = item
            .get("url")
            .and_then(|v| v.as_str())
            .map_or_else(String::new, String::from);
        let text = item
            .get("text")
            .and_then(|v| v.as_str())
            .map_or_else(String::new, String::from);
        let date = item
            .get("publishedDate")
            .and_then(|v| v.as_str())
            .map(String::from);
        let content = if include_content.unwrap_or(false) {
            item.get("text")
                .and_then(|v| v.as_str())
                .map(String::from)
        } else {
            None
        };
        if !url.is_empty() {
            results.push(WebSearchResult {
                title,
                url,
                snippet: text.clone(),
                date,
                content,
            });
        }
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
    async fn parses_exa_results() -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        let body = json!({
            "results": [
                { "title": "T", "url": "https://t.example", "text": "C", "publishedDate": "2024-07-07" }
            ]
        });
        Mock::given(method("POST"))
            .and(path("/search"))
            .and(header("x-api-key", "key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&body))
            .mount(&server)
            .await;

        let mut options = std::collections::HashMap::new();
        options.insert(
            "base_url".to_string(),
            Value::String(format!("{}/search", server.uri())),
        );
        let config = WebSearchProviderConfig {
            provider: WebSearchProviderName::Exa,
            api_key: Some("key".to_string()),
            timeout_ms: None,
            options,
        };
        let provider = ExaFactory.create(config, reqwest::Client::new())?;
        let results = provider
            .search(
                "rust",
                &WebSearchOptions {
                    limit: Some(3),
                    include_content: Some(true),
                    tool_call_id: None,
                },
            )
            .await?;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "T");
        assert_eq!(results[0].content.as_deref(), Some("C"));
        Ok(())
    }
}
