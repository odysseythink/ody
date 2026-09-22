//! Session extraction: turn one odyBox conversation into structured claims.
//!
//! The model proposes; it does not decide what is true. Every claim it returns
//! has to cite the transcript line it came from, and a claim whose citation
//! does not exist in that transcript is dropped before it can reach the store.
//! This is the first of two gates protecting the store — the second is that the
//! store itself refuses a claim with no evidence.
//!
//! Extraction is behind a trait so the pipeline can be exercised without a
//! model call, and so a different backend can be swapped in later.

use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;

use chrono::DateTime;
use chrono::Duration;
use chrono::Utc;
use serde::Deserialize;

use crate::EvidenceKind;
use crate::NewClaim;
use crate::NewEvidence;
use crate::memory_db::claim_kind_from;
use crate::memory_db::confidence_from;
use crate::prompts::Transcript;
use crate::reconcile::ReconcileCandidate;

/// One observation the model attributes to a transcript line.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceDraft {
    /// Index of the transcript line the observation came from.
    pub item: u32,
    /// The words in that line that carry the observation.
    pub quote: String,
}

/// A claim proposed by the model, before it is grounded or stored.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimDraft {
    pub kind: String,
    pub subject: String,
    pub statement: String,
    pub confidence: String,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub decision_implication: Option<String>,
    /// How long the claim should stand before the assistant revisits it.
    #[serde(default)]
    pub review_in_days: Option<u32>,
    /// Id of a claim this one replaces, from the list the model was shown.
    #[serde(default)]
    pub supersedes: Option<String>,
    #[serde(default)]
    pub evidence: Vec<EvidenceDraft>,
}

/// Everything one session produced.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtractionResult {
    /// Compact routing line describing the session.
    pub summary: String,
    #[serde(default)]
    pub claims: Vec<ClaimDraft>,
}

impl ExtractionResult {
    /// Whether the session yielded nothing worth keeping.
    ///
    /// `summary` does not count: it is a routing line for indexing, not a
    /// memory. A session whose claims were all discarded is empty even if the
    /// model volunteered a summary.
    pub fn is_empty(&self) -> bool {
        self.claims.is_empty()
    }

    /// Drops every claim that cannot be traced back to a real transcript line.
    ///
    /// This is the gate that makes a memory auditable: a claim the model cannot
    /// point at is dropped, and so is a citation to a line that does not exist.
    pub fn retain_grounded(&mut self, valid_items: &BTreeSet<u32>) {
        for claim in &mut self.claims {
            claim
                .evidence
                .retain(|evidence| valid_items.contains(&evidence.item));
        }
        self.claims.retain(|claim| !claim.evidence.is_empty());
    }

    /// Converts grounded claims into store rows, with their source attached.
    pub fn to_candidates(
        &self,
        thread_id: &str,
        observed_at: DateTime<Utc>,
    ) -> Vec<ReconcileCandidate> {
        self.claims
            .iter()
            .map(|draft| {
                let claim = NewClaim {
                    kind: claim_kind_from(&draft.kind),
                    subject: draft.subject.clone(),
                    statement: draft.statement.clone(),
                    confidence: confidence_from(&draft.confidence),
                    scope: draft.scope.clone(),
                    decision_implication: draft.decision_implication.clone(),
                    review_due: draft
                        .review_in_days
                        .map(|days| observed_at + Duration::days(i64::from(days))),
                    valid_from: Some(observed_at),
                };
                let evidence = draft
                    .evidence
                    .iter()
                    .map(|item| NewEvidence {
                        kind: EvidenceKind::Fact,
                        occurred_at: observed_at,
                        source_thread: thread_id.to_string(),
                        source_locator: format!("item:{}", item.item),
                        excerpt: item.quote.clone(),
                    })
                    .collect();
                ReconcileCandidate {
                    claim,
                    evidence,
                    supersedes: draft.supersedes.clone(),
                }
            })
            .collect()
    }

    /// Renders the human-readable record kept for the session.
    pub fn render(&self, thread_id: &str, recorded_at: &str) -> String {
        let mut body = format!(
            "# Assistant Memory: {thread_id}

thread_id: {thread_id}
recorded_at: {recorded_at}

## Summary

{}
",
            self.summary.trim()
        );
        if !self.claims.is_empty() {
            body.push_str(
                "
## Claims

| kind | subject | statement | confidence |
| --- | --- | --- | --- |
",
            );
            for claim in &self.claims {
                body.push_str(&format!(
                    "| {} | {} | {} | {} |
",
                    claim.kind, claim.subject, claim.statement, claim.confidence
                ));
            }
        }
        body
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
    Pin<Box<dyn Future<Output = Result<ExtractionResult, ExtractError>> + Send + 'a>>;

/// Turns a session transcript into structured claims.
pub trait MemoryExtractor: Send + Sync {
    fn extract<'a>(&'a self, transcript: &'a Transcript) -> ExtractFuture<'a>;
}

/// Extractor that never produces memory. Useful as a default and in tests that
/// only exercise scheduling.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopExtractor;

impl MemoryExtractor for NoopExtractor {
    fn extract<'a>(&'a self, _transcript: &'a Transcript) -> ExtractFuture<'a> {
        Box::pin(async {
            Ok(ExtractionResult {
                summary: String::new(),
                claims: Vec::new(),
            })
        })
    }
}

/// Parses a model response into an extraction result.
///
/// Several providers are configured with `wire_api = "chat"`, where
/// `output_schema` is a Responses-API-only feature and is therefore ignored.
/// Such models often wrap their JSON in a markdown fence or add a sentence of
/// prose, so this tolerates both instead of demanding a bare object.
pub fn parse_extraction_result(raw: &str) -> Result<ExtractionResult, ExtractError> {
    let trimmed = raw.trim();
    if let Ok(result) = serde_json::from_str::<ExtractionResult>(trimmed) {
        return Ok(prune_uncited(result));
    }

    for candidate in extraction_candidates(trimmed) {
        if let Ok(result) = serde_json::from_str::<ExtractionResult>(&candidate) {
            return Ok(prune_uncited(result));
        }
    }

    Err(ExtractError::Parse(format!(
        "no JSON object in extraction output; raw response starts with: {}",
        trimmed.chars().take(200).collect::<String>()
    )))
}

/// Drops claims the model did not cite at all.
///
/// A claim with no citation is an opinion about the user, not a memory: it
/// cannot be reviewed, corrected, or defended later.
fn prune_uncited(result: ExtractionResult) -> ExtractionResult {
    let mut result = result;
    for claim in &mut result.claims {
        claim
            .evidence
            .retain(|evidence| !evidence.quote.trim().is_empty());
    }
    result.claims.retain(|claim| !claim.evidence.is_empty());
    result
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

#[cfg(test)]
#[path = "extract_tests.rs"]
mod tests;
