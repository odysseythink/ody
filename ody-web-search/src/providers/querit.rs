use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::config::WebSearchProviderConfig;
use crate::error::WebSearchError;
use crate::provider::{
    SharedWebSearchProvider, WebSearchOptions, WebSearchProvider, WebSearchProviderFactory,
    WebSearchResult,
};
use crate::providers::{
    require_api_key, take_base_url, take_string_option, take_u32_option, validate_options,
};

const DEFAULT_QUERIT_URL: &str = "https://api.querit.ai/v1/search";

pub struct QueritFactory;

impl WebSearchProviderFactory for QueritFactory {
    fn name(&self) -> &str {
        "querit"
    }

    fn create(
        &self,
        mut config: WebSearchProviderConfig,
        http_client: reqwest::Client,
    ) -> Result<SharedWebSearchProvider, WebSearchError> {
        validate_options(&config, &["base_url", "max_results", "time_range"])?;
        let api_key = require_api_key(&config)?;
        let base_url =
            take_base_url(&mut config.options).unwrap_or_else(|| DEFAULT_QUERIT_URL.to_string());
        let max_results = take_u32_option(&mut config.options, "max_results", 5, 1, 50);
        // odyBox queritTimeRange values: none/d1/w1/m1/y1. "none" disables the filter.
        let time_range = take_string_option(&mut config.options, "time_range")
            .filter(|v| v != "none");
        let timeout = config
            .timeout_ms
            .map(Duration::from_millis)
            .unwrap_or_else(|| Duration::from_secs(10));
        Ok(std::sync::Arc::new(QueritProvider {
            client: http_client,
            api_key,
            base_url,
            max_results,
            time_range,
            timeout,
        }))
    }
}

pub struct QueritProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    max_results: u32,
    time_range: Option<String>,
    timeout: Duration,
}

impl std::fmt::Debug for QueritProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("QueritProvider")
            .field("base_url", &self.base_url)
            .field("max_results", &self.max_results)
            .field("time_range", &self.time_range)
            .field("timeout", &self.timeout)
            .field("api_key", &"***")
            .finish()
    }
}

#[async_trait]
impl WebSearchProvider for QueritProvider {
    fn name(&self) -> &str {
        "querit"
    }

    async fn search(
        &self,
        query: &str,
        options: &WebSearchOptions,
    ) -> Result<Vec<WebSearchResult>, WebSearchError> {
        // Runtime tool limit wins over the stored max_results preset.
        let count = options
            .limit
            .map(|l| l.clamp(1, 50))
            .unwrap_or(self.max_results);
        let mut body = json!({
            "query": query,
            "count": count,
        });
        if let Some(time_range) = &self.time_range {
            body["filters"] = json!({ "timeRange": { "date": time_range } });
        }

        let response = self
            .client
            .post(&self.base_url)
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .timeout(self.timeout)
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
        parse_querit_response(&json, options.limit)
    }
}

fn parse_querit_response(
    json: &Value,
    requested_limit: Option<u32>,
) -> Result<Vec<WebSearchResult>, WebSearchError> {
    if json.get("error_code").and_then(|v| v.as_i64()) != Some(200) {
        let code = json.get("error_code").cloned().unwrap_or(Value::Null);
        let detail = json
            .get("error")
            .map(|v| v.to_string())
            .unwrap_or_default();
        // Deviation from odyBox (which returns []): a business error such as an
        // invalid key must not silently look like "no results found".
        return Err(WebSearchError::Unexpected {
            message: format!("querit api error: error_code={} {}", code, detail),
        });
    }
    let items = json
        .pointer("/results/result")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut results = Vec::new();
    for item in &items {
        let title = item
            .get("title")
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
    use serde_json::{Value, json};
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn querit_config(options: std::collections::HashMap<String, Value>) -> WebSearchProviderConfig {
        WebSearchProviderConfig {
            provider: WebSearchProviderName::Querit,
            api_key: Some("querit-key".to_string()),
            timeout_ms: None,
            options,
        }
    }

    fn ok_payload() -> serde_json::Value {
        json!({
            "error_code": 200,
            "results": { "result": [
                { "title": "One", "url": "https://one.example", "snippet": "S1" },
                { "title": "Two", "url": "https://two.example", "snippet": "S2" }
            ] }
        })
    }

    #[tokio::test]
    async fn parses_querit_results() -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/search"))
            .and(body_json(json!({ "query": "rust", "count": 5 })))
            .respond_with(ResponseTemplate::new(200).set_body_json(ok_payload()))
            .mount(&server)
            .await;

        let mut options = std::collections::HashMap::new();
        options.insert(
            "base_url".to_string(),
            Value::String(format!("{}/v1/search", server.uri())),
        );
        let provider = QueritFactory.create(querit_config(options), reqwest::Client::new())?;
        let results = provider
            .search("rust", &WebSearchOptions::default())
            .await?;

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "One");
        assert_eq!(results[0].url, "https://one.example");
        assert_eq!(results[0].snippet, "S1");
        Ok(())
    }

    #[tokio::test]
    async fn sends_max_results_and_time_range_filters(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/search"))
            .and(body_json(json!({
                "query": "rust",
                "count": 8,
                "filters": { "timeRange": { "date": "w1" } }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(ok_payload()))
            .mount(&server)
            .await;

        let mut options = std::collections::HashMap::new();
        options.insert(
            "base_url".to_string(),
            Value::String(format!("{}/v1/search", server.uri())),
        );
        options.insert("max_results".to_string(), json!(8));
        options.insert("time_range".to_string(), Value::String("w1".to_string()));
        let provider = QueritFactory.create(querit_config(options), reqwest::Client::new())?;
        let results = provider
            .search("rust", &WebSearchOptions::default())
            .await?;

        assert_eq!(results.len(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn none_time_range_omits_filters() -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/search"))
            .and(body_json(json!({ "query": "rust", "count": 5 })))
            .respond_with(ResponseTemplate::new(200).set_body_json(ok_payload()))
            .mount(&server)
            .await;

        let mut options = std::collections::HashMap::new();
        options.insert(
            "base_url".to_string(),
            Value::String(format!("{}/v1/search", server.uri())),
        );
        options.insert(
            "time_range".to_string(),
            Value::String("none".to_string()),
        );
        let provider = QueritFactory.create(querit_config(options), reqwest::Client::new())?;
        let results = provider
            .search("rust", &WebSearchOptions::default())
            .await?;

        assert_eq!(results.len(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn tool_limit_overrides_max_results(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/search"))
            .and(body_json(json!({ "query": "rust", "count": 3 })))
            .respond_with(ResponseTemplate::new(200).set_body_json(ok_payload()))
            .mount(&server)
            .await;

        let mut options = std::collections::HashMap::new();
        options.insert(
            "base_url".to_string(),
            Value::String(format!("{}/v1/search", server.uri())),
        );
        options.insert("max_results".to_string(), json!(8));
        let provider = QueritFactory.create(querit_config(options), reqwest::Client::new())?;
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

        assert_eq!(results.len(), 2);
        Ok(())
    }

    #[test]
    fn business_error_code_returns_error() {
        // odyBox returns [] here; the runtime port surfaces it as an error so a
        // bad key never silently yields "no results".
        let json = json!({ "error_code": 401, "error": "invalid key" });
        let err = parse_querit_response(&json, None).expect_err("expected error");
        assert!(err.to_string().contains("401"), "unexpected error: {}", err);
    }

    #[test]
    fn factory_requires_api_key() {
        let config = WebSearchProviderConfig {
            provider: WebSearchProviderName::Querit,
            api_key: None,
            timeout_ms: None,
            options: std::collections::HashMap::new(),
        };
        assert!(QueritFactory.create(config, reqwest::Client::new()).is_err());
    }

    #[test]
    fn factory_rejects_unknown_options() {
        let mut options = std::collections::HashMap::new();
        options.insert("foo".to_string(), Value::String("bar".to_string()));
        let config = WebSearchProviderConfig {
            provider: WebSearchProviderName::Querit,
            api_key: Some("k".to_string()),
            timeout_ms: None,
            options,
        };
        assert!(QueritFactory.create(config, reqwest::Client::new()).is_err());
    }
}
