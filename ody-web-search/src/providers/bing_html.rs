use std::time::Duration;

use async_trait::async_trait;

use crate::config::WebSearchProviderConfig;
use crate::error::WebSearchError;
use crate::provider::{
    SharedWebSearchProvider, WebSearchOptions, WebSearchProvider, WebSearchProviderFactory,
    WebSearchResult,
};
use crate::providers::{take_string_option, validate_options};

const BING_SEARCH_URL: &str = "https://www.bing.com/search";
const USER_AGENT: &str = "ody-code";

pub struct BingHtmlFactory;

impl WebSearchProviderFactory for BingHtmlFactory {
    fn name(&self) -> &str {
        "bing-html"
    }

    fn create(
        &self,
        mut config: WebSearchProviderConfig,
        http_client: reqwest::Client,
    ) -> Result<SharedWebSearchProvider, WebSearchError> {
        validate_options(&config, &["proxy_url"])?;
        let proxy_url = take_string_option(&mut config.options, "proxy_url");
        let timeout = config
            .timeout_ms
            .map(Duration::from_millis)
            .unwrap_or_else(|| Duration::from_secs(25));
        Ok(std::sync::Arc::new(BingHtmlProvider {
            client: http_client,
            proxy_url,
            timeout,
        }))
    }
}

pub struct BingHtmlProvider {
    client: reqwest::Client,
    proxy_url: Option<String>,
    timeout: Duration,
}

impl std::fmt::Debug for BingHtmlProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("BingHtmlProvider")
            .field("proxy_url", &self.proxy_url)
            .field("timeout", &self.timeout)
            .finish()
    }
}

#[async_trait]
impl WebSearchProvider for BingHtmlProvider {
    fn name(&self) -> &str {
        "bing-html"
    }

    async fn search(
        &self,
        query: &str,
        options: &WebSearchOptions,
    ) -> Result<Vec<WebSearchResult>, WebSearchError> {
        let target_url = format!("{}?q={}", BING_SEARCH_URL, urlencoding::encode(query));
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
        let html = response
            .text()
            .await
            .map_err(|e| WebSearchError::from_reqwest(&e))?;
        let mut results = parse_bing_html(&html);
        if let Some(limit) = options.limit {
            let limit = limit.clamp(1, 50) as usize;
            results.truncate(limit);
        }
        Ok(results)
    }
}

fn parse_bing_html(html: &str) -> Vec<WebSearchResult> {
    let mut results = Vec::new();
    // Same marker as the odyBox TS implementation's `#b_results>li.b_algo` selector.
    let parts: Vec<&str> = html.split("<li class=\"b_algo\"").collect();
    for part in parts.iter().skip(1) {
        let Some((link, title)) = extract_title_link(part) else {
            continue;
        };
        let snippet = extract_snippet(part);
        if !title.is_empty() && !link.is_empty() {
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

fn extract_title_link(item: &str) -> Option<(String, String)> {
    let regex =
        regex::Regex::new(r#"<h2[^>]*>\s*<a[^>]*href="([^"]*)"[^>]*>(.*?)</a>\s*</h2>"#).ok()?;
    let captures = regex.captures(item)?;
    let link = captures.get(1)?.as_str().to_string();
    let title = strip_html_tags(captures.get(2)?.as_str());
    Some((link, title.trim().to_string()))
}

fn extract_snippet(item: &str) -> String {
    // Primary: `p[class^="b_lineclamp"]` (mirrors the TS selector).
    if let Some(snippet) =
        extract_first_group(item, r#"<p[^>]*class="b_lineclamp[^"]*"[^>]*>(.*?)</p>"#)
    {
        return snippet;
    }
    // Fallback: `.b_caption p`.
    if let Some(snippet) =
        extract_first_group(item, r#"<div class="b_caption"[^>]*>\s*<p[^>]*>(.*?)</p>"#)
    {
        return snippet;
    }
    String::new()
}

fn extract_first_group(html: &str, pattern: &str) -> Option<String> {
    let regex = regex::Regex::new(pattern).ok()?;
    let capture = regex.captures(html)?.get(1)?;
    Some(strip_html_tags(capture.as_str()).trim().to_string())
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
    use serde_json::Value;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const SERP_FIXTURE: &str = r#"<html><body><ol id="b_results">
        <li class="b_algo" data-id iid=SERP.1>
            <h2 class=""><a target="_blank" href="https://www.bing.com/ck/a?!&amp;p=abc&amp;u=a1https%3a%2f%2falpha.example%2fpage" h="ID=SERP,1"><strong>Alpha</strong> Project</a></h2>
            <div class="b_caption"><p class="b_lineclamp2" data-rslinkclamp-iid="">Alpha <b>snippet</b> text</p></div>
        </li>
        <li class="b_algo" data-id iid=SERP.2>
            <h2><a href="https://beta.example/post">Beta Site</a></h2>
            <div class="b_caption"><p>Plain caption beta</p></div>
        </li>
        <li class="b_algo" data-id iid=SERP.3>
            <div class="b_caption"><p>no title link, should be skipped</p></div>
        </li>
    </ol></body></html>"#;

    fn bing_html_config(proxy_url: String) -> WebSearchProviderConfig {
        let mut options = std::collections::HashMap::new();
        options.insert("proxy_url".to_string(), Value::String(proxy_url));
        WebSearchProviderConfig {
            provider: WebSearchProviderName::BingHtml,
            api_key: None,
            timeout_ms: None,
            options,
        }
    }

    #[tokio::test]
    async fn parses_bing_serp_html_results() -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .and(header(
                "X-Proxy-Url",
                "https://www.bing.com/search?q=hello%20world",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string(SERP_FIXTURE))
            .mount(&server)
            .await;

        let provider =
            BingHtmlFactory.create(bing_html_config(format!("{}/search", server.uri())), reqwest::Client::new())?;
        let results = provider
            .search(
                "hello world",
                &WebSearchOptions {
                    limit: Some(10),
                    include_content: None,
                    tool_call_id: None,
                },
            )
            .await?;

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Alpha Project");
        // Ported from odyBox bing.ts: the bing.com/ck/a redirect href is kept as-is.
        assert!(results[0].url.starts_with("https://www.bing.com/ck/a?"));
        assert_eq!(results[0].snippet, "Alpha snippet text");
        assert_eq!(results[1].title, "Beta Site");
        assert_eq!(results[1].url, "https://beta.example/post");
        assert_eq!(results[1].snippet, "Plain caption beta");
        Ok(())
    }

    #[tokio::test]
    async fn truncates_results_to_limit() -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(200).set_body_string(SERP_FIXTURE))
            .mount(&server)
            .await;

        let provider =
            BingHtmlFactory.create(bing_html_config(format!("{}/search", server.uri())), reqwest::Client::new())?;
        let results = provider
            .search(
                "hello world",
                &WebSearchOptions {
                    limit: Some(1),
                    include_content: None,
                    tool_call_id: None,
                },
            )
            .await?;

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Alpha Project");
        Ok(())
    }

    #[test]
    fn factory_creates_provider_without_api_key() {
        let config = WebSearchProviderConfig {
            provider: WebSearchProviderName::BingHtml,
            api_key: None,
            timeout_ms: None,
            options: std::collections::HashMap::new(),
        };
        assert!(BingHtmlFactory.create(config, reqwest::Client::new()).is_ok());
    }

    #[test]
    fn factory_rejects_unknown_options() {
        let mut options = std::collections::HashMap::new();
        options.insert("foo".to_string(), Value::String("bar".to_string()));
        let config = WebSearchProviderConfig {
            provider: WebSearchProviderName::BingHtml,
            api_key: None,
            timeout_ms: None,
            options,
        };
        assert!(BingHtmlFactory.create(config, reqwest::Client::new()).is_err());
    }
}
