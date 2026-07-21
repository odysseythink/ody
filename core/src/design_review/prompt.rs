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
          "confidence": { "enum": ["high", "medium", "low", "speculative"] },
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

Use "speculative" confidence only when a finding is your own unverified hunch
rather than a defect you have confirmed against the design or the codebase.
Speculative findings are recorded but never put in front of the user for
sign-off, so do not use it to downgrade a real, confirmed problem.

Be concise so the full JSON is returned complete: keep each "detail" to at most
two or three sentences and each "suggested_fix" to one. Prefer the highest-signal
findings over an exhaustive list — a response that is cut off mid-JSON is unusable.

--- DESIGN DOCUMENT ---

"#;

pub(crate) fn build_design_review_prompt(design_markdown: &str) -> String {
    format!("{DESIGN_REVIEW_PROMPT_PREFIX}{design_markdown}")
}

/// Title of the give-up fallback finding synthesized when the reviewer's output
/// cannot be structured at all. Marked [`DesignReviewConfidence::Speculative`] so
/// the sign-off gate never escalates it — a raw, possibly-truncated JSON dump
/// must never be shown to the user as an item to accept/defer/fix.
pub(crate) const UNSTRUCTURED_REVIEW_TITLE: &str = "Review output could not be structured";

/// Whether `output` is the give-up fallback (the reviewer output could not be
/// parsed or salvaged). The orchestrator uses this to surface a visible warning
/// so the failure is not silent.
pub(crate) fn review_was_unstructured(output: &DesignReviewOutput) -> bool {
    output.findings.len() == 1
        && output.findings[0].title == UNSTRUCTURED_REVIEW_TITLE
        && output.findings[0].confidence == DesignReviewConfidence::Speculative
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
    // Best-effort recovery: a reviewer that hits the output-token cap mid-array
    // emits JSON that never closes, so both exact parses fail. Rather than lose
    // every real finding to the fallback below, salvage each COMPLETE finding
    // object before the truncation point.
    if let Some(output) = salvage_review_output(text) {
        return output;
    }
    // Give up. The fallback is Speculative so the sign-off gate never puts this
    // raw dump in front of the user; it stays as advisory text in the review
    // appendix, and the orchestrator emits a warning (see `review_was_unstructured`).
    // The detail is bounded so the appendix does not carry a giant broken blob.
    DesignReviewOutput {
        overall_assessment: String::new(),
        findings: vec![DesignReviewFinding {
            severity: DesignReviewSeverity::Low,
            confidence: DesignReviewConfidence::Speculative,
            title: UNSTRUCTURED_REVIEW_TITLE.to_string(),
            detail: bounded_detail(text, 400),
            location: None,
            suggested_fix: None,
        }],
    }
}

fn bounded_detail(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// Salvage complete finding objects from a truncated/malformed review JSON. Walks
/// the `findings` array element by element and keeps every object that parses,
/// stopping at the first that does not (the truncated tail). Returns `None` when
/// no complete finding could be recovered, so the caller falls through to the
/// unstructured fallback.
fn salvage_review_output(text: &str) -> Option<DesignReviewOutput> {
    let findings_key = text.find("\"findings\"")?;
    let arr_start = findings_key + text[findings_key..].find('[')?;
    let bytes = text.as_bytes();
    let mut i = arr_start + 1;
    let mut findings = Vec::new();
    while i < bytes.len() {
        match bytes[i] {
            b']' => break,
            b',' => i += 1,
            b if (b as char).is_whitespace() => i += 1,
            b'{' => {
                let Some(end) = find_object_end(bytes, i) else {
                    break; // object is truncated — stop, keep what we have
                };
                match serde_json::from_str::<RawDesignReviewFinding>(&text[i..end]) {
                    Ok(f) => findings.push(DesignReviewFinding::from(f)),
                    Err(_) => break,
                }
                i = end;
            }
            _ => break,
        }
    }
    if findings.is_empty() {
        return None;
    }
    let overall_assessment = extract_json_string_field(text, "overall_assessment").unwrap_or_default();
    Some(DesignReviewOutput {
        overall_assessment,
        findings,
    })
}

/// Given the index of an opening `{`, return the index just past its matching
/// `}`, honoring string literals and escapes. `None` if the object never closes
/// (truncated input).
fn find_object_end(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_str = false;
    let mut escaped = false;
    for (offset, &b) in bytes[open..].iter().enumerate() {
        if in_str {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_str = false;
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + offset + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// Extract a top-level JSON string field's value (unescaped) by scanning, so a
/// truncated document whose object won't fully parse can still yield its
/// `overall_assessment`. `None` if the key is absent or its value isn't a string.
fn extract_json_string_field(text: &str, key: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let key_pos = text.find(&format!("\"{key}\""))?;
    let mut i = key_pos + key.len() + 2;
    while i < bytes.len() && bytes[i] != b':' {
        i += 1;
    }
    i += 1;
    while i < bytes.len() && bytes[i] != b'"' {
        if !(bytes[i] as char).is_whitespace() {
            return None;
        }
        i += 1;
    }
    let start = i; // opening quote
    i += 1;
    let mut escaped = false;
    while i < bytes.len() {
        let b = bytes[i];
        if escaped {
            escaped = false;
        } else if b == b'\\' {
            escaped = true;
        } else if b == b'"' {
            // `start..=i` is a complete JSON string literal; let serde unescape it.
            return serde_json::from_str::<String>(&text[start..=i]).ok();
        }
        i += 1;
    }
    None
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
    lines.push(
        "The design has been persisted. The host will now present the next-step menu — do not stop here, and do not start implementing."
            .to_string(),
    );
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
    fn parse_unparseable_text_falls_back_to_nonescalating_finding() {
        let text = "I think this looks fine.";
        let output = parse_design_review_output(text);
        assert_eq!(output.findings.len(), 1);
        assert_eq!(output.findings[0].severity, DesignReviewSeverity::Low);
        // Crucially Speculative, so the sign-off gate never surfaces this raw dump
        // as an item to accept/defer/fix.
        assert_eq!(
            output.findings[0].confidence,
            DesignReviewConfidence::Speculative
        );
        assert_eq!(output.findings[0].title, UNSTRUCTURED_REVIEW_TITLE);
        assert!(output.findings[0].detail.contains(text));
        assert!(review_was_unstructured(&output));
    }

    #[test]
    fn salvage_recovers_complete_findings_from_truncated_json() {
        // A response cut off mid-array (the outer object and array never close,
        // and the last finding object is incomplete). Both exact parses fail; the
        // salvage path must recover the two complete findings and drop the partial
        // tail — never fall through to the unstructured dump.
        let text = "```json\n{\n  \"overall_assessment\": \"Missing edge cases, see below.\",\n  \"findings\": [\n    {\"severity\": \"critical\", \"confidence\": \"high\", \"title\": \"Overwrites user shell config\", \"detail\": \"No consent or undo.\"},\n    {\"severity\": \"high\", \"confidence\": \"medium\", \"title\": \"Hardcoded C: paths\", \"detail\": \"Ignores WOW64.\"},\n    {\"severity\": \"medium\", \"confidence\": \"low\", \"title\": \"Non-atomic writ";
        let output = parse_design_review_output(text);
        assert!(!review_was_unstructured(&output));
        assert_eq!(output.overall_assessment, "Missing edge cases, see below.");
        assert_eq!(output.findings.len(), 2);
        assert_eq!(output.findings[0].severity, DesignReviewSeverity::Critical);
        assert_eq!(output.findings[0].title, "Overwrites user shell config");
        assert_eq!(output.findings[1].title, "Hardcoded C: paths");
    }

    #[test]
    fn salvage_handles_braces_inside_string_values() {
        // A `}` inside a string value must not be mistaken for the object end.
        let text = "{\"overall_assessment\": \"ok\", \"findings\": [{\"severity\": \"low\", \"confidence\": \"low\", \"title\": \"brace } in title\", \"detail\": \"has { and } chars\"}], \"trailing garbage that breaks exact parse";
        let output = parse_design_review_output(text);
        assert!(!review_was_unstructured(&output));
        assert_eq!(output.findings.len(), 1);
        assert_eq!(output.findings[0].title, "brace } in title");
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
        assert!(appendix.contains("the next-step menu"));
        assert!(
            !appendix.contains("exiting Design mode"),
            "the appendix must not signal exit — the host drives the next step"
        );
    }
}
