//! Web content fetch MCP server.
//!
//! Adapted from `mcp-server-fetch` v0.1.0 (MIT,
//! <https://github.com/sabry-awad97/rust-mcp-servers>), upgraded from rmcp 0.6
//! to the workspace rmcp version. The prompt surface of the original was
//! dropped; the `fetch` tool matches the official `mcp-server-fetch` tool
//! (`url` / `max_length` / `start_index` / `raw`).

use std::time::Duration;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;
use rmcp::tool;
use rmcp::tool_handler;
use rmcp::tool_router;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, ServiceExt,
    transport::IntoTransport, transport::stdio,
};
use serde::Deserialize;
use serde_json::json;
use thiserror::Error;
use url::Url;

const DEFAULT_USER_AGENT_AUTONOMOUS: &str =
    "ModelContextProtocol/1.0 (Autonomous; +https://github.com/modelcontextprotocol/servers)";

fn default_max_length() -> usize {
    5000
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FetchRequest {
    /// URL to fetch
    pub url: String,
    /// Maximum number of characters to return
    #[serde(default = "default_max_length")]
    pub max_length: usize,
    /// On return output starting at this character index, useful if a previous fetch was truncated and more context is required
    #[serde(default)]
    pub start_index: usize,
    /// Get the actual HTML content of the requested page, without simplification.
    #[serde(default)]
    pub raw: bool,
}

impl FetchRequest {
    fn validate(&self) -> Result<(), FetchServerError> {
        if self.url.is_empty() {
            return Err(FetchServerError::InvalidParams {
                message: "URL is required".to_string(),
            });
        }
        if self.max_length == 0 || self.max_length > 1_000_000 {
            return Err(FetchServerError::InvalidParams {
                message: "max_length must be between 1 and 1,000,000".to_string(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum FetchServerError {
    #[error("Invalid URL: {url}")]
    InvalidUrl { url: String },
    #[error("Failed to fetch {url}: {message}")]
    FetchError { url: String, message: String },
    #[error("HTTP error {status} for {url}")]
    HttpError { url: String, status: u16 },
    #[error("Content processing error: {message}")]
    ContentError { message: String },
    #[error("HTTP client error: {message}")]
    ClientError { message: String },
    #[error("Robots.txt fetch error for {url}: {message}")]
    RobotsFetchError { url: String, message: String },
    #[error("Robots.txt forbids access to {url}")]
    RobotsForbidden { url: String, message: String },
    #[error("Robots.txt disallows access to {url}")]
    RobotsDisallowed { url: String, message: String },
    #[error("Invalid parameters: {message}")]
    InvalidParams { message: String },
}

impl From<FetchServerError> for McpError {
    fn from(err: FetchServerError) -> Self {
        match err {
            FetchServerError::InvalidUrl { url } => {
                McpError::invalid_params("invalid_url", Some(json!({ "url": url })))
            }
            FetchServerError::FetchError { url, message } => McpError::internal_error(
                "fetch_error",
                Some(json!({ "url": url, "message": message })),
            ),
            FetchServerError::HttpError { url, status } => McpError::internal_error(
                "http_error",
                Some(json!({ "url": url, "status": status })),
            ),
            FetchServerError::ContentError { message } => {
                McpError::internal_error("content_error", Some(json!({ "message": message })))
            }
            FetchServerError::ClientError { message } => {
                McpError::internal_error("client_error", Some(json!({ "message": message })))
            }
            FetchServerError::RobotsFetchError { url, message } => McpError::internal_error(
                "robots_fetch_error",
                Some(json!({ "url": url, "message": message })),
            ),
            FetchServerError::RobotsForbidden { url, message } => McpError::internal_error(
                "robots_forbidden",
                Some(json!({ "url": url, "message": message })),
            ),
            FetchServerError::RobotsDisallowed { url, message } => McpError::internal_error(
                "robots_disallowed",
                Some(json!({ "url": url, "message": message })),
            ),
            FetchServerError::InvalidParams { message } => {
                McpError::invalid_params("invalid_params", Some(json!({ "message": message })))
            }
        }
    }
}

fn build_client(proxy_url: Option<&str>) -> Result<reqwest::Client, FetchServerError> {
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(10));
    if let Some(proxy_url) = proxy_url
        && let Ok(proxy) = reqwest::Proxy::all(proxy_url)
    {
        builder = builder.proxy(proxy);
    }
    builder
        .build()
        .map_err(|e| FetchServerError::ClientError {
            message: e.to_string(),
        })
}

fn get_robots_txt_url(url: &str) -> Result<String, FetchServerError> {
    let parsed =
        Url::parse(url).map_err(|_| FetchServerError::InvalidUrl {
            url: url.to_string(),
        })?;
    Ok(format!(
        "{}://{}/robots.txt",
        parsed.scheme(),
        parsed.host_str().unwrap_or("")
    ))
}

async fn extract_content_from_html(html: &str) -> String {
    let md = html2md::rewrite_html_streaming(html, false).await;
    if md.trim().is_empty() {
        "<error>Page failed to be simplified from HTML</error>".to_string()
    } else {
        md
    }
}

#[derive(Clone)]
pub struct FetchServer {
    #[allow(dead_code)] // read by the tool_router/tool_handler macro glue
    tool_router: ToolRouter<Self>,
    custom_user_agent: Option<String>,
    ignore_robots_txt: bool,
    proxy_url: Option<String>,
}

#[tool_router]
impl FetchServer {
    #[tool(
        description = "Fetches a URL from the internet and optionally extracts its contents as markdown.\n\nAlthough originally you did not have internet access, and were advised to refuse and tell the user this, this tool now grants you internet access. Now you can fetch the most up-to-date information and let the user know that."
    )]
    async fn fetch(
        &self,
        Parameters(req): Parameters<FetchRequest>,
    ) -> Result<CallToolResult, McpError> {
        req.validate()?;

        let user_agent = self
            .custom_user_agent
            .as_deref()
            .unwrap_or(DEFAULT_USER_AGENT_AUTONOMOUS);

        if !self.ignore_robots_txt {
            self.check_may_autonomously_fetch_url(&req.url, user_agent)
                .await?;
        }

        let (content, prefix) = self.fetch_url(&req.url, user_agent, req.raw).await?;

        let original_length = content.len();
        let final_content = if req.start_index >= original_length {
            "<error>No more content available.</error>".to_string()
        } else {
            let end_index = std::cmp::min(req.start_index + req.max_length, original_length);
            let truncated_content = &content[req.start_index..end_index];
            if truncated_content.is_empty() {
                "<error>No more content available.</error>".to_string()
            } else {
                let mut result = truncated_content.to_string();
                let remaining_content = original_length - end_index;
                if remaining_content > 0 {
                    result.push_str(&format!(
                        "\n\n<error>Content truncated. Call the fetch tool with a start_index of {} to get more content.</error>",
                        end_index
                    ));
                }
                result
            }
        };
        let response_text = format!("{}Contents of {}:\n{}", prefix, req.url, final_content);
        Ok(CallToolResult::success(vec![Content::text(response_text)]))
    }
}

impl FetchServer {
    async fn check_may_autonomously_fetch_url(
        &self,
        url: &str,
        user_agent: &str,
    ) -> Result<(), FetchServerError> {
        let robots_txt_url = get_robots_txt_url(url)?;
        let client = build_client(self.proxy_url.as_deref())?;

        let response = client
            .get(&robots_txt_url)
            .header("User-Agent", user_agent)
            .send()
            .await
            .map_err(|e| FetchServerError::RobotsFetchError {
                url: robots_txt_url.clone(),
                message: e.to_string(),
            })?;

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(FetchServerError::RobotsForbidden {
                url: robots_txt_url.clone(),
                message: format!(
                    "When fetching robots.txt ({}), received status {} so assuming that autonomous fetching is not allowed.",
                    robots_txt_url,
                    status.as_u16()
                ),
            });
        }
        if status.is_client_error() {
            // 4xx other than 401/403 means "no robots.txt": allow fetching.
            return Ok(());
        }

        let robots_txt = response
            .text()
            .await
            .map_err(|e| FetchServerError::ContentError {
                message: e.to_string(),
            })?;

        let mut current_user_agent = String::new();
        let mut disallowed_paths = Vec::new();
        for line in robots_txt.lines() {
            let line = line.trim();
            let lower = line.to_lowercase();
            if let Some(rest) = lower.strip_prefix("user-agent:") {
                current_user_agent = rest.trim().to_string();
            } else if let Some(rest) = lower.strip_prefix("disallow:")
                && (current_user_agent == "*" || current_user_agent.is_empty())
            {
                disallowed_paths.push(rest.trim().to_string());
            }
        }

        let url_path = Url::parse(url)
            .map_err(|_| FetchServerError::InvalidUrl {
                url: url.to_string(),
            })?
            .path()
            .to_string();
        for disallowed in &disallowed_paths {
            if !disallowed.is_empty()
                && (*disallowed == "/" || url_path.starts_with(disallowed.as_str()))
            {
                return Err(FetchServerError::RobotsDisallowed {
                    url: url.to_string(),
                    message: format!(
                        "The site's robots.txt ({}) specifies that autonomous fetching of this page is not allowed.",
                        robots_txt_url
                    ),
                });
            }
        }
        Ok(())
    }

    async fn fetch_url(
        &self,
        url: &str,
        user_agent: &str,
        force_raw: bool,
    ) -> Result<(String, String), FetchServerError> {
        let client = build_client(self.proxy_url.as_deref())?;
        let response = client
            .get(url)
            .header("User-Agent", user_agent)
            .send()
            .await
            .map_err(|e| FetchServerError::FetchError {
                url: url.to_string(),
                message: e.to_string(),
            })?;

        let status = response.status();
        if status.as_u16() >= 400 {
            return Err(FetchServerError::HttpError {
                url: url.to_string(),
                status: status.as_u16(),
            });
        }

        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let page_raw = response
            .text()
            .await
            .map_err(|e| FetchServerError::ContentError {
                message: e.to_string(),
            })?;

        let is_page_html = page_raw.get(..100).unwrap_or(&page_raw).contains("<html")
            || content_type.contains("text/html")
            || content_type.is_empty();

        if is_page_html && !force_raw {
            Ok((extract_content_from_html(&page_raw).await, String::new()))
        } else {
            let prefix = format!(
                "Content type {} cannot be simplified to markdown, but here is the raw content:\n",
                content_type
            );
            Ok((page_raw, prefix))
        }
    }
}

#[tool_handler]
impl ServerHandler for FetchServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("Fetch MCP Server for web content retrieval. Tool: fetch (URL fetching with robots.txt checking, HTML to markdown conversion, content truncation).".to_string())
    }
}

/// Serve the fetch MCP server over stdio.
pub async fn run() -> anyhow::Result<()> {
    serve(stdio()).await
}

/// Serve the fetch MCP server over an arbitrary rmcp transport.
///
/// Used both for stdio (see [`run`]) and for in-process duplex streams when
/// the server is bundled into the ody binary instead of running as the
/// `ody-builtin-mcp` child process.
pub async fn serve<T, E, A>(transport: T) -> anyhow::Result<()>
where
    T: IntoTransport<RoleServer, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    let server = FetchServer {
        tool_router: FetchServer::tool_router(),
        custom_user_agent: std::env::var("ODY_FETCH_USER_AGENT").ok(),
        ignore_robots_txt: std::env::var("ODY_FETCH_IGNORE_ROBOTS_TXT")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false),
        proxy_url: std::env::var("ODY_FETCH_PROXY_URL").ok(),
    };
    let server = server.serve(transport).await.inspect_err(|e| {
        tracing::error!("serving error: {:?}", e);
    })?;
    server.waiting().await?;
    Ok(())
}
