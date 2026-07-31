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

const DEFAULT_BAIDU_URL: &str = "https://qianfan.baidubce.com/v2/ai_search/web_search";

pub struct BaiduFactory;

impl WebSearchProviderFactory for BaiduFactory {
    fn name(&self) -> &str {
        "baidu"
    }

    fn create(
        &self,
        mut config: WebSearchProviderConfig,
        http_client: reqwest::Client,
    ) -> Result<SharedWebSearchProvider, WebSearchError> {
        validate_options(&config, &["base_url", "top_k"])?;
        let api_key = require_api_key(&config)?;
        let base_url =
            take_base_url(&mut config.options).unwrap_or_else(|| DEFAULT_BAIDU_URL.to_string());
        let top_k = take_top_k(&mut config.options).unwrap_or(10);
        let timeout = config
            .timeout_ms
            .map(Duration::from_millis)
            .unwrap_or_else(|| Duration::from_secs(10));
        Ok(std::sync::Arc::new(BaiduProvider {
            client: http_client,
            api_key,
            base_url,
            top_k,
            timeout,
        }))
    }
}

pub struct BaiduProvider {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    top_k: u32,
    timeout: Duration,
}

impl std::fmt::Debug for BaiduProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BaiduProvider")
            .field("base_url", &self.base_url)
            .field("top_k", &self.top_k)
            .field("timeout", &self.timeout)
            .field("api_key", &"***")
            .finish()
    }
}

#[async_trait]
impl WebSearchProvider for BaiduProvider {
    fn name(&self) -> &str {
        "baidu"
    }

    async fn search(
        &self,
        query: &str,
        _options: &WebSearchOptions,
    ) -> Result<Vec<WebSearchResult>, WebSearchError> {
        let body = json!({
            "messages": [{ "role": "user", "content": query }],
            "resource_type_filter": [{ "type": "web", "top_k": self.top_k }],
        });
        let request = self
            .client
            .post(&self.base_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header(
                "X-Appbuilder-Authorization",
                format!("Bearer {}", self.api_key),
            )
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
        parse_baidu_response(&json)
    }
}

fn take_top_k(options: &mut std::collections::HashMap<String, Value>) -> Option<u32> {
    options.remove("top_k").and_then(|v| match v {
        Value::Number(n) => n.as_u64().map(|u| u.clamp(1, 50) as u32),
        _ => None,
    })
}

fn parse_baidu_response(json: &Value) -> Result<Vec<WebSearchResult>, WebSearchError> {
    if json.get("code").is_some()
        || (json.get("message").is_some() && json.get("references").is_none())
    {
        let message = json
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown baidu error");
        return Err(WebSearchError::Unexpected {
            message: format!("baidu search error: {}", message),
        });
    }
    let empty = Vec::new();
    let refs = json
        .get("references")
        .and_then(|v| v.as_array())
        .unwrap_or(&empty);
    let mut results = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for r in refs {
        let type_ = r
            .get("type")
            .and_then(|v| v.as_str())
            .or_else(|| r.get("resource_type").and_then(|v| v.as_str()))
            .unwrap_or("web")
            .to_lowercase();
        if type_ != "web" {
            continue;
        }
        let title = r
            .get("title")
            .and_then(|v| v.as_str())
            .or_else(|| r.get("web_anchor").and_then(|v| v.as_str()))
            .unwrap_or("")
            .trim()
            .to_string();
        let url = r
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let snippet = r
            .get("snippet")
            .and_then(|v| v.as_str())
            .or_else(|| r.get("content").and_then(|v| v.as_str()))
            .unwrap_or("")
            .trim()
            .to_string();
        if title.is_empty() || url.is_empty() || seen.contains(&url) {
            continue;
        }
        seen.insert(url.clone());
        results.push(WebSearchResult {
            title,
            url,
            snippet,
            date: None,
            content: None,
        });
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
    async fn parses_baidu_references() -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        let body = json!({
            "references": [
                { "type": "web", "title": "A", "url": "https://a.example", "snippet": "SA" },
                { "type": "doc", "title": "B", "url": "https://b.example", "snippet": "SB" },
                { "type": "web", "title": "C", "url": "https://c.example", "snippet": "SC" }
            ]
        });
        Mock::given(method("POST"))
            .and(path("/v2/ai_search/web_search"))
            .and(header("Authorization", "Bearer key"))
            .and(header("X-Appbuilder-Authorization", "Bearer key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&body))
            .mount(&server)
            .await;

        let mut options = std::collections::HashMap::new();
        options.insert(
            "base_url".to_string(),
            Value::String(format!("{}/v2/ai_search/web_search", server.uri())),
        );
        let config = WebSearchProviderConfig {
            provider: WebSearchProviderName::Baidu,
            api_key: Some("key".to_string()),
            timeout_ms: None,
            options,
        };
        let provider = BaiduFactory.create(config, reqwest::Client::new())?;
        let results = provider.search("q", &WebSearchOptions::default()).await?;
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "A");
        assert_eq!(results[1].title, "C");
        Ok(())
    }

    #[tokio::test]
    async fn returns_error_for_baidu_api_error() -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        let body = json!({ "code": "ERR", "message": "bad request" });
        Mock::given(method("POST"))
            .and(path("/v2/ai_search/web_search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&body))
            .mount(&server)
            .await;

        let mut options = std::collections::HashMap::new();
        options.insert(
            "base_url".to_string(),
            Value::String(format!("{}/v2/ai_search/web_search", server.uri())),
        );
        let config = WebSearchProviderConfig {
            provider: WebSearchProviderName::Baidu,
            api_key: Some("key".to_string()),
            timeout_ms: None,
            options,
        };
        let provider = BaiduFactory.create(config, reqwest::Client::new())?;
        let result = provider.search("q", &WebSearchOptions::default()).await;
        assert!(matches!(result, Err(WebSearchError::Unexpected { .. })));
        Ok(())
    }
}
