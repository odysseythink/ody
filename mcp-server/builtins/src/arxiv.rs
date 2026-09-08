//! arXiv MCP server.
//!
//! Tool schema aligned with the chatbox-hosted arXiv server (which followed
//! `blazickjp/arxiv-mcp-server`): `search_papers`, `download_paper`,
//! `list_papers`, `read_paper`.
//!
//! Search uses the public arXiv Atom API (`export.arxiv.org`). Downloads go
//! through ar5iv's HTML rendering (converted to markdown); papers without an
//! HTML rendering return a clear error instead of falling back to PDF text
//! extraction.

use std::path::PathBuf;

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

const ARXIV_API_URL: &str = "http://export.arxiv.org/api/query";
const AR5IV_HTML_URL: &str = "https://ar5iv.labs.arxiv.org/html";
const DEFAULT_MAX_RESULTS: u32 = 10;
const MAX_RESULTS_CAP: u32 = 50;
const DEFAULT_READ_CHARS: usize = 12_000;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchPapersRequest {
    /// arXiv query string. Supports quoted phrases, field prefixes (ti:, au:, abs:, cat:) and boolean operators (AND/OR/ANDNOT)
    pub query: String,
    /// Maximum number of results to return (default: 10, max: 50)
    pub max_results: Option<u32>,
    /// Inclusive start date (YYYY-MM-DD)
    pub date_from: Option<String>,
    /// Inclusive end date (YYYY-MM-DD)
    pub date_to: Option<String>,
    /// arXiv category filters (e.g. ["cs.LG", "cs.AI"]); strongly improves relevance
    pub categories: Option<Vec<String>>,
    /// Sort by "relevance" (default) or "date" (newest first)
    pub sort_by: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DownloadPaperRequest {
    /// The arXiv ID of the paper to download (e.g. "2103.12345")
    pub paper_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListPapersRequest {}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReadPaperRequest {
    /// The arXiv ID of the paper to read
    pub paper_id: String,
    /// On return output starting at this character index, useful if a previous read was truncated
    #[serde(default)]
    pub start: usize,
    /// Maximum number of characters to return (default: 12000)
    #[serde(default)]
    pub max_length: usize,
}

fn err(message: impl Into<String>) -> McpError {
    McpError::internal_error(message.into(), None)
}

fn storage_path() -> PathBuf {
    if let Ok(path) = std::env::var("ODY_BUILTIN_ARXIV_STORAGE") {
        return PathBuf::from(path);
    }
    #[cfg(windows)]
    {
        if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
            return PathBuf::from(local_app_data)
                .join("ody-builtin-mcp")
                .join("arxiv");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home)
            .join(".cache")
            .join("ody-builtin-mcp")
            .join("arxiv");
    }
    std::env::temp_dir().join("ody-builtin-mcp").join("arxiv")
}

/// Map a (possibly old-style or versioned) arXiv ID to a filesystem-safe stem.
fn paper_stem(paper_id: &str) -> String {
    paper_id
        .trim()
        .trim_start_matches("arXiv:")
        .trim_start_matches("arxiv:")
        .replace('/', "_")
        .replace(':', "_")
}

fn build_search_query(req: &SearchPapersRequest) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(categories) = &req.categories {
        for category in categories {
            parts.push(format!("cat:{category}"));
        }
    }
    parts.push(format!("({})", req.query));
    match (&req.date_from, &req.date_to) {
        (Some(from), Some(to)) => parts.push(format!(
            "submittedDate:[{}000000 TO {}235959]",
            from.replace('-', ""),
            to.replace('-', "")
        )),
        (Some(from), None) => parts.push(format!(
            "submittedDate:[{}000000 TO *]",
            from.replace('-', "")
        )),
        (None, Some(to)) => parts.push(format!(
            "submittedDate:[* TO {}235959]",
            to.replace('-', "")
        )),
        (None, None) => {}
    }
    parts.join(" AND ")
}

#[derive(Debug, Default)]
struct FeedEntry {
    id: String,
    title: String,
    published: String,
    summary: String,
    authors: Vec<String>,
    in_author: bool,
}

fn normalize_text(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parse the arXiv Atom feed, returning one JSON object per entry.
fn parse_feed(xml: &str) -> Result<Vec<serde_json::Value>, McpError> {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;

    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut entries: Vec<FeedEntry> = Vec::new();
    let mut current: Option<FeedEntry> = None;
    let mut field: Option<String> = None;

    loop {
        match reader.read_event() {
            Err(e) => return Err(err(format!("Failed to parse arXiv feed: {e}"))),
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => match e.local_name().into_inner() {
                b"entry" => current = Some(FeedEntry::default()),
                b"author" => {
                    if let Some(c) = current.as_mut() {
                        c.in_author = true;
                    }
                }
                b"id" | b"title" | b"published" | b"summary" | b"name" => {
                    field = Some(
                        String::from_utf8_lossy(e.local_name().into_inner()).into_owned(),
                    );
                }
                _ => {}
            },
            Ok(Event::Text(t)) => {
                let text = t.decode().map_err(|e| err(e.to_string()))?.into_owned();
                if let (Some(c), Some(f)) = (current.as_mut(), field.as_deref()) {
                    match f {
                        "id" => c.id.push_str(&text),
                        "title" => c.title.push_str(&text),
                        "published" => c.published.push_str(&text),
                        "summary" => c.summary.push_str(&text),
                        "name" if c.in_author => c.authors.push(normalize_text(&text)),
                        _ => {}
                    }
                }
            }
            Ok(Event::End(e)) => match e.local_name().into_inner() {
                b"entry" => {
                    if let Some(c) = current.take() {
                        entries.push(c);
                    }
                }
                b"author" => {
                    if let Some(c) = current.as_mut() {
                        c.in_author = false;
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    Ok(entries
        .into_iter()
        .map(|e| {
            let url = e.id.clone();
            json!({
                "id": e.id,
                "title": normalize_text(&e.title),
                "authors": e.authors,
                "published": e.published,
                "summary": normalize_text(&e.summary),
                "url": url,
            })
        })
        .collect())
}

#[derive(Clone)]
pub struct ArxivServer {
    #[allow(dead_code)] // read by the tool_router/tool_handler macro glue
    tool_router: ToolRouter<Self>,
    client: reqwest::Client,
    storage: PathBuf,
}

#[tool_router]
impl ArxivServer {
    #[tool(
        description = "Search for papers on arXiv with optional categories, date range, and sorting. Query supports quoted phrases, field prefixes (ti:, au:, abs:, cat:) and boolean operators (AND/OR/ANDNOT)."
    )]
    async fn search_papers(
        &self,
        Parameters(req): Parameters<SearchPapersRequest>,
    ) -> Result<CallToolResult, McpError> {
        if req.query.trim().is_empty() {
            return Err(McpError::invalid_params("query is required", None));
        }
        let max_results = req
            .max_results
            .unwrap_or(DEFAULT_MAX_RESULTS)
            .clamp(1, MAX_RESULTS_CAP);
        let sort_by = match req.sort_by.as_deref() {
            Some("date") => "submittedDate",
            _ => "relevance",
        };

        let response = self
            .client
            .get(ARXIV_API_URL)
            .query(&[
                ("search_query", build_search_query(&req)),
                ("start", "0".to_string()),
                ("max_results", max_results.to_string()),
                ("sortBy", sort_by.to_string()),
                ("sortOrder", "descending".to_string()),
            ])
            .send()
            .await
            .map_err(|e| err(format!("Failed to reach arXiv API: {e}")))?;

        if !response.status().is_success() {
            return Err(err(format!(
                "arXiv API returned status {}",
                response.status()
            )));
        }

        let feed = response
            .text()
            .await
            .map_err(|e| err(format!("Failed to read arXiv response: {e}")))?;
        let results = parse_feed(&feed)?;
        let text = serde_json::to_string_pretty(&results)
            .map_err(|e| err(format!("Failed to serialize results: {e}")))?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    #[tool(
        description = "Download a paper from arXiv and return its text content. Uses the ar5iv HTML rendering, converted to markdown, and stores the paper locally. Call read_paper later to re-read without downloading."
    )]
    async fn download_paper(
        &self,
        Parameters(req): Parameters<DownloadPaperRequest>,
    ) -> Result<CallToolResult, McpError> {
        let stem = paper_stem(&req.paper_id);
        if stem.is_empty() {
            return Err(McpError::invalid_params(
                "paper_id is required",
                Some(json!({ "paper_id": req.paper_id })),
            ));
        }

        let response = self
            .client
            .get(format!("{AR5IV_HTML_URL}/{stem}"))
            .send()
            .await
            .map_err(|e| err(format!("Failed to fetch paper HTML: {e}")))?;

        if !response.status().is_success() {
            return Err(err(format!(
                "Paper '{stem}' has no HTML rendering available (ar5iv status {}); only HTML papers can be downloaded",
                response.status()
            )));
        }

        let html = response
            .text()
            .await
            .map_err(|e| err(format!("Failed to read paper HTML: {e}")))?;
        let markdown = html2md::rewrite_html_streaming(&html, false).await;
        if markdown.trim().is_empty() {
            return Err(err(format!(
                "Paper '{stem}' failed to be converted from HTML to markdown"
            )));
        }

        std::fs::create_dir_all(&self.storage).map_err(|e| {
            err(format!(
                "Failed to create arXiv storage directory {}: {e}",
                self.storage.display()
            ))
        })?;
        std::fs::write(self.storage.join(format!("{stem}.md")), &markdown).map_err(|e| {
            err(format!("Failed to store paper '{stem}': {e}"))
        })?;

        Ok(CallToolResult::success(vec![Content::text(markdown)]))
    }

    #[tool(
        description = "List all papers that have been downloaded and stored locally via download_paper. Returns arXiv IDs only — use read_paper to access content. Returns an empty list if no papers have been downloaded yet. Workflow: search_papers -> download_paper -> list_papers -> read_paper."
    )]
    async fn list_papers(
        &self,
        Parameters(_req): Parameters<ListPapersRequest>,
    ) -> Result<CallToolResult, McpError> {
        let mut ids: Vec<String> = Vec::new();
        if self.storage.is_dir() {
            let entries = std::fs::read_dir(&self.storage).map_err(|e| {
                err(format!(
                    "Failed to list arXiv storage directory {}: {e}",
                    self.storage.display()
                ))
            })?;
            for entry in entries {
                let entry = entry.map_err(|e| err(e.to_string()))?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if let Some(stem) = name.strip_suffix(".md") {
                    ids.push(stem.to_string());
                }
            }
        }
        ids.sort();
        let text = serde_json::to_string_pretty(&ids)
            .map_err(|e| err(format!("Failed to serialize paper list: {e}")))?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    #[tool(
        description = "Read the text content of a paper that was previously downloaded via download_paper. Returns the paper in markdown format, bounded to 12,000 characters by default; use start/max_length to page through large papers. Will fail with a clear error if the paper has not been downloaded yet — call download_paper first. Workflow: search_papers -> download_paper -> read_paper."
    )]
    async fn read_paper(
        &self,
        Parameters(req): Parameters<ReadPaperRequest>,
    ) -> Result<CallToolResult, McpError> {
        let stem = paper_stem(&req.paper_id);
        let path = self.storage.join(format!("{stem}.md"));
        let content = std::fs::read_to_string(&path).map_err(|_| {
            err(format!(
                "Paper '{stem}' not found in storage. You may need to download it first using download_paper."
            ))
        })?;

        let max_length = if req.max_length == 0 {
            DEFAULT_READ_CHARS
        } else {
            req.max_length
        };
        let start = req.start.min(content.len());
        let end = (start + max_length).min(content.len());
        let mut text = content[start..end].to_string();
        if end < content.len() {
            text.push_str(&format!(
                "\n\n<error>Content truncated. Call the read_paper tool with a start of {end} to get more content.</error>"
            ));
        }
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }
}

#[tool_handler]
impl ServerHandler for ArxivServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("MCP server for accessing arXiv papers. Tools: search_papers (search by query with optional categories/date/sort), download_paper (download via ar5iv HTML, stored locally), list_papers (list stored papers), read_paper (read stored paper content).".to_string())
    }
}

/// Serve the arXiv MCP server over stdio.
pub async fn run() -> anyhow::Result<()> {
    serve(stdio()).await
}

/// Serve the arXiv MCP server over an arbitrary rmcp transport.
///
/// Used both for stdio (see [`run`]) and for in-process duplex streams when
/// the server is bundled into the ody binary instead of running as the
/// `ody-builtin-mcp` child process.
pub async fn serve<T, E, A>(transport: T) -> anyhow::Result<()>
where
    T: IntoTransport<RoleServer, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    let server = ArxivServer {
        tool_router: ArxivServer::tool_router(),
        client: reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?,
        storage: storage_path(),
    };
    let server = server.serve(transport).await.inspect_err(|e| {
        tracing::error!("serving error: {:?}", e);
    })?;
    server.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stem_sanitizes_old_style_ids() {
        assert_eq!(paper_stem("cs/0612060"), "cs_0612060");
        assert_eq!(paper_stem("arXiv:2103.12345v2"), "2103.12345v2");
        assert_eq!(paper_stem("2103.12345"), "2103.12345");
    }

    #[test]
    fn search_query_combines_filters() {
        let req = SearchPapersRequest {
            query: "\"neural networks\"".to_string(),
            max_results: None,
            date_from: Some("2023-01-01".to_string()),
            date_to: Some("2023-12-31".to_string()),
            categories: Some(vec!["cs.LG".to_string(), "cs.AI".to_string()]),
            sort_by: None,
        };
        let q = build_search_query(&req);
        assert_eq!(
            q,
            "cat:cs.LG AND cat:cs.AI AND (\"neural networks\") AND submittedDate:[20230101000000 TO 20231231235959]"
        );
    }

    #[test]
    fn parses_atom_feed() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <feed xmlns="http://www.w3.org/2005/Atom">
          <entry>
            <id>http://arxiv.org/abs/2103.12345v1</id>
            <published>2021-03-24T00:00:00Z</published>
            <title>Test Paper   Title</title>
            <summary> A summary
            spanning lines </summary>
            <author><name>Jane Doe</name></author>
            <author><name>John Roe</name></author>
          </entry>
        </feed>"#;
        let results = parse_feed(xml).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["title"], "Test Paper Title");
        assert_eq!(results[0]["authors"][0], "Jane Doe");
        assert_eq!(results[0]["authors"][1], "John Roe");
        assert_eq!(results[0]["summary"], "A summary spanning lines");
    }
}
