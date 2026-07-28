use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;

use crate::config::WebSearchProviderConfig;
use crate::error::WebSearchError;
use crate::provider::{
    SharedWebSearchProvider, WebSearchOptions, WebSearchProvider, WebSearchProviderFactory,
    WebSearchResult,
};
use crate::providers::{require_api_key, take_base_url, take_string_option, validate_options};

const DEFAULT_SERPLY_URL: &str = "https://api.serply.io/v1/search/";

pub struct SerplyFactory;

impl WebSearchProviderFactory for SerplyFactory {
    fn name(&self) -> &str {
        "serply"
    }

    fn create(
        &self,
        mut config: WebSearchProviderConfig,
        http_client: reqwest::Client,
    ) -> Result<SharedWebSearchProvider, WebSearchError> {
        validate_options(&config, &["base_url", "language", "hl", "gl", "device"])?;
        let api_key = require_api_key(&config)?;
        let base_url = take_base_url(&mut config.options)
            .unwrap_or_else(|| DEFAULT_SERPLY_URL.to_string());
        let language = take_string_option(&mut config.options, "language").unwrap_or_else(|| "en".to_string());
        let hl = take_string_option(&mut config.options, "hl").unwrap_or_else(|| "en".to_string());
        let gl = take_string_option(&mut config.options, "gl")
            .unwrap_or_else(|| "US".to_string())
            .to_uppercase();
        let device = take_string_option(&mut config.options, "device").unwrap_or_else(|| "desktop".to_string());
        let timeout = config
            .timeout_ms
            .map(Duration::from_millis)
            .unwrap_or_else(|| Duration::from_secs(10));
        Ok(std::sync::Arc::new(SerplyProvider {
            client: http_client,
            api_key,
            base_url,
            language,
            hl,
            gl,
            device,
            timeout,
        }))
    }
}

pub struct SerplyProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    language: String,
    hl: String,
    gl: String,
    device: String,
    timeout: Duration,
}

impl std::fmt::Debug for SerplyProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SerplyProvider")
            .field("base_url", &self.base_url)
            .field("language", &self.language)
            .field("hl", &self.hl)
            .field("gl", &self.gl)
            .field("device", &self.device)
            .field("timeout", &self.timeout)
            .field("api_key", &"***")
            .finish()
    }
}

#[async_trait]
impl WebSearchProvider for SerplyProvider {
    fn name(&self) -> &str {
        "serply"
    }

    async fn search(
        &self,
        query: &str,
        options: &WebSearchOptions,
    ) -> Result<Vec<WebSearchResult>, WebSearchError> {
        let limit = options.limit.map(|l| l.clamp(1, 100)).unwrap_or(10);
        let request = self
            .client
            .get(&self.base_url)
            .query(&[
                ("q", query),
                ("language", &self.language),
                ("hl", &self.hl),
                ("gl", &self.gl),
                ("num", &limit.to_string()),
            ])
            .header("X-API-KEY", &self.api_key)
            .header("X-User-Agent", &self.device)
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
        parse_serply_response(&json, options.limit)
    }
}

fn parse_serply_response(
    json: &Value,
    requested_limit: Option<u32>,
) -> Result<Vec<WebSearchResult>, WebSearchError> {
    if json
        .get("message")
        .and_then(|v| v.as_str())
        .map(|m| m == "Unauthorized")
        .unwrap_or(false)
    {
        return Err(WebSearchError::Auth);
    }
    let items = json
        .get("results")
        .and_then(|v| v.as_array())
        .ok_or_else(|| WebSearchError::Unexpected {
            message: "serply response missing results".to_string(),
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
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn parses_serply_results() -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        let body = json!({
            "results": [
                { "title": "T", "link": "https://t.example", "snippet": "S", "date": "2024-05-05" }
            ]
        });
        Mock::given(method("GET"))
            .and(path("/v1/search/"))
            .and(query_param("q", "rust"))
            .and(query_param("gl", "US"))
            .and(query_param("num", "5"))
            .and(header("X-API-KEY", "key"))
            .and(header("X-User-Agent", "desktop"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&body))
            .mount(&server)
            .await;

        let mut options = std::collections::HashMap::new();
        options.insert(
            "base_url".to_string(),
            Value::String(format!("{}/v1/search/", server.uri())),
        );
        let config = WebSearchProviderConfig {
            provider: WebSearchProviderName::Serply,
            api_key: Some("key".to_string()),
            timeout_ms: None,
            options,
        };
        let provider = SerplyFactory.create(config, reqwest::Client::new())?;
        let results = provider
            .search("rust", &WebSearchOptions { limit: Some(5), include_content: None, tool_call_id: None })
            .await?;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "T");
        Ok(())
    }

    #[tokio::test]
    async fn returns_auth_error_for_unauthorized_message(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/search/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&json!({ "message": "Unauthorized" })))
            .mount(&server)
            .await;

        let mut options = std::collections::HashMap::new();
        options.insert(
            "base_url".to_string(),
            Value::String(format!("{}/v1/search/", server.uri())),
        );
        let config = WebSearchProviderConfig {
            provider: WebSearchProviderName::Serply,
            api_key: Some("key".to_string()),
            timeout_ms: None,
            options,
        };
        let provider = SerplyFactory.create(config, reqwest::Client::new())?;
        let result = provider.search("q", &WebSearchOptions::default()).await;
        assert_eq!(result, Err(WebSearchError::Auth));
        Ok(())
    }
}
