//! Context7 MCP server.
//!
//! Port of `@upstash/context7-mcp` v3.x tools against the Context7 v2 HTTP
//! API (`https://context7.com/api/v2`), matching the current official tool
//! surface: `resolve-library-id` and `query-docs` (aliases of the historical
//! `get-library-docs` argument names are accepted).

use std::time::Duration;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;
use rmcp::tool;
use rmcp::tool_handler;
use rmcp::tool_router;
use rmcp::{ErrorData as McpError, ServerHandler, ServiceExt, transport::stdio};
use serde::Deserialize;
use serde_json::json;

const API_BASE_URL: &str = "https://context7.com/api";

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResolveLibraryIdRequest {
    /// The user's question or task (used for relevance ranking)
    pub query: String,
    /// Library name to search for and retrieve a Context7-compatible library ID. Use the official library name with proper punctuation — e.g., 'Next.js' instead of 'nextjs'
    pub library_name: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct QueryDocsRequest {
    /// Exact Context7-compatible library ID (e.g., '/vercel/next.js') retrieved from 'resolve-library-id' or directly from the user query in the format '/org/project' or '/org/project/version'
    #[serde(alias = "context7CompatibleLibraryID", alias = "libraryID")]
    pub library_id: String,
    /// What to look up in the library's documentation, scoped to a single concept. The query is sent to the Context7 API for processing
    pub query: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchResult {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    total_snippets: i64,
    #[serde(default = "default_neg_one")]
    trust_score: f64,
    #[serde(default)]
    benchmark_score: f64,
    #[serde(default)]
    versions: Vec<String>,
    #[serde(default)]
    source: Option<String>,
}

fn default_neg_one() -> f64 {
    -1.0
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    results: Vec<SearchResult>,
}

fn err(message: impl Into<String>) -> McpError {
    McpError::internal_error(message.into(), None)
}

fn reputation_label(trust_score: f64) -> &'static str {
    if trust_score < 0.0 {
        "Unknown"
    } else if trust_score >= 7.0 {
        "High"
    } else if trust_score >= 4.0 {
        "Medium"
    } else {
        "Low"
    }
}

fn format_search_result(result: &SearchResult) -> String {
    let mut lines = vec![
        format!("- Title: {}", result.title),
        format!("- Context7-compatible library ID: {}", result.id),
        format!("- Description: {}", result.description),
    ];
    if result.total_snippets != -1 {
        lines.push(format!("- Code Snippets: {}", result.total_snippets));
    }
    lines.push(format!(
        "- Source Reputation: {}",
        reputation_label(result.trust_score)
    ));
    if result.benchmark_score > 0.0 {
        lines.push(format!("- Benchmark Score: {}", result.benchmark_score));
    }
    if !result.versions.is_empty() {
        lines.push(format!("- Versions: {}", result.versions.join(", ")));
    }
    if let Some(source) = &result.source {
        lines.push(format!("- Source: {source}"));
    }
    lines.join("\n")
}

#[derive(Clone)]
pub struct Context7Server {
    #[allow(dead_code)] // read by the tool_router/tool_handler macro glue
    tool_router: ToolRouter<Self>,
    client: reqwest::Client,
    api_key: Option<String>,
}

impl Context7Server {
    fn add_api_key(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.api_key {
            Some(key) => request.header("X-Context7-API-Key", key),
            None => request,
        }
    }
}

#[tool_router]
impl Context7Server {
    #[tool(
        name = "resolve-library-id",
        description = "Resolves a package/product name to a Context7-compatible library ID and returns matching libraries.

You MUST call this function before 'query-docs' to obtain a valid Context7-compatible library ID UNLESS the user explicitly provides a library ID in the format '/org/project' or '/org/project/version' in their query.

Each result includes the library ID, title, description, code snippet count, and source reputation."
    )]
    async fn resolve_library_id(
        &self,
        Parameters(req): Parameters<ResolveLibraryIdRequest>,
    ) -> Result<CallToolResult, McpError> {
        if req.library_name.trim().is_empty() {
            return Err(McpError::invalid_params(
                "libraryName is required",
                Some(json!({ "libraryName": req.library_name })),
            ));
        }

        let response = self
            .add_api_key(
                self.client
                    .get(format!("{API_BASE_URL}/v2/libs/search"))
                    .query(&[("query", &req.query), ("libraryName", &req.library_name)]),
            )
            .send()
            .await
            .map_err(|e| err(format!("Error searching libraries: {e}")))?;

        if !response.status().is_success() {
            return Err(err(format!(
                "Context7 search failed with status {}: {}",
                response.status(),
                response.text().await.unwrap_or_default()
            )));
        }

        let search: SearchResponse = response
            .json()
            .await
            .map_err(|e| err(format!("Failed to parse Context7 search response: {e}")))?;

        if search.results.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "No libraries found matching the provided name.",
            )]));
        }

        let formatted: Vec<String> = search.results.iter().map(format_search_result).collect();
        let text = format!("Available Libraries:\n\n{}", formatted.join("\n----------\n"));
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    #[tool(
        name = "query-docs",
        description = "Retrieves and queries up-to-date documentation and code examples from Context7 for any programming library or framework.

You must call 'resolve-library-id' first to obtain the exact Context7-compatible library ID required to use this tool, UNLESS the user explicitly provides a library ID in the format '/org/project' or '/org/project/version' in their query.

Do not call this tool more than 3 times per question."
    )]
    async fn query_docs(
        &self,
        Parameters(req): Parameters<QueryDocsRequest>,
    ) -> Result<CallToolResult, McpError> {
        if req.library_id.trim().is_empty() {
            return Err(McpError::invalid_params(
                "libraryId is required",
                Some(json!({ "libraryId": req.library_id })),
            ));
        }

        let response = self
            .add_api_key(
                self.client
                    .get(format!("{API_BASE_URL}/v2/context"))
                    .query(&[("query", &req.query), ("libraryId", &req.library_id)]),
            )
            .send()
            .await
            .map_err(|e| err(format!("Error fetching library context. Please try again later. {e}")))?;

        if !response.status().is_success() {
            return Err(err(format!(
                "Context7 context request failed with status {}: {}",
                response.status(),
                response.text().await.unwrap_or_default()
            )));
        }

        let text = response.text().await.map_err(|e| {
            err(format!("Failed to read Context7 context response: {e}"))
        })?;
        if text.trim().is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "Documentation not found or not finalized for this library. This might have happened because you used an invalid Context7-compatible library ID. To get a valid Context7-compatible library ID, use the 'resolve-library-id' with the package name you wish to retrieve documentation for.",
            )]));
        }
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }
}

#[tool_handler]
impl ServerHandler for Context7Server {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("Use this server to fetch current documentation whenever the user asks about a library, framework, SDK, API, CLI tool, or cloud service. Tools: resolve-library-id (map a package name to a Context7 library ID), query-docs (fetch up-to-date documentation and code examples for a library ID).".to_string())
    }
}

/// Serve the Context7 MCP server over stdio.
pub async fn run() -> anyhow::Result<()> {
    let server = Context7Server {
        tool_router: Context7Server::tool_router(),
        client: reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()?,
        api_key: std::env::var("CONTEXT7_API_KEY").ok(),
    };
    let server = server.serve(stdio()).await.inspect_err(|e| {
        tracing::error!("serving error: {:?}", e);
    })?;
    server.waiting().await?;
    Ok(())
}
