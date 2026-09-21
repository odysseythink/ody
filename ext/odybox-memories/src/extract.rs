//! Session extraction: turn one odyBox conversation transcript into a memory
//! record.
//!
//! Extraction is behind a trait so the pipeline can be exercised in tests
//! without a model call, and so a different backend can be swapped in later.

use std::future::Future;
use std::pin::Pin;

use serde::Deserialize;

/// A single session's extracted memory.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtractedMemory {
    /// Compact routing line used for indexing.
    pub summary: String,
    /// Markdown memory body.
    pub raw_memory: String,
}

impl ExtractedMemory {
    /// Whether the model judged there was nothing worth keeping.
    pub fn is_empty(&self) -> bool {
        self.summary.trim().is_empty() && self.raw_memory.trim().is_empty()
    }

    /// Renders the record persisted for the session.
    pub fn render(&self, thread_id: &str, recorded_at: &str) -> String {
        format!(
            "# Assistant Memory: {thread_id}\n\nthread_id: {thread_id}\nrecorded_at: {recorded_at}\n\n## Summary\n\n{}\n\n{}\n",
            self.summary.trim(),
            self.raw_memory.trim()
        )
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error("extraction model call failed: {0}")]
    Model(String),
    #[error("extraction output was not valid JSON: {0}")]
    Parse(String),
}

/// Boxed future returned by extractor implementations.
pub type ExtractFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ExtractedMemory, ExtractError>> + Send + 'a>>;

/// Turns a session transcript into a memory record.
pub trait MemoryExtractor: Send + Sync {
    fn extract<'a>(&'a self, transcript: &'a str) -> ExtractFuture<'a>;
}

/// Extractor that never produces memory. Useful as a default and in tests that
/// only exercise scheduling.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopExtractor;

impl MemoryExtractor for NoopExtractor {
    fn extract<'a>(&'a self, _transcript: &'a str) -> ExtractFuture<'a> {
        Box::pin(async {
            Ok(ExtractedMemory {
                summary: String::new(),
                raw_memory: String::new(),
            })
        })
    }
}

/// Parses a model response into memory.
///
/// Several providers are configured with `wire_api = "chat"`, where
/// `output_schema` is a Responses-API-only feature and is therefore ignored.
/// Such models often wrap their JSON in a markdown fence or add a sentence of
/// prose, so this tolerates both instead of demanding a bare object.
pub fn parse_extracted_memory(raw: &str) -> Result<ExtractedMemory, ExtractError> {
    let trimmed = raw.trim();
    if let Ok(memory) = serde_json::from_str::<ExtractedMemory>(trimmed) {
        return Ok(memory);
    }

    for candidate in extraction_candidates(trimmed) {
        if let Ok(memory) = serde_json::from_str::<ExtractedMemory>(&candidate) {
            return Ok(memory);
        }
    }

    Err(ExtractError::Parse(format!(
        "no JSON object in extraction output; raw response starts with: {}",
        trimmed.chars().take(200).collect::<String>()
    )))
}

fn extraction_candidates(text: &str) -> Vec<String> {
    let mut candidates = Vec::new();

    // ```json { ... } ``` (also handles a bare ``` fence)
    if let Some(fence_start) = text.find("```") {
        let after_fence = &text[fence_start + 3..];
        let after_fence = after_fence.strip_prefix("json").unwrap_or(after_fence);
        let after_fence = after_fence.trim_start();
        if let Some(fence_end) = after_fence.find("```") {
            candidates.push(after_fence[..fence_end].trim().to_string());
        }
    }

    // The widest { ... } span, which covers "prose before/after the JSON".
    if let (Some(start), Some(end)) = (text.find('{'), text.rfind('}'))
        && start < end
    {
        candidates.push(text[start..=end].to_string());
    }

    candidates
}
