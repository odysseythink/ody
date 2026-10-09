use super::*;
use crate::session::tests::make_session_and_context;
use ody_protocol::retry_after::RetryAfter;
use tokio::time::Instant;

/// A retry with server advice waits until the advised deadline, even when the
/// error spent time in transit before reaching the retry loop.
#[tokio::test(start_paused = true)]
async fn stream_retry_waits_until_server_deadline() {
    let (session, turn_context) = make_session_and_context().await;
    let mut client_session = session.services.model_client.new_session();

    let advice = RetryAfter::from_delay(Duration::from_secs(10)).expect("valid advice");
    // Simulate 3s of error propagation before the retry loop sees the error.
    tokio::time::advance(Duration::from_secs(3)).await;

    let mut retries = 0;
    let err = OdyErr::Stream("disconnected".to_string(), Some(advice));
    handle_retryable_response_stream_error(
        &mut retries,
        /*max_retries*/ 2,
        err,
        &mut client_session,
        &session,
        &turn_context,
        ResponsesStreamRequest::Sampling,
    )
    .await
    .expect("retry should be allowed");

    assert_eq!(retries, 1);
    let resumed_at = Instant::now();
    // Tokio rounds timer deadlines up to the next millisecond.
    assert!(
        (advice.deadline()..=advice.deadline() + Duration::from_millis(1)).contains(&resumed_at),
        "retry resumed at {resumed_at:?}, expected deadline {:?}",
        advice.deadline(),
    );
}

/// Without server advice the retry falls back to local jittered backoff.
#[tokio::test(start_paused = true)]
async fn stream_retry_without_advice_uses_backoff() {
    let (session, turn_context) = make_session_and_context().await;
    let mut client_session = session.services.model_client.new_session();

    let mut retries = 0;
    let err = OdyErr::Stream("disconnected".to_string(), None);
    let start = Instant::now();
    handle_retryable_response_stream_error(
        &mut retries,
        /*max_retries*/ 2,
        err,
        &mut client_session,
        &session,
        &turn_context,
        ResponsesStreamRequest::Sampling,
    )
    .await
    .expect("retry should be allowed");

    assert_eq!(retries, 1);
    let waited = Instant::now() - start;
    // backoff(attempt 1) = 200ms with a 0.9..1.1 jitter.
    assert!(
        (Duration::from_millis(180)..=Duration::from_millis(221)).contains(&waited),
        "backoff wait was {waited:?}",
    );
}

/// Switching to the fallback transport must not bypass the server's retry deadline.
#[tokio::test(start_paused = true)]
async fn fallback_switch_waits_for_server_deadline() {
    let (session, turn_context) = make_session_and_context().await;
    let mut client_session = session.services.model_client.new_session();
    assert!(
        session.services.model_client.responses_websocket_enabled(),
        "test setup expects a websocket-capable provider"
    );

    let advice = RetryAfter::from_delay(Duration::from_secs(10)).expect("valid advice");

    let mut retries = 1;
    let err = OdyErr::Stream("disconnected".to_string(), Some(advice));
    handle_retryable_response_stream_error(
        &mut retries,
        /*max_retries*/ 1,
        err,
        &mut client_session,
        &session,
        &turn_context,
        ResponsesStreamRequest::Sampling,
    )
    .await
    .expect("fallback switch should allow the retry");

    assert_eq!(retries, 0);
    let resumed_at = Instant::now();
    assert!(
        (advice.deadline()..=advice.deadline() + Duration::from_millis(1)).contains(&resumed_at),
        "fallback resumed at {resumed_at:?}, expected deadline {:?}",
        advice.deadline(),
    );
}

/// With the retry budget exhausted and no fallback transport left, the original
/// error is returned.
#[tokio::test(start_paused = true)]
async fn exhausted_retries_without_fallback_return_error() {
    let (session, turn_context) = make_session_and_context().await;
    let mut client_session = session.services.model_client.new_session();
    // Consume the one-shot fallback switch so no fallback remains.
    assert!(
        client_session.try_switch_fallback_transport(
            &turn_context.session_telemetry,
            &turn_context.model_info,
        )
    );

    let mut retries = 1;
    let err = OdyErr::Stream("disconnected".to_string(), None);
    let result = handle_retryable_response_stream_error(
        &mut retries,
        /*max_retries*/ 1,
        err,
        &mut client_session,
        &session,
        &turn_context,
        ResponsesStreamRequest::Sampling,
    )
    .await;

    let Err(OdyErr::Stream(message, None)) = result else {
        panic!("expected the original stream error, got {result:?}");
    };
    assert_eq!(message, "disconnected");
}
