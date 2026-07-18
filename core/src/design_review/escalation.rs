//! Audit-level-driven escalation of adversarial-review findings to the user.
//!
//! Ported from ody-code's `session-mode/reviewer.ts`: the Step 0 audit level the
//! user picks does not just tune how hard the model self-verifies — it decides
//! **which review findings must be put in front of the user for sign-off** before
//! the design can finalize. Stricter level → more findings escalate (monotonic).
//! This is what makes Design mode actively resolve disagreements instead of
//! dumping the findings as advisory text and stopping.

use crate::design_review::types::DesignReviewConfidence;
use crate::design_review::types::DesignReviewFinding;
use crate::design_review::types::DesignReviewOutput;
use crate::design_review::types::DesignReviewSeverity;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use ody_protocol::config_types::DesignAuditLevel;
use ody_protocol::request_user_input::RequestUserInputArgs;
use ody_protocol::request_user_input::RequestUserInputQuestion;
use ody_protocol::request_user_input::RequestUserInputQuestionOption;

/// Severities that escalate to the user at a given audit level. Monotonic: a
/// stricter level is a superset of a looser one. Mirrors ody-code's
/// `escalatedSeverities`, extended for ody-rs's extra `Critical` tier (which
/// escalates at every level).
fn escalated_severities(level: DesignAuditLevel) -> &'static [DesignReviewSeverity] {
    use DesignReviewSeverity::*;
    match level {
        DesignAuditLevel::Basic => &[Critical, High],
        DesignAuditLevel::Standard => &[Critical, High, Medium],
        DesignAuditLevel::Deep => &[Critical, High, Medium, Low],
    }
}

/// Whether a single finding should be escalated to the user for sign-off.
/// `Speculative` findings never escalate regardless of severity — the reviewer
/// flagged them as its own unverified hunches, so they must not block the
/// human sign-off path (mirrors ody-code's `shouldEscalate`).
fn should_escalate(
    severity: DesignReviewSeverity,
    confidence: DesignReviewConfidence,
    level: DesignAuditLevel,
) -> bool {
    escalated_severities(level).contains(&severity)
        && confidence != DesignReviewConfidence::Speculative
}

/// The findings that must be confirmed with the user at this level, ordered
/// most-severe first so the user sees the worst problems before the rest.
fn escalated_findings(
    findings: &[DesignReviewFinding],
    level: DesignAuditLevel,
) -> Vec<&DesignReviewFinding> {
    let mut out: Vec<&DesignReviewFinding> = findings
        .iter()
        .filter(|f| should_escalate(f.severity, f.confidence, level))
        .collect();
    out.sort_by_key(|f| severity_rank(f.severity));
    out
}

/// Whether the transcript language is Chinese, mirroring how core derives the
/// effective language for model instructions (explicit config value, else the
/// system locale).
fn transcript_is_chinese(language: Option<&str>) -> bool {
    let resolved = match language.map(str::trim).filter(|l| !l.is_empty()) {
        Some(l) if l.eq_ignore_ascii_case("auto") => {
            ody_config::locale::detect_system_locale_code()
        }
        Some(l) => ody_config::locale::parse_locale_code(l),
        None => ody_config::locale::detect_system_locale_code(),
    };
    resolved.as_deref() == Some("zh")
}

fn severity_rank(severity: DesignReviewSeverity) -> u8 {
    match severity {
        DesignReviewSeverity::Critical => 0,
        DesignReviewSeverity::High => 1,
        DesignReviewSeverity::Medium => 2,
        DesignReviewSeverity::Low => 3,
    }
}

/// The model's self-declared confidence in an assumption it recorded in the
/// mandatory `## Assumptions & Unverified Items` table. This is the enumerable
/// signal the host filters on: a *low*-confidence assumption is the riskiest
/// (most likely wrong), so it escalates first — the inverse of finding severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssumptionConfidence {
    High,
    Medium,
    Low,
}

impl AssumptionConfidence {
    /// Parse the `Confidence` cell. Anything not clearly `high`/`medium` is
    /// treated as `Low` — an unparseable confidence must surface, never be
    /// silently finalized past.
    fn parse(cell: &str) -> Self {
        match cell.trim().to_ascii_lowercase().as_str() {
            "high" => AssumptionConfidence::High,
            "medium" | "med" | "moderate" => AssumptionConfidence::Medium,
            _ => AssumptionConfidence::Low,
        }
    }
}

/// One row of the design's `## Assumptions & Unverified Items` table — an
/// unconfirmed inference the model made. These are the `[C:INFERRED]` decisions,
/// now surfaced through the same host-driven sign-off gate as review findings
/// instead of a separate model-driven prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Assumption {
    text: String,
    confidence: AssumptionConfidence,
    impact: String,
}

/// Whether an assumption escalates to the user at a given audit level. Monotonic
/// and parallel to `escalated_severities`, but keyed on confidence (low = riskiest):
/// Basic surfaces only low-confidence, Standard adds medium, Deep surfaces all.
fn assumption_escalates(confidence: AssumptionConfidence, level: DesignAuditLevel) -> bool {
    use AssumptionConfidence::*;
    match level {
        DesignAuditLevel::Basic => confidence == Low,
        DesignAuditLevel::Standard => confidence != High,
        DesignAuditLevel::Deep => true,
    }
}

/// Extract the `## Assumptions & Unverified Items` table from the design index
/// markdown. Mirrors `parse_parts_manifest`'s approach (the host already parses
/// the mandated `## Parts` table the same way), but preserves cell positions so
/// an empty middle cell does not shift the columns.
/// Columns: `# | Assumption | Confidence | Impact if wrong | How to verify`.
fn parse_assumptions(markdown: &str) -> Vec<Assumption> {
    let Some(heading_pos) = markdown.find("## Assumptions") else {
        return Vec::new();
    };
    let remainder = &markdown[heading_pos..];
    let section_end = remainder
        .find("\n## ")
        .map(|i| i + 1)
        .unwrap_or(remainder.len());
    let section = &remainder[..section_end];

    let lines: Vec<&str> = section.lines().collect();
    let mut table_start = None;
    for (i, line) in lines.iter().enumerate() {
        if line.trim().starts_with('|') && i + 1 < lines.len() && lines[i + 1].contains("---") {
            table_start = Some(i);
            break;
        }
    }
    let Some(table_start) = table_start else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for line in lines.iter().skip(table_start + 2) {
        let trimmed = line.trim();
        if !trimmed.starts_with('|') {
            break;
        }
        // Position-preserving split: strip the outer pipes, then split, so an
        // empty `How to verify` cell does not misalign `Confidence`/`Impact`.
        let cells: Vec<&str> = trimmed
            .trim_matches('|')
            .split('|')
            .map(str::trim)
            .collect();
        if cells.len() < 4 {
            continue;
        }
        if cells[0].parse::<usize>().is_err() {
            continue;
        }
        let text = cells[1].to_string();
        if text.is_empty() {
            continue;
        }
        out.push(Assumption {
            text,
            confidence: AssumptionConfidence::parse(cells[2]),
            impact: cells[3].to_string(),
        });
    }
    out
}

/// The assumptions that must be confirmed with the user at this level.
fn escalated_assumptions(assumptions: &[Assumption], level: DesignAuditLevel) -> Vec<&Assumption> {
    assumptions
        .iter()
        .filter(|a| assumption_escalates(a.confidence, level))
        .collect()
}

/// What the escalation gate decided about finalizing the design.
pub(crate) enum EscalationDecision {
    /// No escalation was needed, or the user accepted/deferred every escalated
    /// finding. `note` is an optional disposition summary to append.
    Finalize { note: Option<String> },
    /// The user asked to correct at least one finding. The design must NOT be
    /// finalized; `message` tells the model to stay in Design mode and revise.
    Revise { message: String },
}

const QUESTION_ID: &str = "design_review_signoff";

/// The two batch choices for the escalated findings, presented in ONE prompt so
/// the user signs off on all of them at once rather than answering N sequential
/// questions. Returns `(is_finalize, label, description)`.
fn batch_options(chinese: bool) -> [(bool, &'static str, &'static str); 2] {
    if chinese {
        [
            (
                true,
                "接受 / 推迟全部，完成设计",
                "认可这些问题（或记为实现阶段处理的已知风险），直接最终确定设计。",
            ),
            (
                false,
                "有要修正的，留在 Design 模式",
                "留下继续修改。可在备注（tab）里写明要改哪几条（按编号）。",
            ),
        ]
    } else {
        [
            (
                true,
                "Accept / defer all — finalize",
                "Acknowledge these (or log them as known risks for implementation) and finalize the design now.",
            ),
            (
                false,
                "Some need fixing — stay in Design",
                "Keep the design open to revise. Use notes (tab) to say which items, by number.",
            ),
        ]
    }
}

fn is_finalize_answer(chinese: bool, answer: &str) -> bool {
    batch_options(chinese)
        .into_iter()
        .any(|(is_finalize, label, _)| is_finalize && label == answer)
}

fn finding_lines(findings: &[&DesignReviewFinding]) -> String {
    findings
        .iter()
        .enumerate()
        .map(|(i, f)| {
            format!(
                "{}. [{}] {} — {}",
                i + 1,
                f.severity,
                f.title,
                truncate(&f.detail, 160)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn assumption_lines(chinese: bool, assumptions: &[&Assumption]) -> String {
    assumptions
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let impact = if a.impact.trim().is_empty() {
                String::new()
            } else if chinese {
                format!(" — 影响若错：{}", truncate(&a.impact, 120))
            } else {
                format!(" — impact if wrong: {}", truncate(&a.impact, 120))
            };
            format!("{}. {}{}", i + 1, truncate(&a.text, 160), impact)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The two-section body listing every escalated assumption and finding, with a
/// localized heading before each present section. Shared by the prompt and the
/// revise message so both show the same detail.
fn sections_text(
    chinese: bool,
    assumptions: &[&Assumption],
    findings: &[&DesignReviewFinding],
) -> String {
    let mut sections = Vec::new();
    if !assumptions.is_empty() {
        let head = if chinese {
            "设计中的推断假设（需你确认）："
        } else {
            "Inferred assumptions in the design:"
        };
        sections.push(format!("{head}\n{}", assumption_lines(chinese, assumptions)));
    }
    if !findings.is_empty() {
        let head = if chinese {
            "对抗审查发现："
        } else {
            "Adversarial-review findings:"
        };
        sections.push(format!("{head}\n{}", finding_lines(findings)));
    }
    sections.join("\n\n")
}

/// One batched prompt listing every escalated assumption AND finding in two
/// labeled sections, with a single accept-all vs revise choice. This is the
/// merged sign-off gate: what used to be a separate model-driven inferred-
/// decision prompt (Step 4.5) is now folded in here, host-driven and detailed.
fn build_signoff_prompt(
    chinese: bool,
    assumptions: &[&Assumption],
    findings: &[&DesignReviewFinding],
    level: DesignAuditLevel,
) -> RequestUserInputArgs {
    let body = sections_text(chinese, assumptions, findings);
    let total = assumptions.len() + findings.len();
    let question = if chinese {
        format!(
            "在 {level} 等级下，最终确定设计前需要你签核以下 {total} 项：\n\n{body}\n\n怎么处理？"
        )
    } else {
        format!(
            "Before finalizing at the {level} level, sign off on the following {total} item(s):\n\n{body}\n\nHow do you want to proceed?"
        )
    };
    let options = batch_options(chinese)
        .into_iter()
        .map(|(_, label, desc)| RequestUserInputQuestionOption {
            label: label.to_string(),
            description: desc.to_string(),
        })
        .collect();
    RequestUserInputArgs {
        questions: vec![RequestUserInputQuestion {
            id: QUESTION_ID.to_string(),
            header: if chinese { "签核确认" } else { "Sign-off" }.to_string(),
            question,
            is_other: false,
            is_secret: false,
            options: Some(options),
        }],
        auto_resolution_ms: None,
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// Message returned to the model when the user wants to revise. Lists every
/// escalated assumption and finding and appends any notes the user typed about
/// which to fix.
fn build_revise(
    chinese: bool,
    assumptions: &[&Assumption],
    findings: &[&DesignReviewFinding],
    notes: &[String],
) -> String {
    let list = sections_text(chinese, assumptions, findings);
    let notes_line = {
        let joined = notes
            .iter()
            .map(|n| n.trim())
            .filter(|n| !n.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        if joined.is_empty() {
            String::new()
        } else if chinese {
            format!("\n\n用户备注：{joined}")
        } else {
            format!("\n\nUser notes: {joined}")
        }
    };
    if chinese {
        format!(
            "设计未最终确定：你选择留在 Design 模式修正。需要处理：\n\n{list}{notes_line}\n\n修改设计后再用 `submit_design`（`final: true`）重新提交，不要开始实现。"
        )
    } else {
        format!(
            "Design NOT finalized: you chose to revise. Findings to address:\n\n{list}{notes_line}\n\nRevise the design, then resubmit with `submit_design` (`final: true`). Do not start implementing."
        )
    }
}

/// Run the level-driven sign-off gate. Presents ALL escalated assumptions AND
/// review findings in ONE prompt and returns whether the design may finalize.
/// This is the merged gate: the inferred-decision sign-off that used to be a
/// separate model-driven Step 4.5 prompt is now sourced from the design's
/// `## Assumptions & Unverified Items` table and shown here alongside the
/// adversarial findings. If nothing escalates, finalizes silently.
pub(crate) async fn run_escalation_gate(
    session: &Session,
    turn: &TurnContext,
    call_id: String,
    level: Option<DesignAuditLevel>,
    review: Option<&DesignReviewOutput>,
    design_markdown: &str,
) -> EscalationDecision {
    // Template default when the host selected no level: Basic.
    let level = level.unwrap_or(DesignAuditLevel::Basic);

    let all_assumptions = parse_assumptions(design_markdown);
    let assumptions = escalated_assumptions(&all_assumptions, level);
    let findings = match review {
        Some(review) => escalated_findings(&review.findings, level),
        None => Vec::new(),
    };
    if assumptions.is_empty() && findings.is_empty() {
        return EscalationDecision::Finalize { note: None };
    }

    let chinese = transcript_is_chinese(turn.config.language.as_deref());
    let args = build_signoff_prompt(chinese, &assumptions, &findings, level);
    let Some(response) = session.request_user_input(turn, call_id, args).await else {
        // Cancelled / no active turn — do not silently finalize past unresolved
        // critical findings; keep the design open for revision.
        let message = if chinese {
            "设计未最终确定：有事项需要你签核但未收到回应。留在 Design 模式。".to_string()
        } else {
            "Design NOT finalized: items need your sign-off but no response was received. Staying in Design mode.".to_string()
        };
        return EscalationDecision::Revise { message };
    };

    let answer = response.answers.get(QUESTION_ID);
    let primary = answer
        .and_then(|a| a.answers.first())
        .cloned()
        .unwrap_or_default();
    // Only the explicit "accept/defer all" choice finalizes. The revise option,
    // a free-text "Other", or an empty answer all keep the design open — never
    // finalize past unresolved findings by default.
    if is_finalize_answer(chinese, &primary) {
        EscalationDecision::Finalize { note: None }
    } else {
        let notes: Vec<String> = answer
            .map(|a| a.answers.iter().skip(1).cloned().collect())
            .unwrap_or_default();
        EscalationDecision::Revise {
            message: build_revise(chinese, &assumptions, &findings, &notes),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(
        severity: DesignReviewSeverity,
        confidence: DesignReviewConfidence,
        title: &str,
    ) -> DesignReviewFinding {
        DesignReviewFinding {
            severity,
            confidence,
            title: title.to_string(),
            detail: "detail".to_string(),
            location: None,
            suggested_fix: None,
        }
    }

    #[test]
    fn basic_escalates_only_critical_and_high() {
        use DesignReviewSeverity::*;
        assert_eq!(escalated_severities(DesignAuditLevel::Basic), &[Critical, High]);
        assert!(should_escalate(High, DesignReviewConfidence::High, DesignAuditLevel::Basic));
        assert!(!should_escalate(Medium, DesignReviewConfidence::High, DesignAuditLevel::Basic));
    }

    #[test]
    fn escalation_is_monotonic_with_level() {
        use DesignReviewSeverity::*;
        assert_eq!(escalated_severities(DesignAuditLevel::Standard), &[Critical, High, Medium]);
        assert_eq!(
            escalated_severities(DesignAuditLevel::Deep),
            &[Critical, High, Medium, Low]
        );
        // Medium escalates at Standard/Deep but not Basic.
        assert!(!should_escalate(Medium, DesignReviewConfidence::High, DesignAuditLevel::Basic));
        assert!(should_escalate(Medium, DesignReviewConfidence::High, DesignAuditLevel::Standard));
        // Low escalates only at Deep.
        assert!(!should_escalate(Low, DesignReviewConfidence::High, DesignAuditLevel::Standard));
        assert!(should_escalate(Low, DesignReviewConfidence::High, DesignAuditLevel::Deep));
    }

    #[test]
    fn speculative_never_escalates_even_at_deep() {
        use DesignReviewSeverity::*;
        assert!(!should_escalate(
            Critical,
            DesignReviewConfidence::Speculative,
            DesignAuditLevel::Deep
        ));
    }

    #[test]
    fn escalated_findings_ordered_by_severity() {
        use DesignReviewConfidence::High as C;
        use DesignReviewSeverity::*;
        let findings = vec![
            finding(Medium, C, "m"),
            finding(Critical, C, "c"),
            finding(High, C, "h"),
        ];
        let picked = escalated_findings(&findings, DesignAuditLevel::Standard);
        let titles: Vec<&str> = picked.iter().map(|f| f.title.as_str()).collect();
        assert_eq!(titles, vec!["c", "h", "m"]);
    }

    fn assumption(text: &str, confidence: AssumptionConfidence, impact: &str) -> Assumption {
        Assumption {
            text: text.to_string(),
            confidence,
            impact: impact.to_string(),
        }
    }

    #[test]
    fn prompt_is_one_batched_question_listing_findings_and_assumptions() {
        use DesignReviewConfidence::High as C;
        use DesignReviewSeverity::*;
        let f1 = finding(Critical, C, "plaintext key");
        let f2 = finding(High, C, "weak validation");
        let a1 = assumption("overlay reuse", AssumptionConfidence::Low, "rewrite UI");
        let f_refs = vec![&f1, &f2];
        let a_refs = vec![&a1];
        let menu = build_signoff_prompt(false, &a_refs, &f_refs, DesignAuditLevel::Standard);
        // One prompt, not one-per-item — the whole point of the batching/merge.
        assert_eq!(menu.questions.len(), 1);
        // Two batch choices (accept-all vs revise); the client auto-adds Other.
        assert_eq!(menu.questions[0].options.as_ref().unwrap().len(), 2);
        let q = &menu.questions[0].question;
        // Both sections appear in the single question's text.
        assert!(q.contains("plaintext key"));
        assert!(q.contains("weak validation"));
        assert!(q.contains("overlay reuse"));
        assert!(q.contains("rewrite UI")); // assumption impact is inlined
        assert!(q.contains("Standard"));
        assert!(q.contains("3 item")); // 2 findings + 1 assumption
    }

    #[test]
    fn assumption_escalation_is_monotonic_with_level() {
        use AssumptionConfidence::*;
        use DesignAuditLevel::*;
        // Basic: only low-confidence (riskiest) assumptions surface.
        assert!(assumption_escalates(Low, Basic));
        assert!(!assumption_escalates(Medium, Basic));
        assert!(!assumption_escalates(High, Basic));
        // Standard adds medium.
        assert!(assumption_escalates(Medium, Standard));
        assert!(!assumption_escalates(High, Standard));
        // Deep surfaces all, including high-confidence.
        assert!(assumption_escalates(High, Deep));
    }

    #[test]
    fn parse_assumptions_reads_the_mandated_table() {
        let md = "# Title\n\n## Assumptions & Unverified Items\n\
            | # | Assumption | Confidence | Impact if wrong | How to verify |\n\
            |---|---|---|---|---|\n\
            | 1 | plaintext API key storage | low | security boundary changes | read config |\n\
            | 2 | overlay reuse | high | UI rewrite | grep |\n\
            \n## Parts\n| # | File | Scope | Status |\n";
        let parsed = parse_assumptions(md);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].text, "plaintext API key storage");
        assert_eq!(parsed[0].confidence, AssumptionConfidence::Low);
        assert_eq!(parsed[0].impact, "security boundary changes");
        assert_eq!(parsed[1].confidence, AssumptionConfidence::High);
        // At Basic only the low-confidence row escalates; the ## Parts table
        // after it is not swept in.
        let escalated = escalated_assumptions(&parsed, DesignAuditLevel::Basic);
        assert_eq!(escalated.len(), 1);
        assert_eq!(escalated[0].text, "plaintext API key storage");
    }

    #[test]
    fn parse_assumptions_absent_table_is_empty() {
        assert!(parse_assumptions("# Title\n\nno table here\n").is_empty());
    }

    #[test]
    fn parse_assumptions_tolerates_empty_verify_cell() {
        // Empty trailing `How to verify` cell must not shift Confidence/Impact.
        let md = "## Assumptions\n\
            | # | Assumption | Confidence | Impact if wrong | How to verify |\n\
            |---|---|---|---|---|\n\
            | 1 | retry-on-failure | medium | worse UX |  |\n";
        let parsed = parse_assumptions(md);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].confidence, AssumptionConfidence::Medium);
        assert_eq!(parsed[0].impact, "worse UX");
    }

    #[test]
    fn transcript_language_detection() {
        assert!(transcript_is_chinese(Some("zh-CN")));
        assert!(transcript_is_chinese(Some("Chinese")));
        assert!(!transcript_is_chinese(Some("en-US")));
    }

    #[test]
    fn finalize_answer_finalizes() {
        let (_, label, _) = batch_options(false)[0];
        assert!(is_finalize_answer(false, label));
        assert!(!is_finalize_answer(false, batch_options(false)[1].1));
        assert!(!is_finalize_answer(false, "some free text"));
    }

    #[test]
    fn revise_message_lists_findings_assumptions_and_notes() {
        use DesignReviewConfidence::High as C;
        use DesignReviewSeverity::*;
        let f1 = finding(Critical, C, "plaintext key");
        let a1 = assumption("overlay reuse", AssumptionConfidence::Low, "rewrite UI");
        let f_refs = vec![&f1];
        let a_refs = vec![&a1];
        let msg = build_revise(false, &a_refs, &f_refs, &["fix items 1 and 2".to_string()]);
        assert!(msg.contains("NOT finalized"));
        assert!(msg.contains("plaintext key"));
        assert!(msg.contains("overlay reuse"));
        assert!(msg.contains("fix items 1 and 2"));
        assert!(msg.contains("Do not start implementing"));
    }

    #[test]
    fn revise_message_omits_notes_line_when_empty() {
        use DesignReviewConfidence::High as C;
        use DesignReviewSeverity::*;
        let f1 = finding(High, C, "weak validation");
        let refs = vec![&f1];
        let msg = build_revise(false, &[], &refs, &[String::new()]);
        assert!(!msg.contains("User notes"));
    }
}
