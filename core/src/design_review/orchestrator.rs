//! Orchestrates the adversarial design review sub-session.

use std::sync::Arc;
use std::time::Duration;

use ody_analytics::AnalyticsEventsClient;
use ody_analytics::DesignReviewCompletedInput;
use ody_analytics::DesignReviewFailedInput;
use ody_analytics::DesignReviewFailureReason;
use ody_analytics::DesignReviewStartedInput;
use ody_analytics::now_unix_millis;
use ody_protocol::model_metadata::ReasoningEffort;
use ody_protocol::protocol::EventMsg;
use ody_protocol::protocol::ReviewOutputEvent;
use ody_protocol::protocol::WarningEvent;
use ody_protocol::user_input::UserInput;
use tokio_util::sync::CancellationToken;

use crate::design_review::debate::orchestrator::DebateOrchestrator;
use crate::design_review::debate::types::DebateConfig;
use crate::design_review::prompt::Refutation;
use crate::design_review::prompt::build_design_review_prompt;
use crate::design_review::prompt::format_review_appendix;
use crate::design_review::prompt::parse_design_review_output;
use crate::design_review::types::DesignReviewConfidence;
use crate::design_review::types::DesignReviewError;
use crate::design_review::types::DesignReviewFinding;
use crate::design_review::types::DesignReviewOutput;
use crate::design_review::types::DesignReviewRequest;
use crate::design_review::types::DesignReviewSeverity;
use crate::design_review::types::FindingProvenance;
use crate::design_review::types::normalize_fingerprint;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use crate::tasks::SessionTaskContext;

// With the reviewer's reasoning stream disabled (see `run_one_shot_review` call
// below), the structured critique returns well inside this bound. Kept generous
// but far below the old 600s so a stalled reviewer no longer blocks finalize for
// ten minutes.
pub(crate) const DESIGN_REVIEW_TIMEOUT: Duration = Duration::from_secs(240);

/// v1.6b (D11): the standalone usability RECOMMENDATION call. One cheap one-shot
/// classification (no tools, reasoning stream off) whose result is shown to the
/// user as an advisory recommendation in `Ask` mode. On timeout/empty/unparseable
/// it defaults to `user_facing = true` (safe — the user still confirms).
pub(crate) async fn classify_usability(
    session: &Arc<Session>,
    turn: &Arc<TurnContext>,
    model: String,
    design_markdown: &str,
) -> crate::design_review::prompt::UsabilityRecommendation {
    let session_ctx = Arc::new(SessionTaskContext::new(
        Arc::clone(session),
        Arc::clone(&turn.extension_data),
    ));
    let prompt = crate::design_review::prompt::build_usability_recommendation_prompt(design_markdown);
    let input = vec![UserInput::Text {
        text: "Classify and return the JSON object.".to_string(),
        text_elements: Vec::new(),
    }];
    let cancellation_token = CancellationToken::new();
    let fut = crate::tasks::run_one_shot_review(
        session_ctx,
        Arc::clone(turn),
        input,
        cancellation_token.clone(),
        prompt,
        model,
        Some(ReasoningEffort::None),
    );
    match tokio::time::timeout(DESIGN_REVIEW_TIMEOUT, fut).await {
        Ok(Some(event)) => {
            crate::design_review::prompt::parse_usability_recommendation(&event.overall_explanation)
        }
        // Timeout or empty ⇒ safe default (user_facing = true) via empty-input parse.
        _ => {
            cancellation_token.cancel();
            crate::design_review::prompt::parse_usability_recommendation("")
        }
    }
}

pub(crate) struct DesignReviewOrchestrator;

impl DesignReviewOrchestrator {
    /// Run an adversarial review of a design markdown using the configured review model.
    /// Emits analytics events and returns structured findings or an error (does not block exit).
    pub(crate) async fn review(
        session: &Arc<Session>,
        turn: &Arc<TurnContext>,
        request: DesignReviewRequest,
    ) -> Result<DesignReviewOutput, DesignReviewError> {
        let started_at_ms = now_unix_millis();
        emit_started(
            session,
            &session.services.analytics_events_client,
            turn,
            &request,
            started_at_ms,
        );

        let session_ctx = Arc::new(SessionTaskContext::new(
            Arc::clone(session),
            Arc::clone(&turn.extension_data),
        ));
        let prompt = build_design_review_prompt(&request.design_markdown, &request.accepted_risks);
        let input = vec![UserInput::Text {
            text: "Review the design above and return JSON findings.".to_string(),
            text_elements: Vec::new(),
        }];
        let cancellation_token = CancellationToken::new();

        let review_future = crate::tasks::run_one_shot_review(
            session_ctx,
            Arc::clone(turn),
            input,
            cancellation_token.clone(),
            prompt,
            request.review_model.clone(),
            // Disable the reviewer's separate reasoning stream. The critique is a
            // single-shot structured task (emit JSON findings); a long streamed
            // reasoning trace is pure latency and the usual cause of the review
            // tripping its timeout (mirrors ody-code's reviewer `.withThinking('off')`).
            // The model can still reason inside its answer.
            Some(ReasoningEffort::None),
        );

        let review_result = tokio::time::timeout(DESIGN_REVIEW_TIMEOUT, review_future).await;
        let completed_at_ms = now_unix_millis();

        match review_result {
            Ok(Some(review_output)) => {
                let mut output = to_design_review_output(review_output);
                // Augment the single-shot critique with the bounded debate when
                // `[design_review.debate]` is enabled: union the Judge's findings
                // into the single-shot set (dedup by fingerprint). Any debate
                // failure keeps the single-shot output unchanged — the debate can
                // only ever add findings, never make finalize worse (design D3/C5).
                output = Self::augment_with_debate(session, turn, &request, output).await;
                // Parsing produced only the give-up fallback (output unparseable
                // even after salvage — usually a response cut off mid-JSON). Make
                // that visible instead of silently finalizing with no findings:
                // the fallback is Speculative, so it will not reach the sign-off
                // gate, and without this the user would see neither findings nor
                // any signal that the review did not structure.
                if crate::design_review::prompt::review_was_unstructured(&output) {
                    let message = if turn
                        .config
                        .language
                        .as_deref()
                        .is_some_and(|l| l.to_ascii_lowercase().starts_with("zh"))
                    {
                        "对抗性评审的输出无法结构化（可能被截断），本轮findings已跳过签核。"
                            .to_string()
                    } else {
                        "Adversarial review output could not be structured (likely truncated); its findings were skipped for sign-off this round.".to_string()
                    };
                    session
                        .send_event(turn.as_ref(), EventMsg::Warning(WarningEvent { message }))
                        .await;
                }
                emit_completed(
                    session,
                    &session.services.analytics_events_client,
                    turn,
                    &request,
                    &output,
                    started_at_ms,
                    completed_at_ms,
                );
                Ok(output)
            }
            Ok(None) => {
                emit_failed(
                    session,
                    &session.services.analytics_events_client,
                    turn,
                    &request,
                    DesignReviewFailureReason::Cancelled,
                    started_at_ms,
                    completed_at_ms,
                );
                Err(DesignReviewError::Cancelled)
            }
            Err(_) => {
                cancellation_token.cancel();
                emit_failed(
                    session,
                    &session.services.analytics_events_client,
                    turn,
                    &request,
                    DesignReviewFailureReason::Timeout,
                    started_at_ms,
                    completed_at_ms,
                );
                Err(DesignReviewError::Timeout)
            }
        }
    }

    /// When `[design_review.debate]` is enabled, run the bounded debate and
    /// union its findings into the single-shot `output` (dedup by fingerprint).
    /// Disabled ⇒ `output` returned unchanged. Debate failure ⇒ `output`
    /// unchanged plus a non-fatal warning (degrade, never block).
    async fn augment_with_debate(
        session: &Arc<Session>,
        turn: &Arc<TurnContext>,
        request: &DesignReviewRequest,
        mut output: DesignReviewOutput,
    ) -> DesignReviewOutput {
        let Some(mut cfg) = DebateConfig::from_config(turn.config.as_ref()) else {
            return output;
        };
        // v1.6b (D11): the handler resolved the authoritative usability decision
        // (On ⇒ true, Off ⇒ false, Ask ⇒ the user's answer). Apply it — this
        // overrides the config-derived default and recomputes the turn budget.
        cfg.set_append_usability_turn(request.run_usability_pass);
        // Seed the debate with the critic's findings (v1.5a): the debate targets
        // the gap and returns net-new findings. `output` is kept verbatim — the
        // borrow ends before `union_findings` consumes it below.
        match DebateOrchestrator::run(session, turn, request, &cfg, &output.findings).await {
            Ok(debate) => {
                // v1.5b (opt-in): mark critic findings the Judge refuted as
                // Contested — retained, downgraded to Speculative, never deleted.
                if cfg.contest_critic {
                    apply_refutations(&mut output, &debate.refutations);
                }
                union_findings(output, debate.findings)
            }
            Err(err) => {
                let message = if turn
                    .config
                    .language
                    .as_deref()
                    .is_some_and(|l| l.to_ascii_lowercase().starts_with("zh"))
                {
                    "设计辩论未完成，已回退到单发对抗性评审。".to_string()
                } else {
                    format!("Design debate did not complete ({err}); using the single-shot review.")
                };
                session
                    .send_event(turn.as_ref(), EventMsg::Warning(WarningEvent { message }))
                    .await;
                output
            }
        }
    }
}

/// Merge debate findings into the single-shot set, de-duplicated by
/// `DesignReviewFinding::fingerprint()` (the same identity the cross-round
/// sign-off gate uses). Single-shot (critic) findings win ties and are never
/// dropped. Each net-new debate finding is stamped `FindingProvenance::Debate` so
/// the appendix tags it "_(via debate)_" (Task 4 — replaces the earlier
/// assessment-line note with a per-finding marker).
fn union_findings(mut base: DesignReviewOutput, debate: DesignReviewOutput) -> DesignReviewOutput {
    let mut seen: std::collections::HashSet<String> =
        base.findings.iter().map(|f| f.fingerprint()).collect();
    for mut finding in debate.findings {
        if seen.insert(finding.fingerprint()) {
            finding.provenance = FindingProvenance::Debate;
            base.findings.push(finding);
        }
    }
    base
}

/// v1.5b: mark each critic finding the Judge refuted as `Contested` — downgraded
/// to `Speculative` (so the sign-off gate never surfaces it as an item to
/// accept/defer/fix) and annotated with the Judge's reason. Findings are NEVER
/// removed, so even an over-eager Judge cannot make finalize lose a critic finding
/// (design D8 / R8 mitigation). Matched by the same fingerprint the union and
/// sign-off gate use, so a title the Judge reworded simply doesn't match and the
/// finding stays upheld (the safe direction).
fn apply_refutations(output: &mut DesignReviewOutput, refutations: &[Refutation]) {
    if refutations.is_empty() {
        return;
    }
    let by_fp: std::collections::HashMap<String, &str> = refutations
        .iter()
        .map(|r| (format!("F:{}", normalize_fingerprint(&r.title)), r.reason.as_str()))
        .collect();
    for f in &mut output.findings {
        if let Some(reason) = by_fp.get(&f.fingerprint()) {
            f.provenance = FindingProvenance::Contested;
            f.confidence = DesignReviewConfidence::Speculative;
            let reason = reason.trim();
            if !reason.is_empty() {
                f.detail = format!("{} [Debate contested: {reason}]", f.detail);
            }
        }
    }
}

pub(crate) fn format_review_appendix_for_submit(
    output: &DesignReviewOutput,
    seen: &std::collections::HashSet<String>,
    chinese: bool,
) -> String {
    format_review_appendix(output, seen, chinese)
}

fn to_design_review_output(review_output: ReviewOutputEvent) -> DesignReviewOutput {
    // Prefer our own schema if the model followed it.
    parse_design_review_output(&review_output.overall_explanation)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct SeverityCounts {
    pub(crate) critical: usize,
    pub(crate) high: usize,
    pub(crate) medium: usize,
    pub(crate) low: usize,
}

pub(crate) fn severity_counts(findings: &[DesignReviewFinding]) -> SeverityCounts {
    let mut counts = SeverityCounts::default();
    for finding in findings {
        match finding.severity {
            DesignReviewSeverity::Critical => counts.critical += 1,
            DesignReviewSeverity::High => counts.high += 1,
            DesignReviewSeverity::Medium => counts.medium += 1,
            DesignReviewSeverity::Low => counts.low += 1,
        }
    }
    counts
}

fn emit_started(
    session: &Session,
    client: &AnalyticsEventsClient,
    turn: &TurnContext,
    request: &DesignReviewRequest,
    started_at_ms: u64,
) {
    client.track_design_review_started(DesignReviewStartedInput {
        thread_id: session.thread_id.to_string(),
        turn_id: turn.sub_id.clone(),
        review_model: request.review_model.clone(),
        started_at_ms,
    });
}

fn emit_completed(
    session: &Session,
    client: &AnalyticsEventsClient,
    turn: &TurnContext,
    request: &DesignReviewRequest,
    output: &DesignReviewOutput,
    started_at_ms: u64,
    completed_at_ms: u64,
) {
    let counts = severity_counts(&output.findings);
    client.track_design_review_completed(DesignReviewCompletedInput {
        thread_id: session.thread_id.to_string(),
        turn_id: turn.sub_id.clone(),
        review_model: request.review_model.clone(),
        finding_count: output.findings.len(),
        critical_count: counts.critical,
        high_count: counts.high,
        medium_count: counts.medium,
        low_count: counts.low,
        started_at_ms,
        completed_at_ms,
    });
}

fn emit_failed(
    session: &Session,
    client: &AnalyticsEventsClient,
    turn: &TurnContext,
    request: &DesignReviewRequest,
    reason: DesignReviewFailureReason,
    started_at_ms: u64,
    completed_at_ms: u64,
) {
    client.track_design_review_failed(DesignReviewFailedInput {
        thread_id: session.thread_id.to_string(),
        turn_id: turn.sub_id.clone(),
        review_model: request.review_model.clone(),
        reason,
        started_at_ms,
        completed_at_ms,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::design_review::types::DesignReviewConfidence;

    #[test]
    fn severity_counts_sum_findings() {
        let findings = vec![
            DesignReviewFinding {
                severity: DesignReviewSeverity::Critical,
                confidence: DesignReviewConfidence::High,
                title: "a".to_string(),
                detail: "d".to_string(),
                location: None,
                suggested_fix: None,
                provenance: FindingProvenance::Critic,
            },
            DesignReviewFinding {
                severity: DesignReviewSeverity::High,
                confidence: DesignReviewConfidence::High,
                title: "b".to_string(),
                detail: "d".to_string(),
                location: None,
                suggested_fix: None,
                provenance: FindingProvenance::Critic,
            },
            DesignReviewFinding {
                severity: DesignReviewSeverity::High,
                confidence: DesignReviewConfidence::High,
                title: "c".to_string(),
                detail: "d".to_string(),
                location: None,
                suggested_fix: None,
                provenance: FindingProvenance::Critic,
            },
        ];
        let counts = severity_counts(&findings);
        assert_eq!(counts.critical, 1);
        assert_eq!(counts.high, 2);
        assert_eq!(counts.medium, 0);
        assert_eq!(counts.low, 0);
    }

    fn finding(title: &str) -> DesignReviewFinding {
        DesignReviewFinding {
            severity: DesignReviewSeverity::High,
            confidence: DesignReviewConfidence::High,
            title: title.to_string(),
            detail: "d".to_string(),
            location: None,
            suggested_fix: None,
            provenance: FindingProvenance::Critic,
        }
    }

    fn output(assessment: &str, titles: &[&str]) -> DesignReviewOutput {
        DesignReviewOutput {
            overall_assessment: assessment.to_string(),
            findings: titles.iter().map(|t| finding(t)).collect(),
        }
    }

    #[test]
    fn union_dedups_overlapping_titles_by_fingerprint() {
        // "Auth Loss" vs "auth loss" hash to the same fingerprint (case/space
        // normalized), so the overlap collapses.
        let single = output("single", &["Concurrency gap", "Auth Loss"]);
        let debate = output("debate", &["auth loss", "Missing test"]);
        let merged = union_findings(single, debate);
        let titles: Vec<&str> = merged.findings.iter().map(|f| f.title.as_str()).collect();
        assert_eq!(titles, vec!["Concurrency gap", "Auth Loss", "Missing test"]);
        // Single-shot finding wins the tie (original casing preserved).
        assert!(merged.findings.iter().any(|f| f.title == "Auth Loss"));
        assert!(!merged.findings.iter().any(|f| f.title == "auth loss"));
        // Critic assessment retained unchanged (provenance is now per-finding).
        assert_eq!(merged.overall_assessment, "single");
        // Only the net-new debate finding is stamped Debate; critic findings stay Critic.
        let by = |t: &str| merged.findings.iter().find(|f| f.title == t).unwrap().provenance;
        assert_eq!(by("Missing test"), FindingProvenance::Debate);
        assert_eq!(by("Concurrency gap"), FindingProvenance::Critic);
        assert_eq!(by("Auth Loss"), FindingProvenance::Critic);
    }

    #[test]
    fn union_with_empty_debate_is_identity_on_findings() {
        let single = output("single", &["only"]);
        let merged = union_findings(single, output("", &[]));
        assert_eq!(merged.findings.len(), 1);
        assert_eq!(merged.overall_assessment, "single");
    }

    #[test]
    fn apply_refutations_contests_without_deleting() {
        let mut out = output("assessment", &["Real bug", "Bogus claim"]);
        let refs = vec![Refutation {
            title: "bogus claim".to_string(), // case-insensitive fingerprint match
            reason: "Advocate showed the guard already exists.".to_string(),
        }];
        apply_refutations(&mut out, &refs);
        // Nothing deleted — both findings survive.
        assert_eq!(out.findings.len(), 2);
        let bogus = out.findings.iter().find(|f| f.title == "Bogus claim").unwrap();
        // Contested: downgraded to Speculative (never escalates), tagged, annotated.
        assert_eq!(bogus.provenance, FindingProvenance::Contested);
        assert_eq!(bogus.confidence, DesignReviewConfidence::Speculative);
        assert!(bogus.detail.contains("[Debate contested: Advocate showed"));
        // The unrefuted finding is untouched.
        let real = out.findings.iter().find(|f| f.title == "Real bug").unwrap();
        assert_eq!(real.provenance, FindingProvenance::Critic);
        assert_eq!(real.confidence, DesignReviewConfidence::High);
    }

    #[test]
    fn apply_refutations_noop_when_empty() {
        let mut out = output("a", &["x"]);
        apply_refutations(&mut out, &[]);
        assert_eq!(out.findings[0].provenance, FindingProvenance::Critic);
    }
}
