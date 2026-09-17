use ody_analytics::CompactionImplementation;
use ody_analytics::CompactionReason;
use ody_otel::SessionTelemetry;
use ody_protocol::error::OdyErr;
use tracing::warn;

/// Retries failures that may be model-specific and succeed with a different model.
pub(crate) fn should_retry_with_current_model(error: &OdyErr) -> bool {
    matches!(
        error,
        OdyErr::InvalidRequest(_)
            | OdyErr::UnexpectedStatus(_)
            | OdyErr::ContextWindowExceeded
            | OdyErr::UsageLimitReached(_)
            | OdyErr::ServerOverloaded
            | OdyErr::InternalServerError
            | OdyErr::RetryLimit(_)
    )
}

pub(crate) fn record_model_fallback(
    session_telemetry: &SessionTelemetry,
    previous_model: &str,
    current_model: &str,
    reason: CompactionReason,
    implementation: CompactionImplementation,
    fallback_error: Option<&OdyErr>,
) {
    let reason_tag = match reason {
        CompactionReason::UserRequested => "user_requested",
        CompactionReason::ContextLimit => "context_limit",
        CompactionReason::ModelDownshift => "model_downshift",
        CompactionReason::CompHashChanged => "comp_hash_changed",
        CompactionReason::PlanSplitCheckpoint => "plan_split_checkpoint",
        CompactionReason::TaskCheckpoint => "task_checkpoint",
    };
    let implementation_tag = match implementation {
        CompactionImplementation::Responses => "responses",
        CompactionImplementation::ResponsesCompactionV2 => "responses_compaction_v2",
        CompactionImplementation::ResponsesCompact => "responses_compact",
    };
    let outcome = if fallback_error.is_none() {
        "succeeded"
    } else {
        "failed"
    };
    session_telemetry.counter(
        "ody.compaction.model_fallback",
        /*inc*/ 1,
        &[
            ("reason", reason_tag),
            ("implementation", implementation_tag),
            ("outcome", outcome),
        ],
    );
    warn!(
        previous_model,
        current_model,
        ?reason,
        ?implementation,
        outcome,
        ?fallback_error,
        "previous-model compaction failed; retried with current model"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_retry_with_current_model_accepts_model_specific_failures() {
        for err in [
            OdyErr::InvalidRequest("bad".to_string()),
            OdyErr::ContextWindowExceeded,
            OdyErr::ServerOverloaded,
            OdyErr::InternalServerError,
        ] {
            assert!(
                should_retry_with_current_model(&err),
                "expected retryable: {err}"
            );
        }
    }

    #[test]
    fn should_retry_with_current_model_rejects_other_failures() {
        for err in [
            OdyErr::TurnAborted,
            OdyErr::Stream("disconnect".to_string(), None),
            OdyErr::Timeout,
        ] {
            assert!(
                !should_retry_with_current_model(&err),
                "expected non-retryable: {err}"
            );
        }
    }
}
