//! Types for the design-mode adversarial review.

use std::fmt;
use serde::Deserialize;
use serde::Serialize;

/// Input to a design review.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DesignReviewRequest {
    pub design_markdown: String,
    pub review_model: String,
}

/// Structured output of a design review.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DesignReviewOutput {
    pub overall_assessment: String,
    pub findings: Vec<DesignReviewFinding>,
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
}

impl fmt::Display for DesignReviewConfidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::High => write!(f, "High"),
            Self::Medium => write!(f, "Medium"),
            Self::Low => write!(f, "Low"),
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
}

impl fmt::Display for DesignReviewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PromptBuild(msg) => write!(f, "failed to build review prompt: {msg}"),
            Self::Session(msg) => write!(f, "review session failed: {msg}"),
            Self::Parse(msg) => write!(f, "failed to parse review output: {msg}"),
            Self::Timeout => write!(f, "review timed out"),
            Self::Cancelled => write!(f, "review was cancelled"),
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
