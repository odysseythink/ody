//! Types for the design-mode adversarial review.

use serde::Deserialize;
use serde::Serialize;
use std::fmt;

/// Input to a design review.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DesignReviewRequest {
    pub design_markdown: String,
    pub review_model: String,
    /// Risks the user already ACCEPTED or DEFERRED in earlier rounds of this same
    /// design (the sign-off-seen fingerprints, `F:`/`A:`-prefixed). Fed back into
    /// the reviewer prompt so a stateless re-review stops re-raising them and the
    /// finding count can actually fall across rounds. Empty on the first pass.
    pub accepted_risks: Vec<String>,
    /// v1.6b (D11): whether to append the usability-lens Skeptic turn to the debate.
    /// Resolved by the `submit_artifact` handler from `usability_lens` (`On` ⇒ true,
    /// `Off` ⇒ false, `Ask` ⇒ the user's answer to the recommendation prompt). The
    /// authoritative value — it overrides the config-derived default in the debate.
    pub run_usability_pass: bool,
}

/// Structured output of a design review.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DesignReviewOutput {
    pub overall_assessment: String,
    pub findings: Vec<DesignReviewFinding>,
}

/// Where a finding came from, for the review appendix. Set by the orchestration
/// (NOT by the model): the single-shot critic's findings are [`Self::Critic`];
/// findings the debate surfaced beyond the critic are [`Self::Debate`]; a critic
/// finding the debate's Judge refuted (v1.5b, opt-in via `contest_critic`) is
/// retained as [`Self::Contested`] (downgraded to Speculative, never deleted).
/// Not part of the model's JSON contract, so it never affects parsing or
/// [`DesignReviewFinding::fingerprint`] (which is title-only).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum FindingProvenance {
    #[default]
    Critic,
    Debate,
    Contested,
}

/// A single adversarial finding.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DesignReviewFinding {
    pub severity: DesignReviewSeverity,
    pub confidence: DesignReviewConfidence,
    pub title: String,
    pub detail: String,
    pub location: Option<String>,
    pub suggested_fix: Option<String>,
    /// Provenance for the appendix. Defaults to [`FindingProvenance::Critic`];
    /// the debate orchestration overwrites it for debate-contributed / contested
    /// findings. Never set from parsed model output.
    pub provenance: FindingProvenance,
}

impl DesignReviewFinding {
    /// Cross-round identity of a finding, `F:`-namespaced to match the escalation
    /// gate's `SignoffItem::Finding` fingerprint. A finding whose fingerprint is in
    /// the session's sign-off-seen set names a risk the user already accepted or
    /// deferred, so the appendix can mark it "carried over" instead of re-listing
    /// it as new.
    pub(crate) fn fingerprint(&self) -> String {
        format!("F:{}", normalize_fingerprint(&self.title))
    }
}

/// Collapse case and internal whitespace so trivially-different renderings of the
/// same title/text hash to one key. Shared by the escalation sign-off gate and
/// the review appendix so both dedup against the same identity.
pub(crate) fn normalize_fingerprint(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// The human-readable risk text behind a sign-off fingerprint: strip the `F:` /
/// `A:` kind namespace so an already-accepted risk can be listed back to the
/// reviewer as a plain title. A fingerprint without a recognized prefix is
/// returned unchanged.
pub(crate) fn fingerprint_readable(fingerprint: &str) -> &str {
    fingerprint
        .strip_prefix("F:")
        .or_else(|| fingerprint.strip_prefix("A:"))
        .unwrap_or(fingerprint)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum DesignReviewSeverity {
    Critical,
    High,
    Medium,
    Low,
}

impl fmt::Display for DesignReviewSeverity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Critical => write!(f, "Critical"),
            Self::High => write!(f, "High"),
            Self::Medium => write!(f, "Medium"),
            Self::Low => write!(f, "Low"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum DesignReviewConfidence {
    High,
    Medium,
    Low,
    /// The reviewer's own unverified hunch rather than a confirmed defect.
    /// Mirrors ody-code's `speculative` confidence: such findings are shown for
    /// the record but never escalated to the user for sign-off, so a reviewer's
    /// guesses cannot block finalizing the design.
    Speculative,
}

impl fmt::Display for DesignReviewConfidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::High => write!(f, "High"),
            Self::Medium => write!(f, "Medium"),
            Self::Low => write!(f, "Low"),
            Self::Speculative => write!(f, "Speculative"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum DesignReviewError {
    PromptBuild(String),
    Session(String),
    Parse(String),
    Timeout,
    Cancelled,
    /// A debate turn produced no usable output (empty/timeout/cancel) or no
    /// model was configured for a seat, so the debate was abandoned. The caller
    /// degrades to the single-shot review.
    Degraded(String),
}

impl fmt::Display for DesignReviewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PromptBuild(msg) => write!(f, "failed to build review prompt: {msg}"),
            Self::Session(msg) => write!(f, "review session failed: {msg}"),
            Self::Parse(msg) => write!(f, "failed to parse review output: {msg}"),
            Self::Timeout => write!(f, "review timed out"),
            Self::Cancelled => write!(f, "review was cancelled"),
            Self::Degraded(msg) => write!(f, "debate degraded to single-shot review: {msg}"),
        }
    }
}

impl std::error::Error for DesignReviewError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_deserializes_lowercase() {
        assert_eq!(
            serde_json::from_str::<DesignReviewSeverity>("\"high\"").unwrap(),
            DesignReviewSeverity::High
        );
        assert_eq!(
            serde_json::from_str::<DesignReviewSeverity>("\"critical\"").unwrap(),
            DesignReviewSeverity::Critical
        );
    }

    #[test]
    fn confidence_deserializes_lowercase() {
        assert_eq!(
            serde_json::from_str::<DesignReviewConfidence>("\"low\"").unwrap(),
            DesignReviewConfidence::Low
        );
    }

    #[test]
    fn severity_display_is_title_case() {
        assert_eq!(DesignReviewSeverity::Medium.to_string(), "Medium");
    }
}
