use super::*;
use crate::exec_output::StreamOutput;
use chrono::DateTime;
use chrono::Duration as ChronoDuration;
use chrono::TimeZone;
use chrono::Utc;
use http::Response as HttpResponse;
use pretty_assertions::assert_eq;
use reqwest::Response;
use reqwest::ResponseBuilderExt;
use reqwest::StatusCode;
use reqwest::Url;


fn with_now_override<T>(now: DateTime<Utc>, f: impl FnOnce() -> T) -> T {
    NOW_OVERRIDE.with(|cell| {
        *cell.borrow_mut() = Some(now);
        let result = f();
        *cell.borrow_mut() = None;
        result
    })
}


#[test]
fn server_overloaded_maps_to_protocol() {
    let err = OdyErr::ServerOverloaded;
    assert_eq!(err.to_ody_protocol_error(), OdyErrorInfo::ServerOverloaded);
}

#[test]
fn sandbox_denied_uses_aggregated_output_when_stderr_empty() {
    let output = ExecToolCallOutput {
        exit_code: 77,
        stdout: StreamOutput::new(String::new()),
        stderr: StreamOutput::new(String::new()),
        aggregated_output: StreamOutput::new("aggregate detail".to_string()),
        duration: Duration::from_millis(10),
        timed_out: false,
    };
    let err = OdyErr::Sandbox(SandboxErr::Denied {
        output: Box::new(output),
        network_policy_decision: None,
    });
    assert_eq!(get_error_message_ui(&err), "aggregate detail");
}

#[test]
fn sandbox_denied_reports_both_streams_when_available() {
    let output = ExecToolCallOutput {
        exit_code: 9,
        stdout: StreamOutput::new("stdout detail".to_string()),
        stderr: StreamOutput::new("stderr detail".to_string()),
        aggregated_output: StreamOutput::new(String::new()),
        duration: Duration::from_millis(10),
        timed_out: false,
    };
    let err = OdyErr::Sandbox(SandboxErr::Denied {
        output: Box::new(output),
        network_policy_decision: None,
    });
    assert_eq!(get_error_message_ui(&err), "stderr detail\nstdout detail");
}

#[test]
fn sandbox_denied_reports_stdout_when_no_stderr() {
    let output = ExecToolCallOutput {
        exit_code: 11,
        stdout: StreamOutput::new("stdout only".to_string()),
        stderr: StreamOutput::new(String::new()),
        aggregated_output: StreamOutput::new(String::new()),
        duration: Duration::from_millis(8),
        timed_out: false,
    };
    let err = OdyErr::Sandbox(SandboxErr::Denied {
        output: Box::new(output),
        network_policy_decision: None,
    });
    assert_eq!(get_error_message_ui(&err), "stdout only");
}

#[test]
fn to_error_event_handles_response_stream_failed() {
    let response = HttpResponse::builder()
        .status(StatusCode::TOO_MANY_REQUESTS)
        .url(Url::parse("http://example.com").unwrap())
        .body("")
        .unwrap();
    let source = Response::from(response).error_for_status_ref().unwrap_err();
    let err = OdyErr::ResponseStreamFailed(ResponseStreamFailed {
        source,
        request_id: Some("req-123".to_string()),
    });

    let event = err.to_error_event(Some("prefix".to_string()));

    assert_eq!(
        event.message,
        "prefix: Error while reading the server response: HTTP status client error (429 Too Many Requests) for url (http://example.com/), request id: req-123"
    );
    assert_eq!(
        event.ody_error_info,
        Some(OdyErrorInfo::ResponseStreamConnectionFailed {
            http_status_code: Some(429)
        })
    );
}

#[test]
fn sandbox_denied_reports_exit_code_when_no_output_available() {
    let output = ExecToolCallOutput {
        exit_code: 13,
        stdout: StreamOutput::new(String::new()),
        stderr: StreamOutput::new(String::new()),
        aggregated_output: StreamOutput::new(String::new()),
        duration: Duration::from_millis(5),
        timed_out: false,
    };
    let err = OdyErr::Sandbox(SandboxErr::Denied {
        output: Box::new(output),
        network_policy_decision: None,
    });
    assert_eq!(
        get_error_message_ui(&err),
        "command failed inside sandbox with exit code 13"
    );
}

#[test]
fn unexpected_status_non_html_is_unchanged() {
    let err = UnexpectedResponseError {
        status: StatusCode::FORBIDDEN,
        body: "plain text error".to_string(),
        user_message: None,
        url: Some("http://example.com/plain".to_string()),
        cf_ray: None,
        request_id: None,
        identity_authorization_error: None,
        identity_error_code: None,
    };
    let status = StatusCode::FORBIDDEN.to_string();
    let url = "http://example.com/plain";
    assert_eq!(
        err.to_string(),
        format!("unexpected status {status}: plain text error, url: {url}")
    );
}

#[test]
fn unexpected_status_uses_user_message_and_preserves_response_context() {
    let err = UnexpectedResponseError {
        status: StatusCode::UNAUTHORIZED,
        body: "provider-specific response".to_string(),
        user_message: Some("Provider-specific guidance".to_string()),
        url: Some("https://example.com/v1/responses".to_string()),
        cf_ray: None,
        request_id: Some("req-provider".to_string()),
        identity_authorization_error: None,
        identity_error_code: None,
    };

    assert_eq!(
        err.to_string(),
        "Provider-specific guidance, url: https://example.com/v1/responses, request id: req-provider"
    );
}

#[test]
fn unexpected_status_prefers_error_message_when_present() {
    let err = UnexpectedResponseError {
        status: StatusCode::UNAUTHORIZED,
        body: r#"{"error":{"message":"Workspace is not authorized in this region."},"status":401}"#
            .to_string(),
        user_message: None,
        url: Some("https://api.odysseythink.com/v1/responses".to_string()),
        cf_ray: None,
        request_id: Some("req-123".to_string()),
        identity_authorization_error: None,
        identity_error_code: None,
    };
    let status = StatusCode::UNAUTHORIZED.to_string();
    assert_eq!(
        err.to_string(),
        format!(
            "unexpected status {status}: Workspace is not authorized in this region., url: https://api.odysseythink.com/v1/responses, request id: req-123"
        )
    );
}

#[test]
fn unexpected_status_truncates_long_body_with_ellipsis() {
    let long_body = "x".repeat(UNEXPECTED_RESPONSE_BODY_MAX_BYTES + 10);
    let err = UnexpectedResponseError {
        status: StatusCode::BAD_GATEWAY,
        body: long_body,
        user_message: None,
        url: Some("http://example.com/long".to_string()),
        cf_ray: None,
        request_id: Some("req-long".to_string()),
        identity_authorization_error: None,
        identity_error_code: None,
    };
    let status = StatusCode::BAD_GATEWAY.to_string();
    let expected_body = format!("{}...", "x".repeat(UNEXPECTED_RESPONSE_BODY_MAX_BYTES));
    assert_eq!(
        err.to_string(),
        format!(
            "unexpected status {status}: {expected_body}, url: http://example.com/long, request id: req-long"
        )
    );
}

#[test]
fn unexpected_status_includes_cf_ray_and_request_id() {
    let err = UnexpectedResponseError {
        status: StatusCode::UNAUTHORIZED,
        body: "plain text error".to_string(),
        user_message: None,
        url: Some("https://api.odysseythink.com/v1/responses".to_string()),
        cf_ray: Some("9c81f9f18f2fa49d-LHR".to_string()),
        request_id: Some("req-xyz".to_string()),
        identity_authorization_error: None,
        identity_error_code: None,
    };
    let status = StatusCode::UNAUTHORIZED.to_string();
    assert_eq!(
        err.to_string(),
        format!(
            "unexpected status {status}: plain text error, url: https://api.odysseythink.com/v1/responses, cf-ray: 9c81f9f18f2fa49d-LHR, request id: req-xyz"
        )
    );
}

#[test]
fn unexpected_status_includes_identity_auth_details() {
    let err = UnexpectedResponseError {
        status: StatusCode::UNAUTHORIZED,
        body: "plain text error".to_string(),
        user_message: None,
        url: Some("https://api.odysseythink.com/v1/models".to_string()),
        cf_ray: Some("cf-ray-auth-401-test".to_string()),
        request_id: Some("req-auth".to_string()),
        identity_authorization_error: Some("missing_authorization_header".to_string()),
        identity_error_code: Some("token_expired".to_string()),
    };
    let status = StatusCode::UNAUTHORIZED.to_string();
    assert_eq!(
        err.to_string(),
        format!(
            "unexpected status {status}: plain text error, url: https://api.odysseythink.com/v1/models, cf-ray: cf-ray-auth-401-test, request id: req-auth, auth error: missing_authorization_header, auth error code: token_expired"
        )
    );
}

#[test]
fn empty_completion_stream_is_recognized() {
    let err = OdyErr::Stream("empty_completion: no output".to_string(), None);
    assert!(err.is_empty_completion());
}

#[test]
fn empty_completion_prefix_must_be_exact() {
    // Must-survive: messages that contain the substring but are NOT the provider marker.
    let err = OdyErr::Stream("not_empty_completion: ...".to_string(), None);
    assert!(!err.is_empty_completion());

    let err = OdyErr::Stream("empty_completion_log: ...".to_string(), None);
    assert!(!err.is_empty_completion());
}

#[test]
fn non_stream_errors_are_not_empty_completion() {
    assert!(!OdyErr::TurnAborted.is_empty_completion());
    assert!(!OdyErr::RequestTimeout.is_empty_completion());
    assert!(!OdyErr::ContextWindowExceeded.is_empty_completion());
}
