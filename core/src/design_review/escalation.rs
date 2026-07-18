//! Audit-level-driven escalation of adversarial-review findings to the user.
//!
//! Ported from ody-code's `session-mode/reviewer.ts`: the Step 0 audit level the
//! user picks does not just tune how hard the model self-verifies — it decides
//! **which review findings must be put in front of the user for sign-off** before
//! the design can finalize. Stricter level → more findings escalate (monotonic).
//! This is what makes Design mode actively resolve disagreements instead of
//! dumping the findings as advisory text and stopping.

use crate::design_review::next_step::transcript_is_chinese;
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

fn severity_rank(severity: DesignReviewSeverity) -> u8 {
    match severity {
        DesignReviewSeverity::Critical => 0,
        DesignReviewSeverity::High => 1,
        DesignReviewSeverity::Medium => 2,
        DesignReviewSeverity::Low => 3,
    }
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

/// One batched prompt listing every escalated finding, with a single accept-all
/// vs revise choice.
fn build_escalation_menu(
    chinese: bool,
    findings: &[&DesignReviewFinding],
    level: DesignAuditLevel,
) -> RequestUserInputArgs {
    let list = finding_lines(findings);
    let question = if chinese {
        format!(
            "对抗性自审在 {level} 等级下发现 {n} 条需要你确认的问题：\n\n{list}\n\n怎么处理？",
            n = findings.len()
        )
    } else {
        format!(
            "The adversarial review surfaced {n} finding(s) needing your sign-off at the {level} level:\n\n{list}\n\nHow do you want to proceed?",
            n = findings.len()
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
            header: if chinese { "审查确认" } else { "Review sign-off" }.to_string(),
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
/// escalated finding and appends any notes the user typed about which to fix.
fn build_revise(chinese: bool, findings: &[&DesignReviewFinding], notes: &[String]) -> String {
    let list = finding_lines(findings);
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
            "设计未最终确定：你选择留在 Design 模式修正。需要处理的审查发现：\n\n{list}{notes_line}\n\n修改设计后再用 `submit_design`（`final: true`）重新提交，不要开始实现。"
        )
    } else {
        format!(
            "Design NOT finalized: you chose to revise. Findings to address:\n\n{list}{notes_line}\n\nRevise the design, then resubmit with `submit_design` (`final: true`). Do not start implementing."
        )
    }
}

/// Run the level-driven escalation gate. Presents ALL escalated findings in one
/// prompt and returns whether the design may finalize. If nothing escalates (or
/// the review produced no findings), finalizes silently.
pub(crate) async fn run_escalation_gate(
    session: &Session,
    turn: &TurnContext,
    call_id: String,
    level: Option<DesignAuditLevel>,
    review: Option<&DesignReviewOutput>,
) -> EscalationDecision {
    // Template default when the host selected no level: Basic.
    let level = level.unwrap_or(DesignAuditLevel::Basic);
    let Some(review) = review else {
        return EscalationDecision::Finalize { note: None };
    };
    let escalated = escalated_findings(&review.findings, level);
    if escalated.is_empty() {
        return EscalationDecision::Finalize { note: None };
    }

    let chinese = transcript_is_chinese(turn.config.language.as_deref());
    let args = build_escalation_menu(chinese, &escalated, level);
    let Some(response) = session.request_user_input(turn, call_id, args).await else {
        // Cancelled / no active turn — do not silently finalize past unresolved
        // critical findings; keep the design open for revision.
        let message = if chinese {
            "设计未最终确定：审查发现需要你确认但未收到回应。留在 Design 模式。".to_string()
        } else {
            "Design NOT finalized: escalated review findings need your sign-off but no response was received. Staying in Design mode.".to_string()
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
            message: build_revise(chinese, &escalated, &notes),
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

    #[test]
    fn menu_is_one_batched_question_listing_all_findings() {
        use DesignReviewConfidence::High as C;
        use DesignReviewSeverity::*;
        let f1 = finding(Critical, C, "plaintext key");
        let f2 = finding(High, C, "weak validation");
        let refs = vec![&f1, &f2];
        let menu = build_escalation_menu(false, &refs, DesignAuditLevel::Standard);
        // One prompt, not one-per-finding — the whole point of the batching fix.
        assert_eq!(menu.questions.len(), 1);
        // Two batch choices (accept-all vs revise); the client auto-adds Other.
        assert_eq!(menu.questions[0].options.as_ref().unwrap().len(), 2);
        // Every finding is listed in the single question's text.
        assert!(menu.questions[0].question.contains("plaintext key"));
        assert!(menu.questions[0].question.contains("weak validation"));
        assert!(menu.questions[0].question.contains("Standard"));
    }

    #[test]
    fn finalize_answer_finalizes() {
        let (_, label, _) = batch_options(false)[0];
        assert!(is_finalize_answer(false, label));
        assert!(!is_finalize_answer(false, batch_options(false)[1].1));
        assert!(!is_finalize_answer(false, "some free text"));
    }

    #[test]
    fn revise_message_lists_findings_and_notes() {
        use DesignReviewConfidence::High as C;
        use DesignReviewSeverity::*;
        let f1 = finding(Critical, C, "plaintext key");
        let refs = vec![&f1];
        let msg = build_revise(false, &refs, &["fix items 1 and 2".to_string()]);
        assert!(msg.contains("NOT finalized"));
        assert!(msg.contains("plaintext key"));
        assert!(msg.contains("fix items 1 and 2"));
        assert!(msg.contains("Do not start implementing"));
    }

    #[test]
    fn revise_message_omits_notes_line_when_empty() {
        use DesignReviewConfidence::High as C;
        use DesignReviewSeverity::*;
        let f1 = finding(High, C, "weak validation");
        let refs = vec![&f1];
        let msg = build_revise(false, &refs, &[String::new()]);
        assert!(!msg.contains("User notes"));
    }
}
