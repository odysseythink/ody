use std::time::Duration;

use async_trait::async_trait;

use crate::config::WebSearchProviderConfig;
use crate::error::WebSearchError;
use crate::provider::{
    SharedWebSearchProvider, WebSearchOptions, WebSearchProvider, WebSearchProviderFactory,
    WebSearchResult,
};
use crate::providers::{take_string_option, validate_options};

const BING_NEWS_URL: &str = "https://www.bing.com/news/infinitescrollajax";
const USER_AGENT: &str = "ody-code";

pub struct BingNewsFactory;

impl WebSearchProviderFactory for BingNewsFactory {
    fn name(&self) -> &str {
        "bing-news"
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
        Ok(std::sync::Arc::new(BingNewsProvider {
            client: http_client,
            proxy_url,
            timeout,
        }))
    }
}

pub struct BingNewsProvider {
    client: reqwest::Client,
    proxy_url: Option<String>,
    timeout: Duration,
}

impl std::fmt::Debug for BingNewsProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("BingNewsProvider")
            .field("proxy_url", &self.proxy_url)
            .field("timeout", &self.timeout)
            .finish()
    }
}

#[async_trait]
impl WebSearchProvider for BingNewsProvider {
    fn name(&self) -> &str {
        "bing-news"
    }

    async fn search(
        &self,
        query: &str,
        options: &WebSearchOptions,
    ) -> Result<Vec<WebSearchResult>, WebSearchError> {
        let target_url = format!(
            "{}?InfiniteScroll=1&q={}",
            BING_NEWS_URL,
            urlencoding::encode(query)
        );
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
        let mut results = parse_bing_news_html(&html);
        if let Some(limit) = options.limit {
            let limit = limit.clamp(1, 50) as usize;
            results.truncate(limit);
        }
        Ok(results)
    }
}

fn parse_bing_news_html(html: &str) -> Vec<WebSearchResult> {
    let mut results = Vec::new();
    // Same node as the odyBox TS implementation's `.newsitem` selector
    // (Bing renders `<div class="news-card newsitem cardcommon" ...>`).
    let parts: Vec<&str> = html.split("<div class=\"news-card newsitem").collect();
    for part in parts.iter().skip(1) {
        let link = match extract_first_group(part, r#"<a[^>]*class="title"[^>]*href="([^"]*)""#)
        {
            Some(link) => link,
            None => continue,
        };
        let title = extract_first_group(part, r#"<h2[^>]*>(.*?)</h2>"#).unwrap_or_default();
        let snippet =
            extract_first_group(part, r#"<div class="snippet"[^>]*>(.*?)</div>"#)
                .unwrap_or_default();
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

    const NEWS_FIXTURE: &str = r#"<html><body>
        <div class="news-card newsitem cardcommon" url="https://news.example/1" data-title="News One">
            <div class="news-card-body">
                <a target="_blank" class="title" href="https://news.example/1" h="ID=news,1"><h2 class=" ns_hd_h2">News <strong>One</strong> Title</h2></a>
                <div class="snippet" title="src one">Snippet one text</div>
            </div>
        </div>
        <div class="news-card newsitem cardcommon" url="https://news.example/2" data-title="News Two">
            <div class="news-card-body">
                <a class="title" href="https://news.example/2"><h2>Second <strong>News</strong></h2></a>
                <div class="snippet">Snippet two text</div>
            </div>
        </div>
    </body></html>"#;

    fn bing_news_config(proxy_url: String) -> WebSearchProviderConfig {
        let mut options = std::collections::HashMap::new();
        options.insert("proxy_url".to_string(), Value::String(proxy_url));
        WebSearchProviderConfig {
            provider: WebSearchProviderName::BingNews,
            api_key: None,
            timeout_ms: None,
            options,
        }
    }

    #[tokio::test]
    async fn parses_bing_news_html_results() -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/news/infinitescrollajax"))
            .and(header(
                "X-Proxy-Url",
                "https://www.bing.com/news/infinitescrollajax?InfiniteScroll=1&q=hello%20world",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_string(NEWS_FIXTURE))
            .mount(&server)
            .await;

        let provider =
            BingNewsFactory.create(bing_news_config(format!("{}/news/infinitescrollajax", server.uri())), reqwest::Client::new())?;
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
        assert_eq!(results[0].title, "News One Title");
        assert_eq!(results[0].url, "https://news.example/1");
        assert_eq!(results[0].snippet, "Snippet one text");
        assert_eq!(results[1].title, "Second News");
        assert_eq!(results[1].url, "https://news.example/2");
        assert_eq!(results[1].snippet, "Snippet two text");
        Ok(())
    }

    #[tokio::test]
    async fn truncates_results_to_limit() -> Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/news/infinitescrollajax"))
            .respond_with(ResponseTemplate::new(200).set_body_string(NEWS_FIXTURE))
            .mount(&server)
            .await;

        let provider =
            BingNewsFactory.create(bing_news_config(format!("{}/news/infinitescrollajax", server.uri())), reqwest::Client::new())?;
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
        assert_eq!(results[0].title, "News One Title");
        Ok(())
    }

    #[test]
    fn factory_creates_provider_without_api_key() {
        let config = WebSearchProviderConfig {
            provider: WebSearchProviderName::BingNews,
            api_key: None,
            timeout_ms: None,
            options: std::collections::HashMap::new(),
        };
        assert!(BingNewsFactory.create(config, reqwest::Client::new()).is_ok());
    }

    #[test]
    fn factory_rejects_unknown_options() {
        let mut options = std::collections::HashMap::new();
        options.insert("foo".to_string(), Value::String("bar".to_string()));
        let config = WebSearchProviderConfig {
            provider: WebSearchProviderName::BingNews,
            api_key: None,
            timeout_ms: None,
            options,
        };
        assert!(BingNewsFactory.create(config, reqwest::Client::new()).is_err());
    }
}
