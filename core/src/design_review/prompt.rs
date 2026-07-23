//! Prompt construction, JSON parsing, and output formatting for design review.

use crate::design_review::types::DesignReviewConfidence;
use crate::design_review::types::DesignReviewFinding;
use crate::design_review::types::DesignReviewOutput;
use crate::design_review::types::DesignReviewSeverity;
use crate::design_review::types::fingerprint_readable;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashSet;

const DESIGN_REVIEW_INTRO: &str = r#"You are an adversarial reviewer for a software design document.
Your goal is to BREAK the design, not praise it.
'Looks fine' is a losing answer.
Focus on: missing edge cases, unverified assumptions, integration risks,
security gaps, testability, operational concerns, and scope creep.

"#;

/// The JSON output contract shared by the single-shot critic AND the debate
/// Judge, so both produce output the same parser (`parse_review_output` /
/// `review_was_unstructured`) consumes. Keep these two callers in lockstep — a
/// schema change here must serve both.
const FINDINGS_OUTPUT_SPEC: &str = r#"Output strictly as JSON matching this schema:

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

"#;

const DESIGN_DOCUMENT_MARKER: &str = "--- DESIGN DOCUMENT ---\n\n";

/// Build the reviewer prompt. `accepted_risks` are the sign-off fingerprints the
/// user already accepted/deferred in earlier rounds of this design; when present,
/// a block instructs the (otherwise stateless) reviewer not to re-raise them, so
/// the finding count converges across revise rounds instead of re-reporting risks
/// the user already dispositioned.
pub(crate) fn build_design_review_prompt(design_markdown: &str, accepted_risks: &[String]) -> String {
    let accepted_block = format_accepted_risks_block(accepted_risks);
    format!(
        "{DESIGN_REVIEW_INTRO}{FINDINGS_OUTPUT_SPEC}{accepted_block}{DESIGN_DOCUMENT_MARKER}{design_markdown}"
    )
}

// ---------------------------------------------------------------------------
// Debate prompts (Advocate / Skeptic / Judge). The Advocate and Skeptic emit
// argumentative prose; only the Judge emits the JSON findings contract, reusing
// `FINDINGS_OUTPUT_SPEC` so the existing parser applies unchanged. See the
// approved design `.ody-code/designs/2026-07-23-design-debate-mode.md` (C4/D6).
// ---------------------------------------------------------------------------

const DEBATE_TRANSCRIPT_MARKER: &str = "--- DEBATE TRANSCRIPT ---\n\n";

/// Directives common to both personas: single-sided argument, anti-conformity,
/// and (after round 0) the novelty gate. Borrowed from the Council of High
/// Intelligence protocol (see design Prior Art).
const PERSONA_COMMON_RULES: &str = r#"Rules:
- You argue ONE side only. Do NOT be balanced or even-handed.
- Engage the opponent's most recent turn directly; do not talk past it.
- ANTI-CONFORMITY: concede a point ONLY when you can name the specific flaw the
  opponent exposed in your argument. If you cannot name the flaw, do not concede.
- NOVELTY: if the transcript already contains earlier turns, this turn MUST add
  at least one NEW claim, attack, or piece of evidence. Restating your prior turn
  wastes the round.
Keep it to about 300 words. Prose only — do NOT emit JSON.

"#;

const ADVOCATE_INTRO: &str = r#"You are the ADVOCATE in a structured design debate.
Build the strongest evidence-based case that the design's load-bearing decisions
are correct and sufficient. Refute the Skeptic's latest attacks point by point.

"#;

const SKEPTIC_INTRO: &str = r#"You are the SKEPTIC in a structured design debate.
Attack the design by inversion: what would GUARANTEE it fails? Surface failure
modes, unstated assumptions, missing tests, concurrency/ordering hazards, resource
limits, and cheaper alternatives the design did not consider or defer honestly.

"#;

const JUDGE_INTRO: &str = r#"You are the JUDGE of a structured design debate. You did NOT participate; you
synthesize. You are given the design and a transcript of an Advocate defending it
and a Skeptic attacking it.

For each Skeptic attack the Advocate did NOT convincingly refute with concrete
evidence, emit ONE finding. Do NOT emit a finding for an attack the Advocate
soundly refuted. An attack the Advocate merely dented (not refuted) is a finding
with "speculative" confidence. Reserve an empty findings list for a design the
Skeptic genuinely could not dent.

"#;

/// Build the Advocate turn prompt. `transcript` is the debate so far (may be empty
/// on the opening turn).
pub(crate) fn build_advocate_prompt(design_markdown: &str, transcript: &str) -> String {
    build_persona_prompt(ADVOCATE_INTRO, design_markdown, transcript)
}

/// Build the Skeptic turn prompt.
pub(crate) fn build_skeptic_prompt(design_markdown: &str, transcript: &str) -> String {
    build_persona_prompt(SKEPTIC_INTRO, design_markdown, transcript)
}

fn build_persona_prompt(intro: &str, design_markdown: &str, transcript: &str) -> String {
    let transcript_block = if transcript.is_empty() {
        String::new()
    } else {
        format!("{DEBATE_TRANSCRIPT_MARKER}{transcript}\n\n")
    };
    format!(
        "{intro}{PERSONA_COMMON_RULES}{DESIGN_DOCUMENT_MARKER}{design_markdown}\n\n{transcript_block}"
    )
}

/// Build the Judge synthesis prompt. Reuses `FINDINGS_OUTPUT_SPEC` (same JSON the
/// single-shot critic emits) and the same accepted-risks carry-over block, so the
/// existing parser and cross-round dedup apply unchanged.
pub(crate) fn build_judge_prompt(
    design_markdown: &str,
    transcript: &str,
    accepted_risks: &[String],
) -> String {
    let accepted_block = format_accepted_risks_block(accepted_risks);
    format!(
        "{JUDGE_INTRO}{FINDINGS_OUTPUT_SPEC}{accepted_block}{DESIGN_DOCUMENT_MARKER}{design_markdown}\n\n{DEBATE_TRANSCRIPT_MARKER}{transcript}"
    )
}

/// The "previously accepted risks" preamble, or empty when there are none. Titles
/// are de-duplicated and stripped of their `F:`/`A:` kind prefix so the reviewer
/// sees plain risk descriptions.
fn format_accepted_risks_block(accepted_risks: &[String]) -> String {
    let mut seen = HashSet::new();
    let titles: Vec<&str> = accepted_risks
        .iter()
        .map(|f| fingerprint_readable(f))
        .filter(|t| !t.is_empty() && seen.insert(*t))
        .collect();
    if titles.is_empty() {
        return String::new();
    }
    let mut block = String::from(
        "--- PREVIOUSLY ACCEPTED RISKS (do NOT re-raise) ---\n\n\
The user has already reviewed this design across earlier rounds and explicitly \
ACCEPTED or DEFERRED the risks below. Do NOT report them again unless a change in \
the current design has genuinely reopened or worsened one of them. Report only NEW \
problems, or previously-raised problems the latest revision failed to fix.\n\n",
    );
    for title in titles {
        block.push_str("- ");
        block.push_str(title);
        block.push('\n');
    }
    block.push('\n');
    block
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

/// Render the findings for the tool-result appendix, splitting them into what is
/// NEW this round versus risks the user already accepted/deferred in an earlier
/// round (`seen` = the sign-off-seen fingerprints). Carried-over findings are
/// listed separately and annotated, and a one-line convergence summary is shown,
/// so a flat total reads as "mostly things you already signed off" rather than
/// "my design still has N problems" — and the reader no longer has to diff rounds
/// by hand to find the overlap.
pub(crate) fn format_review_appendix(
    output: &DesignReviewOutput,
    seen: &HashSet<String>,
    chinese: bool,
) -> String {
    let mut lines = vec!["## Adversarial design review findings".to_string()];
    if !output.overall_assessment.is_empty() {
        lines.push(output.overall_assessment.clone());
    }

    if output.findings.is_empty() {
        lines.push(if chinese {
            "本轮无 findings。".to_string()
        } else {
            "No findings returned.".to_string()
        });
    } else {
        let (carried, fresh): (Vec<&DesignReviewFinding>, Vec<&DesignReviewFinding>) = output
            .findings
            .iter()
            .partition(|f| seen.contains(&f.fingerprint()));

        // Convergence summary — only meaningful once at least one prior-round risk
        // is being carried over.
        if !carried.is_empty() {
            lines.push(if chinese {
                format!(
                    "本轮 {} 项新增，{} 项此前已确认（沿用，无需重复处理）。",
                    fresh.len(),
                    carried.len()
                )
            } else {
                format!(
                    "This round: {} new, {} previously accepted (carried over, no action needed).",
                    fresh.len(),
                    carried.len()
                )
            });
        }

        if !fresh.is_empty() {
            for finding in &fresh {
                push_finding(&mut lines, finding);
            }
        }
        if !carried.is_empty() {
            lines.push(String::new());
            lines.push(if chinese {
                "### 此前已确认的风险（沿用）".to_string()
            } else {
                "### Previously accepted risks (carried over)".to_string()
            });
            for finding in &carried {
                push_finding(&mut lines, finding);
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

fn push_finding(lines: &mut Vec<String>, finding: &DesignReviewFinding) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_prompt_contains_schema_and_design() {
        let design = "# Design\n\n## Scope\nIn scope: everything.";
        let prompt = build_design_review_prompt(design, &[]);
        assert!(prompt.contains("Output strictly as JSON"));
        assert!(prompt.contains("\"severity\""));
        assert!(prompt.contains("\"overall_assessment\""));
        assert!(prompt.contains(design));
        assert!(prompt.contains("BREAK the design"));
        // No accepted risks → no carry-over preamble at all.
        assert!(!prompt.contains("PREVIOUSLY ACCEPTED RISKS"));
    }

    #[test]
    fn single_shot_prompt_text_is_unchanged_by_the_spec_split() {
        // The single-shot critic prompt must remain byte-stable after splitting
        // the shared FINDINGS_OUTPUT_SPEC out of the old monolithic prefix.
        let design = "# D";
        let prompt = build_design_review_prompt(design, &[]);
        assert!(prompt.starts_with("You are an adversarial reviewer"));
        assert!(prompt.contains("BREAK the design"));
        assert!(prompt.contains("Output strictly as JSON"));
        assert!(prompt.ends_with("--- DESIGN DOCUMENT ---\n\n# D"));
    }

    #[test]
    fn judge_prompt_reuses_findings_schema_and_carries_transcript() {
        let prompt = build_judge_prompt("# Design", "[Advocate]: x\n\n[Skeptic]: y", &[]);
        // Same JSON contract as the single-shot critic → same parser applies.
        assert!(prompt.contains("Output strictly as JSON"));
        assert!(prompt.contains("\"overall_assessment\""));
        assert!(prompt.contains("\"severity\""));
        // Judge framing + transcript present; personas do not re-review from scratch.
        assert!(prompt.contains("You are the JUDGE"));
        assert!(prompt.contains("did NOT participate"));
        assert!(prompt.contains("--- DEBATE TRANSCRIPT ---"));
        assert!(prompt.contains("[Skeptic]: y"));
    }

    #[test]
    fn persona_prompts_carry_anti_conformity_and_novelty_and_no_json() {
        let skeptic = build_skeptic_prompt("# Design", "[Advocate]: opening");
        assert!(skeptic.contains("You are the SKEPTIC"));
        assert!(skeptic.contains("ANTI-CONFORMITY"));
        assert!(skeptic.contains("name the specific flaw"));
        assert!(skeptic.contains("NOVELTY"));
        assert!(skeptic.contains("--- DEBATE TRANSCRIPT ---"));
        // Personas argue in prose, never JSON.
        assert!(!skeptic.contains("Output strictly as JSON"));

        // Opening advocate turn has no transcript block.
        let advocate = build_advocate_prompt("# Design", "");
        assert!(advocate.contains("You are the ADVOCATE"));
        assert!(!advocate.contains("--- DEBATE TRANSCRIPT ---"));
        assert!(!advocate.contains("Output strictly as JSON"));
    }

    #[test]
    fn build_prompt_injects_accepted_risks_and_strips_kind_prefix() {
        let design = "# Design\n\n## Scope\nIn.";
        let accepted = vec![
            "F:missing rate limiting".to_string(),
            "A:overlay reuse".to_string(),
            // Duplicate readable title (different prefix collapse) is de-duped.
            "F:missing rate limiting".to_string(),
        ];
        let prompt = build_design_review_prompt(design, &accepted);
        assert!(prompt.contains("PREVIOUSLY ACCEPTED RISKS (do NOT re-raise)"));
        assert!(prompt.contains("- missing rate limiting"));
        assert!(prompt.contains("- overlay reuse"));
        // The `F:`/`A:` namespace never leaks into the prompt.
        assert!(!prompt.contains("F:missing"));
        assert!(!prompt.contains("A:overlay"));
        // De-dup: the repeated risk appears exactly once.
        assert_eq!(prompt.matches("- missing rate limiting").count(), 1);
        // The block precedes the design document.
        let block_pos = prompt.find("PREVIOUSLY ACCEPTED RISKS").unwrap();
        let doc_pos = prompt.find("--- DESIGN DOCUMENT ---").unwrap();
        assert!(block_pos < doc_pos);
        assert!(doc_pos < prompt.find(design).unwrap());
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
        let appendix = format_review_appendix(&output, &HashSet::new(), false);
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
        // With no seen set, nothing is carried over: no convergence line or section.
        assert!(!appendix.contains("carried over"));
        assert!(!appendix.contains("Previously accepted"));
    }

    fn finding(severity: DesignReviewSeverity, title: &str) -> DesignReviewFinding {
        DesignReviewFinding {
            severity,
            confidence: DesignReviewConfidence::High,
            title: title.to_string(),
            detail: format!("detail for {title}"),
            location: None,
            suggested_fix: None,
        }
    }

    #[test]
    fn format_appendix_splits_carried_over_from_new() {
        use DesignReviewSeverity::*;
        let output = DesignReviewOutput {
            overall_assessment: "assessment".to_string(),
            findings: vec![
                finding(Critical, "Plaintext key"),
                finding(High, "Missing rate limiting"),
                finding(Medium, "New this round"),
            ],
        };
        // "Missing rate limiting" was accepted last round (normalized, F:-namespaced).
        let mut seen = HashSet::new();
        seen.insert("F:missing rate limiting".to_string());
        // "Plaintext key" was NOT accepted — reworded titles fail safe as new.
        let appendix = format_review_appendix(&output, &seen, false);

        // Convergence summary reflects 2 new, 1 carried.
        assert!(appendix.contains("2 new, 1 previously accepted"));
        // Carried-over section exists and holds only the accepted risk.
        let carried_pos = appendix.find("### Previously accepted risks").unwrap();
        assert!(appendix[carried_pos..].contains("Missing rate limiting"));
        // The two new findings appear before the carried-over section.
        assert!(appendix.find("Plaintext key").unwrap() < carried_pos);
        assert!(appendix.find("New this round").unwrap() < carried_pos);
    }

    #[test]
    fn format_appendix_chinese_convergence_line() {
        use DesignReviewSeverity::*;
        let output = DesignReviewOutput {
            overall_assessment: String::new(),
            findings: vec![finding(High, "Known risk"), finding(Low, "Fresh one")],
        };
        let mut seen = HashSet::new();
        seen.insert("F:known risk".to_string());
        let appendix = format_review_appendix(&output, &seen, true);
        assert!(appendix.contains("1 项新增"));
        assert!(appendix.contains("1 项此前已确认"));
        assert!(appendix.contains("### 此前已确认的风险（沿用）"));
    }
}
