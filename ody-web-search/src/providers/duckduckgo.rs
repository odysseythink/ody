use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;

use crate::config::WebSearchProviderConfig;
use crate::error::WebSearchError;
use crate::provider::{
    SharedWebSearchProvider, WebSearchOptions, WebSearchProvider, WebSearchProviderFactory,
    WebSearchResult,
};
use crate::providers::validate_options;

const DUCKDUCKGO_HTML_URL: &str = "https://html.duckduckgo.com/html";
const USER_AGENT: &str = "ody-code";

pub struct DuckDuckGoFactory;

impl WebSearchProviderFactory for DuckDuckGoFactory {
    fn name(&self) -> &str {
        "duckduckgo"
    }

    fn create(
        &self,
        mut config: WebSearchProviderConfig,
        http_client: reqwest::Client,
    ) -> Result<SharedWebSearchProvider, WebSearchError> {
        validate_options(&config, &["proxy_url"])?;
        let proxy_url = take_proxy_url(&mut config.options);
        let timeout = config
            .timeout_ms
            .map(Duration::from_millis)
            .unwrap_or_else(|| Duration::from_secs(25));
        Ok(std::sync::Arc::new(DuckDuckGoProvider {
            client: http_client,
            proxy_url,
            timeout,
        }))
    }
}

pub struct DuckDuckGoProvider {
    client: reqwest::Client,
    proxy_url: Option<String>,
    timeout: Duration,
}

impl std::fmt::Debug for DuckDuckGoProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DuckDuckGoProvider")
            .field("proxy_url", &self.proxy_url)
            .field("timeout", &self.timeout)
            .finish()
    }
}

#[async_trait]
impl WebSearchProvider for DuckDuckGoProvider {
    fn name(&self) -> &str {
        "duckduckgo"
    }

    async fn search(
        &self,
        query: &str,
        options: &WebSearchOptions,
    ) -> Result<Vec<WebSearchResult>, WebSearchError> {
        let target_url = format!("{}?q={}", DUCKDUCKGO_HTML_URL, urlencoding::encode(query));
        let request = if let Some(proxy_url) = &self.proxy_url {
            self.client
                .get(proxy_url)
                .header("X-Proxy-Url", &target_url)
                .header("User-Agent", USER_AGENT)
                .timeout(self.timeout)
        } else {
            self.client
                .get(&target_url)
                .header("User-Agent", USER_AGENT)
                .timeout(self.timeout)
        };

        let response = request.send().await.map_err(|e| WebSearchError::from_reqwest(&e))?;
        let status = response.status();
        if !status.is_success() {
            let body = match response.text().await {
                Ok(body) => body,
                Err(_) => String::new(),
            };
            return Err(WebSearchError::from_http_status(status, &body));
        }
        let html = response.text().await.map_err(|e| WebSearchError::from_reqwest(&e))?;
        let mut results = parse_duckduckgo_html(&html);
        if let Some(limit) = options.limit {
            let limit = limit.clamp(1, 50) as usize;
            results.truncate(limit);
        }
        Ok(results)
    }
}

fn take_proxy_url(options: &mut std::collections::HashMap<String, Value>) -> Option<String> {
    options.remove("proxy_url").and_then(|v| match v {
        Value::String(s) => Some(s),
        _ => None,
    })
}

fn parse_duckduckgo_html(html: &str) -> Vec<WebSearchResult> {
    let mut results = Vec::new();
    // Split on the same class marker used by the TS implementation.
    let parts: Vec<&str> = html.split("<div class=\"result results_links").collect();
    for part in parts.iter().skip(1) {
        let title = extract_tag_contents(part, "result__a");
        let href = extract_tag_attribute(part, "result__a", "href");
        let link = href.map(extract_duckduckgo_redirect_url).unwrap_or_default();
        let snippet = extract_tag_contents(part, "result__snippet")
            .replace("<b>", "")
            .replace("</b>", "");
        let snippet = strip_html_tags(&snippet);
        if !title.is_empty() && !link.is_empty() && !snippet.is_empty() {
            results.push(WebSearchResult {
                title,
                url: link,
                snippet,
                date: None,
                content: None,
            });
        }
    }
    results
}

fn extract_tag_contents(html: &str, class_name: &str) -> String {
    let pattern = format!("<a[^>]*class=\"{}\"[^>]*>(.*?)</a>", class_name);
    let Ok(regex) = regex::Regex::new(&pattern) else {
        return String::new();
    };
    let Some(capture) = regex.captures(html).and_then(|c| c.get(1)) else {
        return String::new();
    };
    strip_html_tags(capture.as_str())
}

fn extract_tag_attribute(html: &str, class_name: &str, attribute: &str) -> Option<String> {
    let pattern = format!("<a[^>]*class=\"{}\"[^>]*{}=\"([^\"]*)\"", class_name, attribute);
    let regex = regex::Regex::new(&pattern).ok()?;
    let capture = regex.captures(html)?.get(1)?;
    Some(capture.as_str().to_string())
}

fn extract_duckduckgo_redirect_url(href: String) -> String {
    let mut normalized = href;
    if normalized.starts_with("//") {
        normalized = format!("https:{}", normalized);
    }
    if let Ok(url) = url::Url::parse(&normalized) {
        if let Some((_, actual)) = url.query_pairs().find(|(k, _)| k == "uddg") {
            let decoded = urlencoding::decode(&actual).unwrap_or_else(|_| actual.clone());
            return decoded.into_owned();
        }
    }
    normalized
}

fn strip_html_tags(html: &str) -> String {
    let regex = regex::Regex::new("<[^>]+>").expect("valid regex");
    regex.replace_all(html, "").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{WebSearchProviderConfig, WebSearchProviderName};
    use crate::provider::WebSearchOptions;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn parses_duckduckgo_html_results() -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        let html = r#"<html>
            <div class="result results_links results_links_deep web-result">
                <a class="result__a" href="//duckduckgo.com/lkdjs/?uddg=https%3A%2F%2Fa.example">Title A</a>
                <a class="result__snippet"><b>snippet</b> A</a>
            </div>
            <div class="result results_links results_links_deep web-result">
                <a class="result__a" href="https://b.example">Title B</a>
                <a class="result__snippet">snippet B</a>
            </div>
        </html>"#;
        Mock::given(method("GET"))
            .and(path("/html"))
            .and(header("User-Agent", USER_AGENT))
            .respond_with(ResponseTemplate::new(200).set_body_string(html))
            .mount(&server)
            .await;

        let mut options = std::collections::HashMap::new();
        options.insert(
            "proxy_url".to_string(),
            Value::String(format!("{}/html", server.uri())),
        );
        let config = WebSearchProviderConfig {
            provider: WebSearchProviderName::Duckduckgo,
            api_key: None,
            timeout_ms: None,
            options,
        };
        let provider = DuckDuckGoFactory.create(config, reqwest::Client::new())?;
        let results = provider
            .search("hello world", &WebSearchOptions { limit: Some(2), include_content: None, tool_call_id: None })
            .await?;
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Title A");
        assert_eq!(results[0].url, "https://a.example");
        assert_eq!(results[0].snippet, "snippet A");
        assert_eq!(results[1].title, "Title B");
        assert_eq!(results[1].url, "https://b.example");
        Ok(())
    }

    #[tokio::test]
    async fn uses_proxy_url_when_configured() -> Result<(), Box<dyn std::error::Error>> {
        let proxy_server = MockServer::start().await;
        let html = r#"<div class="result results_links">
            <a class="result__a" href="https://c.example">Title C</a>
            <a class="result__snippet">snippet C</a>
        </div>"#;
        Mock::given(method("GET"))
            .and(path("/proxy"))
            .and(header("X-Proxy-Url", "https://html.duckduckgo.com/html?q=query"))
            .respond_with(ResponseTemplate::new(200).set_body_string(html))
            .mount(&proxy_server)
            .await;
        let mut options = std::collections::HashMap::new();
        options.insert(
            "proxy_url".to_string(),
            Value::String(format!("{}/proxy", proxy_server.uri())),
        );
        let config = WebSearchProviderConfig {
            provider: WebSearchProviderName::Duckduckgo,
            api_key: None,
            timeout_ms: None,
            options,
        };
        let provider = DuckDuckGoFactory.create(config, reqwest::Client::new())?;
        let results = provider.search("query", &WebSearchOptions::default()).await?;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Title C");
        Ok(())
    }
}
