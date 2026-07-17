//! Orchestrates the adversarial design review sub-session.

use std::sync::Arc;
use std::time::Duration;

use ody_analytics::AnalyticsEventsClient;
use ody_analytics::DesignReviewCompletedInput;
use ody_analytics::DesignReviewFailedInput;
use ody_analytics::DesignReviewFailureReason;
use ody_analytics::DesignReviewStartedInput;
use ody_analytics::now_unix_millis;
use ody_protocol::protocol::ReviewOutputEvent;
use ody_protocol::user_input::UserInput;
use tokio_util::sync::CancellationToken;

use crate::design_review::prompt::build_design_review_prompt;
use crate::design_review::prompt::format_review_appendix;
use crate::design_review::prompt::parse_design_review_output;
use crate::design_review::types::DesignReviewError;
use crate::design_review::types::DesignReviewFinding;
use crate::design_review::types::DesignReviewOutput;
use crate::design_review::types::DesignReviewRequest;
use crate::design_review::types::DesignReviewSeverity;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use crate::tasks::SessionTaskContext;

const DESIGN_REVIEW_TIMEOUT: Duration = Duration::from_secs(600);

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
        let prompt = build_design_review_prompt(&request.design_markdown);
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
        );

        let review_result = tokio::time::timeout(DESIGN_REVIEW_TIMEOUT, review_future).await;
        let completed_at_ms = now_unix_millis();

        match review_result {
            Ok(Some(review_output)) => {
                let output = to_design_review_output(review_output);
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
}

pub(crate) fn format_review_appendix_for_submit(output: &DesignReviewOutput) -> String {
    format_review_appendix(output)
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
            },
            DesignReviewFinding {
                severity: DesignReviewSeverity::High,
                confidence: DesignReviewConfidence::High,
                title: "b".to_string(),
                detail: "d".to_string(),
                location: None,
                suggested_fix: None,
            },
            DesignReviewFinding {
                severity: DesignReviewSeverity::High,
                confidence: DesignReviewConfidence::High,
                title: "c".to_string(),
                detail: "d".to_string(),
                location: None,
                suggested_fix: None,
            },
        ];
        let counts = severity_counts(&findings);
        assert_eq!(counts.critical, 1);
        assert_eq!(counts.high, 2);
        assert_eq!(counts.medium, 0);
        assert_eq!(counts.low, 0);
    }
}
