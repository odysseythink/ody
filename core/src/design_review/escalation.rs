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
/// `escalatedSeverities`, extended for ody's extra `Critical` tier (which
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

/// Per-item sign-off question ids are `{QUESTION_ID_PREFIX}{index}`, where index
/// matches the order items are queued (findings most-severe-first, then
/// assumptions). Aggregation maps each answer back to its item by this id.
const QUESTION_ID_PREFIX: &str = "design_review_signoff_";

/// One escalated item shown on its own sign-off page. Modelling findings and
/// assumptions uniformly lets the gate queue them as a single ordered list of
/// per-item questions and map every answer back to what it signed off.
#[derive(Clone, Copy)]
enum SignoffItem<'a> {
    Finding(&'a DesignReviewFinding),
    Assumption(&'a Assumption),
}

impl SignoffItem<'_> {
    /// Stable-enough identity for cross-round de-duplication. The stateless
    /// reviewer tends to reproduce the same finding title / assumption text when
    /// the underlying risk is unchanged, so a normalized title (findings) or text
    /// (assumptions) matches a risk the user already dispositioned in an earlier
    /// revise round. Namespaced by kind so a finding and an assumption that happen
    /// to share wording never collide. Not perfect — a reworded title re-escalates
    /// (fails safe: shows it again) rather than being wrongly hidden.
    fn fingerprint(&self) -> String {
        match self {
            SignoffItem::Finding(f) => format!("F:{}", normalize_fingerprint(&f.title)),
            SignoffItem::Assumption(a) => format!("A:{}", normalize_fingerprint(&a.text)),
        }
    }

    /// Short bracketed tag for the question header — severity for a finding,
    /// confidence for an assumption — so the user sees the stakes at a glance.
    fn tag(&self, chinese: bool) -> String {
        match self {
            SignoffItem::Finding(f) => format!("[{}]", f.severity),
            SignoffItem::Assumption(a) => {
                let c = match (a.confidence, chinese) {
                    (AssumptionConfidence::High, true) => "高把握假设",
                    (AssumptionConfidence::Medium, true) => "中把握假设",
                    (AssumptionConfidence::Low, true) => "低把握假设",
                    (AssumptionConfidence::High, false) => "assumption · high",
                    (AssumptionConfidence::Medium, false) => "assumption · med",
                    (AssumptionConfidence::Low, false) => "assumption · low",
                };
                format!("[{c}]")
            }
        }
    }

    /// The single item rendered as the question body (and reused verbatim as the
    /// revise-list bullet). One item per page, so this is never crammed against
    /// others — it can carry more detail than the old one-line-per-item list did.
    fn body(&self, chinese: bool) -> String {
        match self {
            SignoffItem::Finding(f) => format!("{} — {}", f.title, truncate(&f.detail, 300)),
            SignoffItem::Assumption(a) => {
                if a.impact.trim().is_empty() {
                    truncate(&a.text, 300)
                } else if chinese {
                    format!(
                        "{} — 影响若错：{}",
                        truncate(&a.text, 200),
                        truncate(&a.impact, 160)
                    )
                } else {
                    format!(
                        "{} — impact if wrong: {}",
                        truncate(&a.text, 200),
                        truncate(&a.impact, 160)
                    )
                }
            }
        }
    }
}

/// Queue findings (already sorted most-severe-first) then assumptions as one
/// ordered list. Index in this list is the question suffix, so the order here is
/// exactly the paging order the user walks with ←/→.
fn signoff_items<'a>(
    assumptions: &'a [&'a Assumption],
    findings: &'a [&'a DesignReviewFinding],
) -> Vec<SignoffItem<'a>> {
    findings
        .iter()
        .map(|f| SignoffItem::Finding(*f))
        .chain(assumptions.iter().map(|a| SignoffItem::Assumption(*a)))
        .collect()
}

/// The three per-item choices. Index 0 (Accept) is the highlighted default, so a
/// user who just presses through — or skips to the last page and submits —
/// finalizes; only an explicit "needs fixing" on an item blocks the design.
fn signoff_options(chinese: bool) -> Vec<RequestUserInputQuestionOption> {
    let triples = if chinese {
        [
            ("接受", "认可这条，记为可接受的已知风险。"),
            ("推迟到实现阶段", "先不改设计，留到实现阶段处理。"),
            ("需要修正", "这条要改：留在 Design 模式修正后再最终确定。"),
        ]
    } else {
        [
            ("Accept", "Acknowledge this as an acceptable known risk."),
            (
                "Defer to implementation",
                "Leave the design as-is; handle it during implementation.",
            ),
            (
                "Needs fixing",
                "This must change: stay in Design and revise before finalizing.",
            ),
        ]
    };
    triples
        .into_iter()
        .map(|(label, description)| RequestUserInputQuestionOption {
            label: label.to_string(),
            description: description.to_string(),
        })
        .collect()
}

/// The one label that blocks finalize. Everything else (Accept, Defer, a
/// free-text Other, or an unanswered page) is treated as non-blocking.
fn needs_fix_label(chinese: bool) -> &'static str {
    if chinese { "需要修正" } else { "Needs fixing" }
}

/// One `request_user_input` question per escalated item, so the TUI paginates
/// them (←/→) with a single digestible item per page instead of one giant
/// truncated list. Findings come first (most severe first), then assumptions;
/// index in `items` is the question-id suffix used to map answers back. The last
/// page submits every answer at once, and unanswered pages default to Accept.
fn build_signoff_questions(
    chinese: bool,
    items: &[SignoffItem<'_>],
    suppressed: usize,
) -> RequestUserInputArgs {
    let total = items.len();
    // Suffix that tells the user this round is smaller because already-confirmed
    // risks were carried over, so a shrinking count reads as progress rather than
    // "where did the rest go?".
    let skipped = if suppressed == 0 {
        String::new()
    } else if chinese {
        format!("，已略过 {suppressed} 项此前已确认")
    } else {
        format!(", {suppressed} already-confirmed skipped")
    };
    let questions = items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let progress = if chinese {
                format!("签核 {}/{total}", i + 1)
            } else {
                format!("Sign-off {}/{total}", i + 1)
            };
            let question = if chinese {
                format!(
                    "请签核此项（{progress}{skipped}）：\n\n{}",
                    item.body(chinese)
                )
            } else {
                format!(
                    "Sign off on this item ({progress}{skipped}):\n\n{}",
                    item.body(chinese)
                )
            };
            RequestUserInputQuestion {
                id: format!("{QUESTION_ID_PREFIX}{i}"),
                header: format!("{} {progress}", item.tag(chinese)),
                question,
                is_other: false,
                is_secret: false,
                options: Some(signoff_options(chinese)),
            }
        })
        .collect();
    RequestUserInputArgs {
        questions,
        auto_resolution_ms: None,
    }
}

/// Identity of a design across revise rounds: its normalized `# ` title. A
/// revise keeps the same title, so the sign-off memory carries over; a brand-new
/// design has a different title, so the memory resets and cannot leak stale
/// suppressions from a design that was abandoned without finalizing. Falls back
/// to the empty string when the design has no heading (the completeness gate
/// mandates one, so this is only a defensive default).
fn design_title_key(markdown: &str) -> String {
    markdown
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("# "))
        .map(normalize_fingerprint)
        .unwrap_or_default()
}

/// Collapse case and internal whitespace so trivially-different renderings of the
/// same title/text hash to one key.
fn normalize_fingerprint(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// Message returned to the model when the user flagged one or more items to fix.
/// Lists ONLY the flagged items (each with its own page note, if any) — not the
/// whole audit — so the model gets a tight, actionable revise list.
fn build_revise(chinese: bool, flagged: &[(String, String)]) -> String {
    let list = flagged
        .iter()
        .enumerate()
        .map(|(i, (body, note))| {
            let note = note.trim();
            if note.is_empty() {
                format!("{}. {body}", i + 1)
            } else if chinese {
                format!("{}. {body}\n   备注：{note}", i + 1)
            } else {
                format!("{}. {body}\n   note: {note}", i + 1)
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    if chinese {
        format!(
            "设计未最终确定：你标记了以下项需要修正。需要处理：\n\n{list}\n\n修改设计后再用 `submit_design`（`final: true`）重新提交，不要开始实现。"
        )
    } else {
        format!(
            "Design NOT finalized: you flagged the following to fix. Address:\n\n{list}\n\nRevise the design, then resubmit with `submit_design` (`final: true`). Do not start implementing."
        )
    }
}

/// Run the level-driven sign-off gate. Presents each escalated assumption AND
/// review finding on its own paginated page (←/→) and returns whether the design
/// may finalize. This is the merged gate: the inferred-decision sign-off that
/// used to be a separate model-driven Step 4.5 prompt is now sourced from the
/// design's `## Assumptions & Unverified Items` table and shown here alongside
/// the adversarial findings. If nothing escalates, finalizes silently.
///
/// Aggregation is opt-in-to-block: an item keeps the design open ONLY if its page
/// was explicitly answered "needs fixing". Accept, Defer, a free-text Other, or
/// an unanswered page all finalize — so pressing through (or skipping straight to
/// the last page and submitting) accepts everything, while flagging one item
/// stays in Design with just that item to fix.
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
        // Design finalizes with nothing to sign off — the effort is done, so a
        // future design starts from an empty suppression set.
        session.clear_design_signoff_seen().await;
        return EscalationDecision::Finalize { note: None };
    }

    let all_items = signoff_items(&assumptions, &findings);

    // Suppress items the user already dispositioned (accept/defer) in an earlier
    // revise round of THIS design: the stateless reviewer re-raises them every
    // round, and re-confirming an already-accepted risk is what made the count
    // feel like it never fell. Only the delta (new findings + items still flagged
    // for fixing) is put in front of the user.
    let seen = session
        .design_signoff_seen_for(design_title_key(design_markdown))
        .await;
    let items: Vec<SignoffItem<'_>> = all_items
        .iter()
        .copied()
        .filter(|item| !seen.contains(&item.fingerprint()))
        .collect();
    let suppressed = all_items.len() - items.len();

    if items.is_empty() {
        // Everything that escalated this round was already signed off — nothing
        // new to ask. Finalize and reset the per-design memory.
        session.clear_design_signoff_seen().await;
        return EscalationDecision::Finalize { note: None };
    }

    let chinese = transcript_is_chinese(turn.config.language.as_deref());
    let args = build_signoff_questions(chinese, &items, suppressed);
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

    let needs_fix = needs_fix_label(chinese);
    let mut flagged: Vec<(String, String)> = Vec::new();
    // Fingerprints the user actively dispositioned as non-blocking this round, so
    // a re-review after a revise does not surface them again. Only *explicit*
    // Accept / Defer / Other picks are remembered — an unanswered page is left out
    // so the user gets another look at it next round rather than it vanishing.
    let mut newly_seen: Vec<String> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let Some(answer) = response.answers.get(&format!("{QUESTION_ID_PREFIX}{i}")) else {
            continue;
        };
        let picked = answer.answers.first().map(String::as_str).unwrap_or_default();
        if picked == needs_fix {
            // The page's optional note is appended by the client as `user_note: …`.
            let note = answer
                .answers
                .iter()
                .find_map(|a| a.strip_prefix("user_note: "))
                .unwrap_or_default()
                .to_string();
            flagged.push((item.body(chinese), note));
        } else {
            newly_seen.push(item.fingerprint());
        }
    }

    if !newly_seen.is_empty() {
        session.record_design_signoff_seen(newly_seen).await;
    }

    if flagged.is_empty() {
        // Design finalizes — clear the per-design memory for the next design.
        session.clear_design_signoff_seen().await;
        EscalationDecision::Finalize { note: None }
    } else {
        EscalationDecision::Revise {
            message: build_revise(chinese, &flagged),
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
        assert_eq!(
            escalated_severities(DesignAuditLevel::Basic),
            &[Critical, High]
        );
        assert!(should_escalate(
            High,
            DesignReviewConfidence::High,
            DesignAuditLevel::Basic
        ));
        assert!(!should_escalate(
            Medium,
            DesignReviewConfidence::High,
            DesignAuditLevel::Basic
        ));
    }

    #[test]
    fn escalation_is_monotonic_with_level() {
        use DesignReviewSeverity::*;
        assert_eq!(
            escalated_severities(DesignAuditLevel::Standard),
            &[Critical, High, Medium]
        );
        assert_eq!(
            escalated_severities(DesignAuditLevel::Deep),
            &[Critical, High, Medium, Low]
        );
        // Medium escalates at Standard/Deep but not Basic.
        assert!(!should_escalate(
            Medium,
            DesignReviewConfidence::High,
            DesignAuditLevel::Basic
        ));
        assert!(should_escalate(
            Medium,
            DesignReviewConfidence::High,
            DesignAuditLevel::Standard
        ));
        // Low escalates only at Deep.
        assert!(!should_escalate(
            Low,
            DesignReviewConfidence::High,
            DesignAuditLevel::Standard
        ));
        assert!(should_escalate(
            Low,
            DesignReviewConfidence::High,
            DesignAuditLevel::Deep
        ));
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
    fn prompt_is_one_page_per_item_findings_first_then_assumptions() {
        use DesignReviewConfidence::High as C;
        use DesignReviewSeverity::*;
        let f1 = finding(Critical, C, "plaintext key");
        let f2 = finding(High, C, "weak validation");
        let a1 = assumption("overlay reuse", AssumptionConfidence::Low, "rewrite UI");
        let f_refs = vec![&f1, &f2];
        let a_refs = vec![&a1];
        let items = signoff_items(&a_refs, &f_refs);
        let menu = build_signoff_questions(false, &items, 0);
        // One page per escalated item — no more one giant truncated dump.
        assert_eq!(menu.questions.len(), 3);
        // Three per-item choices (Accept / Defer / Needs fixing); client adds Other.
        assert_eq!(menu.questions[0].options.as_ref().unwrap().len(), 3);
        // Findings come first (most severe first), assumptions last; each page
        // carries exactly its own item.
        assert!(menu.questions[0].question.contains("plaintext key"));
        assert!(menu.questions[0].header.contains("Critical"));
        assert!(menu.questions[0].header.contains("Sign-off 1/3"));
        assert!(menu.questions[1].question.contains("weak validation"));
        assert!(menu.questions[2].question.contains("overlay reuse"));
        assert!(menu.questions[2].question.contains("rewrite UI")); // impact inlined
        assert!(menu.questions[2].header.contains("assumption"));
        // Stable ids the aggregator maps answers back through.
        assert_eq!(menu.questions[0].id, "design_review_signoff_0");
        assert_eq!(menu.questions[2].id, "design_review_signoff_2");
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
    fn only_needs_fix_is_the_blocking_label() {
        // The one label that keeps the design open; the highlighted default
        // (index 0) is Accept, which finalizes.
        assert_eq!(needs_fix_label(false), "Needs fixing");
        assert_eq!(signoff_options(false)[0].label, "Accept");
        assert_ne!(signoff_options(false)[0].label, needs_fix_label(false));
        assert_eq!(needs_fix_label(true), "需要修正");
    }

    #[test]
    fn revise_message_lists_only_flagged_items_with_their_notes() {
        let flagged = vec![
            (
                "plaintext key — detail".to_string(),
                "encrypt it".to_string(),
            ),
            ("overlay reuse — impact if wrong: rewrite UI".to_string(), String::new()),
        ];
        let msg = build_revise(false, &flagged);
        assert!(msg.contains("NOT finalized"));
        assert!(msg.contains("1. plaintext key"));
        assert!(msg.contains("note: encrypt it"));
        assert!(msg.contains("2. overlay reuse"));
        assert!(msg.contains("Do not start implementing"));
    }

    #[test]
    fn fingerprint_is_kind_namespaced_and_normalized() {
        use DesignReviewConfidence::High as C;
        use DesignReviewSeverity::*;
        // Case + internal-whitespace differences collapse to the same key.
        let a = SignoffItem::Finding(&finding(High, C, "Missing  rate LIMITING"));
        let b = SignoffItem::Finding(&finding(Critical, C, "missing rate limiting"));
        assert_eq!(a.fingerprint(), b.fingerprint());
        // A finding and an assumption with identical wording never collide.
        let f = finding(High, C, "shared wording");
        let asm = assumption("shared wording", AssumptionConfidence::Low, "x");
        assert_ne!(
            SignoffItem::Finding(&f).fingerprint(),
            SignoffItem::Assumption(&asm).fingerprint()
        );
    }

    #[test]
    fn design_title_key_identifies_a_design_across_rounds() {
        // Same title (any case / spacing) → same key, so a revise round keeps the
        // sign-off memory; a different title → different key, so it resets.
        let r1 = "# Add   OAuth Login\n\nbody v1\n";
        let r2 = "#   add oauth login\n\nbody v2 (revised)\n";
        let other = "# Refactor cache layer\n";
        assert_eq!(design_title_key(r1), design_title_key(r2));
        assert_ne!(design_title_key(r1), design_title_key(other));
        // A `##` subheading is not mistaken for the title.
        assert_eq!(design_title_key("## Overview\n# Real Title\n"), "real title");
        // No heading → empty fallback, still stable.
        assert_eq!(design_title_key("no heading here\n"), "");
    }

    #[test]
    fn suppressed_note_only_appears_when_carrying_items_over() {
        use DesignReviewConfidence::High as C;
        use DesignReviewSeverity::*;
        let f = finding(Critical, C, "plaintext key");
        let f_refs = [&f];
        let items = signoff_items(&[], &f_refs);
        // No carry-over: no skipped note.
        let clean = build_signoff_questions(true, &items, 0);
        assert!(!clean.questions[0].question.contains("已略过"));
        // With carry-over: the shrinking count is explained on the page.
        let carried = build_signoff_questions(true, &items, 12);
        assert!(carried.questions[0].question.contains("已略过 12 项此前已确认"));
        // The English form too, and the short header chip stays free of it.
        let en = build_signoff_questions(false, &items, 3);
        assert!(en.questions[0].question.contains("3 already-confirmed skipped"));
        assert!(!en.questions[0].header.contains("already-confirmed"));
    }

    #[test]
    fn revise_message_omits_note_line_when_empty() {
        let msg = build_revise(false, &[("weak validation — detail".to_string(), String::new())]);
        assert!(msg.contains("1. weak validation"));
        assert!(!msg.contains("note:"));
    }
}
