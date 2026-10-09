use super::*;
use crate::RetryAfter;
use crate::TransportError;
use http::Method;
use http::StatusCode;
use std::sync::Arc;
use std::sync::Mutex;
use tokio::time::Instant;

fn retryable_http_error(retry_after: Option<RetryAfter>) -> TransportError {
    TransportError::Http {
        status: StatusCode::TOO_MANY_REQUESTS,
        url: None,
        headers: None,
        body: None,
        retry_after,
    }
}

fn always_retry_policy() -> RetryPolicy {
    RetryPolicy {
        max_attempts: 3,
        base_delay: Duration::from_millis(200),
        retry_on: RetryOn {
            retry_429: true,
            retry_5xx: true,
            retry_transport: true,
        },
    }
}

fn make_request() -> Request {
    Request::new(Method::GET, "http://localhost/".to_string())
}

/// Server advice replaces local backoff: the retry waits until the advised
/// deadline captured at error receipt, so caller-side work between receipt and
/// the sleep does not extend the wait.
#[tokio::test(start_paused = true)]
async fn run_with_retry_prefers_server_retry_after_deadline() {
    // Simulate 3s of caller-side work after the server issued its advice.
    let advice = RetryAfter::from_delay(Duration::from_secs(10)).expect("valid advice");
    tokio::time::advance(Duration::from_secs(3)).await;

    let attempts = Arc::new(Mutex::new(Vec::new()));
    let attempts_in_op = Arc::clone(&attempts);
    let result = run_with_retry(
        always_retry_policy(),
        make_request,
        move |_req, _attempt| {
            let attempts = Arc::clone(&attempts_in_op);
            async move {
                attempts
                    .lock()
                    .expect("attempts poisoned")
                    .push(Instant::now());
                if attempts.lock().expect("attempts poisoned").len() == 1 {
                    Err(retryable_http_error(Some(advice)))
                } else {
                    Ok(())
                }
            }
        },
    )
    .await;

    result.expect("second attempt should succeed");
    let attempts = attempts.lock().expect("attempts poisoned");
    assert_eq!(attempts.len(), 2);
    // Tokio rounds timer deadlines up to the next millisecond.
    assert!(
        (advice.deadline()..=advice.deadline() + Duration::from_millis(1)).contains(&attempts[1]),
        "retry resumed at {:?}, expected deadline {:?}",
        attempts[1],
        advice.deadline(),
    );
}

/// Without server advice the retry falls back to jittered exponential backoff.
#[tokio::test(start_paused = true)]
async fn run_with_retry_without_advice_uses_backoff() {
    let attempts = Arc::new(Mutex::new(Vec::new()));
    let attempts_in_op = Arc::clone(&attempts);
    let result = run_with_retry(
        always_retry_policy(),
        make_request,
        move |_req, _attempt| {
            let attempts = Arc::clone(&attempts_in_op);
            async move {
                attempts
                    .lock()
                    .expect("attempts poisoned")
                    .push(Instant::now());
                if attempts.lock().expect("attempts poisoned").len() == 1 {
                    Err(retryable_http_error(None))
                } else {
                    Ok(())
                }
            }
        },
    )
    .await;

    result.expect("second attempt should succeed");
    let attempts = attempts.lock().expect("attempts poisoned");
    assert_eq!(attempts.len(), 2);
    let waited = attempts[1] - attempts[0];
    // backoff(base 200ms, attempt 1) applies a 0.9..1.1 jitter.
    assert!(
        (Duration::from_millis(180)..=Duration::from_millis(221)).contains(&waited),
        "backoff wait was {waited:?}",
    );
}
