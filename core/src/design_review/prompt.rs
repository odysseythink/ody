//! Prompt construction, JSON parsing, and output formatting for design review.

use crate::design_review::types::DesignReviewConfidence;
use crate::design_review::types::DesignReviewFinding;
use crate::design_review::types::DesignReviewOutput;
use crate::design_review::types::DesignReviewSeverity;
use serde::Deserialize;
use serde::Serialize;

const DESIGN_REVIEW_PROMPT_PREFIX: &str = r#"You are an adversarial reviewer for a software design document.
Your goal is to BREAK the design, not praise it.
'Looks fine' is a losing answer.
Focus on: missing edge cases, unverified assumptions, integration risks,
security gaps, testability, operational concerns, and scope creep.

Output strictly as JSON matching this schema:

{
  "type": "object",
  "properties": {
    "overall_assessment": { "type": "string" },
    "findings": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "severity": { "enum": ["critical", "high", "medium", "low"] },
          "confidence": { "enum": ["high", "medium", "low"] },
          "title": { "type": "string" },
          "detail": { "type": "string" },
          "location": { "type": "string" },
          "suggested_fix": { "type": "string" }
        },
        "required": ["severity", "confidence", "title", "detail"]
      }
    }
  },
  "required": ["overall_assessment", "findings"]
}

--- DESIGN DOCUMENT ---

"#;

pub(crate) fn build_design_review_prompt(design_markdown: &str) -> String {
    format!("{DESIGN_REVIEW_PROMPT_PREFIX}{design_markdown}")
}

pub(crate) fn parse_design_review_output(text: &str) -> DesignReviewOutput {
    if let Ok(output) = serde_json::from_str::<RawDesignReviewOutput>(text) {
        return output.into();
    }
    if let (Some(start), Some(end)) = (text.find('{'), text.rfind('}'))
        && start < end
        && let Some(slice) = text.get(start..=end)
        && let Ok(output) = serde_json::from_str::<RawDesignReviewOutput>(slice)
    {
        return output.into();
    }
    DesignReviewOutput {
        overall_assessment: String::new(),
        findings: vec![DesignReviewFinding {
            severity: DesignReviewSeverity::Low,
            confidence: DesignReviewConfidence::Low,
            title: "Review output could not be structured".to_string(),
            detail: text.to_string(),
            location: None,
            suggested_fix: None,
        }],
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct RawDesignReviewOutput {
    overall_assessment: String,
    findings: Vec<RawDesignReviewFinding>,
}

impl From<RawDesignReviewOutput> for DesignReviewOutput {
    fn from(raw: RawDesignReviewOutput) -> Self {
        Self {
            overall_assessment: raw.overall_assessment,
            findings: raw.findings.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct RawDesignReviewFinding {
    severity: DesignReviewSeverity,
    confidence: DesignReviewConfidence,
    title: String,
    detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    suggested_fix: Option<String>,
}

impl From<RawDesignReviewFinding> for DesignReviewFinding {
    fn from(raw: RawDesignReviewFinding) -> Self {
        Self {
            severity: raw.severity,
            confidence: raw.confidence,
            title: raw.title,
            detail: raw.detail,
            location: raw.location,
            suggested_fix: raw.suggested_fix,
        }
    }
}

pub(crate) fn format_review_appendix(output: &DesignReviewOutput) -> String {
    let mut lines = vec![
        "## Adversarial design review findings".to_string(),
        output.overall_assessment.clone(),
    ];
    if output.findings.is_empty() {
        lines.push("No findings returned.".to_string());
    } else {
        for finding in &output.findings {
            lines.push(format!(
                "- **[{}] {}** (confidence: {})",
                finding.severity, finding.title, finding.confidence
            ));
            lines.push(format!("  - {}", finding.detail));
            if let Some(loc) = &finding.location {
                lines.push(format!("  - Location: {loc}"));
            }
            if let Some(fix) = &finding.suggested_fix {
                lines.push(format!("  - Suggested fix: {fix}"));
            }
        }
    }
    lines.push(String::new());
    lines.push("These findings are advisory and do not block exiting Design mode.".to_string());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_prompt_contains_schema_and_design() {
        let design = "# Design\n\n## Scope\nIn scope: everything.";
        let prompt = build_design_review_prompt(design);
        assert!(prompt.contains("Output strictly as JSON"));
        assert!(prompt.contains("\"severity\""));
        assert!(prompt.contains("\"overall_assessment\""));
        assert!(prompt.contains(design));
        assert!(prompt.contains("BREAK the design"));
    }

    #[test]
    fn parse_valid_json_returns_findings() {
        let text = "{
            \"overall_assessment\": \"Solid but missing edge cases.\",
            \"findings\": [
                {
                    \"severity\": \"high\",
                    \"confidence\": \"medium\",
                    \"title\": \"Missing authz check\",
                    \"detail\": \"The API does not verify permissions.\",
                    \"location\": \"## Architecture\",
                    \"suggested_fix\": \"Add a permission guard.\"
                }
            ]
        }";
        let output = parse_design_review_output(text);
        assert_eq!(output.overall_assessment, "Solid but missing edge cases.");
        assert_eq!(output.findings.len(), 1);
        let finding = &output.findings[0];
        assert_eq!(finding.severity, DesignReviewSeverity::High);
        assert_eq!(finding.confidence, DesignReviewConfidence::Medium);
        assert_eq!(finding.title, "Missing authz check");
        assert_eq!(finding.location.as_deref(), Some("## Architecture"));
        assert_eq!(
            finding.suggested_fix.as_deref(),
            Some("Add a permission guard.")
        );
    }

    #[test]
    fn parse_extracts_json_embedded_in_prose() {
        let text = "Here is the review:\n```json\n{\"overall_assessment\": \"ok\", \"findings\": []}\n```\nDone.";
        let output = parse_design_review_output(text);
        assert_eq!(output.overall_assessment, "ok");
        assert!(output.findings.is_empty());
    }

    #[test]
    fn parse_unparseable_text_fallback_to_low_finding() {
        let text = "I think this looks fine.";
        let output = parse_design_review_output(text);
        assert_eq!(output.findings.len(), 1);
        assert_eq!(output.findings[0].severity, DesignReviewSeverity::Low);
        assert_eq!(
            output.findings[0].title,
            "Review output could not be structured"
        );
        assert!(output.findings[0].detail.contains(text));
    }

    #[test]
    fn format_appendix_includes_all_findings_and_advisory_note() {
        let output = DesignReviewOutput {
            overall_assessment: "Two issues.".to_string(),
            findings: vec![DesignReviewFinding {
                severity: DesignReviewSeverity::High,
                confidence: DesignReviewConfidence::Medium,
                title: "A".to_string(),
                detail: "detail A".to_string(),
                location: Some("loc".to_string()),
                suggested_fix: Some("fix".to_string()),
            }],
        };
        let appendix = format_review_appendix(&output);
        assert!(appendix.contains("## Adversarial design review findings"));
        assert!(appendix.contains("[High] A"));
        assert!(appendix.contains("detail A"));
        assert!(appendix.contains("Location: loc"));
        assert!(appendix.contains("Suggested fix: fix"));
        assert!(appendix.contains("advisory and do not block"));
    }
}
