use super::*;
use http::Method;
use http::StatusCode;
use std::time::Duration;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;

/// The transport captures `Retry-After` advice once, when the error response arrives.
#[tokio::test(start_paused = true)]
async fn execute_error_captures_retry_after_header() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "7")
                .set_body_string("slow down"),
        )
        .mount(&server)
        .await;

    let transport = ReqwestTransport::new(reqwest::Client::new());
    let err = transport
        .execute(Request::new(
            Method::GET,
            format!("{}/rate-limited", server.uri()),
        ))
        .await
        .expect_err("429 should surface as an error");

    let TransportError::Http {
        status,
        retry_after,
        ..
    } = err
    else {
        panic!("expected TransportError::Http");
    };
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        retry_after.map(|retry_after| retry_after.remaining_delay()),
        Some(Duration::from_secs(7)),
    );
}

/// Streaming error responses carry the same advice.
#[tokio::test(start_paused = true)]
async fn stream_error_captures_retry_after_header() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(503)
                .insert_header("retry-after", "3")
                .set_body_string("overloaded"),
        )
        .mount(&server)
        .await;

    let transport = ReqwestTransport::new(reqwest::Client::new());
    let err = match transport
        .stream(Request::new(
            Method::POST,
            format!("{}/responses", server.uri()),
        ))
        .await
    {
        Err(err) => err,
        Ok(_) => panic!("503 should surface as an error"),
    };

    let TransportError::Http {
        status,
        retry_after,
        ..
    } = err
    else {
        panic!("expected TransportError::Http");
    };
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        retry_after.map(|retry_after| retry_after.remaining_delay()),
        Some(Duration::from_secs(3)),
    );
}

/// Error responses without the header carry no advice.
#[tokio::test(start_paused = true)]
async fn error_without_retry_after_header_carries_no_advice() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;

    let transport = ReqwestTransport::new(reqwest::Client::new());
    let err = transport
        .execute(Request::new(
            Method::GET,
            format!("{}/broken", server.uri()),
        ))
        .await
        .expect_err("500 should surface as an error");

    let TransportError::Http { retry_after, .. } = err else {
        panic!("expected TransportError::Http");
    };
    assert_eq!(retry_after, None);
}
