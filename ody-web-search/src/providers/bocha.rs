use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::config::WebSearchProviderConfig;
use crate::error::WebSearchError;
use crate::provider::{
    SharedWebSearchProvider, WebSearchOptions, WebSearchProvider, WebSearchProviderFactory,
    WebSearchResult,
};
use crate::providers::{require_api_key, take_base_url, validate_options};

const DEFAULT_BOCHA_URL: &str = "https://api.bocha.cn/v1/web-search";
const LEGACY_BOCHA_URL: &str = "https://api.bochaai.com/v1/web-search";

pub struct BochaFactory;

impl WebSearchProviderFactory for BochaFactory {
    fn name(&self) -> &str {
        "bocha"
    }

    fn create(
        &self,
        mut config: WebSearchProviderConfig,
        http_client: reqwest::Client,
    ) -> Result<SharedWebSearchProvider, WebSearchError> {
        validate_options(&config, &["base_url"])?;
        let api_key = require_api_key(&config)?;
        let base_url =
            take_base_url(&mut config.options).unwrap_or_else(|| DEFAULT_BOCHA_URL.to_string());
        let timeout = config
            .timeout_ms
            .map(Duration::from_millis)
            .unwrap_or_else(|| Duration::from_secs(10));
        Ok(std::sync::Arc::new(BochaProvider {
            client: http_client,
            api_key,
            base_url,
            timeout,
        }))
    }
}

pub struct BochaProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    timeout: Duration,
}

impl std::fmt::Debug for BochaProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("BochaProvider")
            .field("base_url", &self.base_url)
            .field("timeout", &self.timeout)
            .field("api_key", &"***")
            .finish()
    }
}

#[async_trait]
impl WebSearchProvider for BochaProvider {
    fn name(&self) -> &str {
        "bocha"
    }

    async fn search(
        &self,
        query: &str,
        options: &WebSearchOptions,
    ) -> Result<Vec<WebSearchResult>, WebSearchError> {
        let limit = options.limit.map(|l| l.clamp(1, 50)).unwrap_or(10);
        let body = json!({
            "query": query,
            "freshness": "noLimit",
            "summary": true,
            "count": limit,
        });

        // Same dual-endpoint fallback as the odyBox TS implementation.
        let mut last_error: Option<WebSearchError> = None;
        for endpoint in [&self.base_url, LEGACY_BOCHA_URL] {
            match self.query_endpoint(endpoint, &body).await {
                Ok(json) => match parse_bocha_response(&json) {
                    Ok(results) => return Ok(results),
                    Err(e) => last_error = Some(e),
                },
                Err(e) => last_error = Some(e),
            }
        }
        Err(last_error.unwrap_or_else(|| WebSearchError::Unexpected {
            message: "bocha request failed on all endpoints".to_string(),
        }))
    }
}

impl BochaProvider {
    async fn query_endpoint(
        &self,
        endpoint: &str,
        body: &Value,
    ) -> Result<Value, WebSearchError> {
        let response = self
            .client
            .post(endpoint)
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(body)
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
        response
            .json()
            .await
            .map_err(|e| WebSearchError::from_reqwest(&e))
    }
}

fn parse_bocha_response(json: &Value) -> Result<Vec<WebSearchResult>, WebSearchError> {
    // Ported from native-web-search.ts: a non-200 (or non-numeric) `code`
    // marks the payload as an error even on HTTP 200.
    if let Some(code) = json.get("code") {
        let valid = match code {
            Value::Number(n) => n.as_f64() == Some(200.0),
            Value::String(s) => s == "200",
            _ => false,
        };
        if !valid {
            let message = json
                .get("msg")
                .or_else(|| json.get("message"))
                .and_then(|v| v.as_str())
                .unwrap_or("bocha api error");
            return Err(WebSearchError::Unexpected {
                message: format!("bocha api error (code {:?}): {}", code, message),
            });
        }
    }

    let value = json
        .pointer("/data/webPages/value")
        .or_else(|| json.pointer("/webPages/value"))
        .and_then(|v| v.as_array())
        .ok_or_else(|| WebSearchError::Unexpected {
            message: "bocha response missing webPages.value array".to_string(),
        })?;

    let mut results = Vec::new();
    for item in value {
        let title = item
            .get("name")
            .and_then(|v| v.as_str())
            .map_or_else(String::new, String::from);
        let url = item
            .get("url")
            .and_then(|v| v.as_str())
            .map_or_else(String::new, String::from);
        let snippet = item
            .get("summary")
            .or_else(|| item.get("snippet"))
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

    fn bocha_config(base_url: String) -> WebSearchProviderConfig {
        let mut options = std::collections::HashMap::new();
        options.insert("base_url".to_string(), Value::String(base_url));
        WebSearchProviderConfig {
            provider: WebSearchProviderName::Bocha,
            api_key: Some("bocha-key".to_string()),
            timeout_ms: None,
            options,
        }
    }

    #[tokio::test]
    async fn parses_bocha_results() -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        let body = json!({
            "code": 200,
            "data": { "webPages": { "value": [
                { "name": "Result One", "url": "https://one.example", "summary": "Summary one" },
                { "name": "Result Two", "url": "https://two.example", "snippet": "Snippet two" }
            ] } }
        });
        Mock::given(method("POST"))
            .and(path("/v1/web-search"))
            .and(header("Authorization", "Bearer bocha-key"))
            .and(body_json(json!({
                "query": "rust",
                "freshness": "noLimit",
                "summary": true,
                "count": 10
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(&body))
            .mount(&server)
            .await;

        let provider = BochaFactory.create(
            bocha_config(format!("{}/v1/web-search", server.uri())),
            reqwest::Client::new(),
        )?;
        let results = provider
            .search(
                "rust",
                &WebSearchOptions {
                    limit: Some(10),
                    include_content: None,
                    tool_call_id: None,
                },
            )
            .await?;

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Result One");
        assert_eq!(results[0].url, "https://one.example");
        assert_eq!(results[0].snippet, "Summary one");
        // summary falls back to snippet
        assert_eq!(results[1].snippet, "Snippet two");
        Ok(())
    }

    #[tokio::test]
    async fn supports_top_level_webpages_value() -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        let body = json!({
            "code": 200,
            "webPages": { "value": [
                { "name": "Top", "url": "https://top.example", "summary": "S" }
            ] }
        });
        Mock::given(method("POST"))
            .and(path("/v1/web-search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&body))
            .mount(&server)
            .await;

        let provider = BochaFactory.create(
            bocha_config(format!("{}/v1/web-search", server.uri())),
            reqwest::Client::new(),
        )?;
        let results = provider
            .search("rust", &WebSearchOptions::default())
            .await?;

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Top");
        Ok(())
    }

    #[test]
    fn non_success_code_returns_error_with_msg() {
        let json = json!({ "code": 429, "msg": "rate limited" });
        let err = parse_bocha_response(&json).expect_err("expected error");
        assert!(
            err.to_string().contains("rate limited"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn malformed_payload_returns_error() {
        let json = json!({ "code": 200 });
        let err = parse_bocha_response(&json).expect_err("expected error");
        assert!(
            err.to_string().contains("webPages"),
            "unexpected error: {}",
            err
        );
    }

    #[test]
    fn factory_requires_api_key() {
        let config = WebSearchProviderConfig {
            provider: WebSearchProviderName::Bocha,
            api_key: None,
            timeout_ms: None,
            options: std::collections::HashMap::new(),
        };
        assert!(BochaFactory.create(config, reqwest::Client::new()).is_err());
    }

    #[test]
    fn factory_rejects_unknown_options() {
        let mut options = std::collections::HashMap::new();
        options.insert("foo".to_string(), Value::String("bar".to_string()));
        let config = WebSearchProviderConfig {
            provider: WebSearchProviderName::Bocha,
            api_key: Some("k".to_string()),
            timeout_ms: None,
            options,
        };
        assert!(BochaFactory.create(config, reqwest::Client::new()).is_err());
    }
}
