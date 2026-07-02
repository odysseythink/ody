use super::super::protocol::RemoteControlPairingStatusRequest;
use super::super::protocol::StartRemoteControlPairingRequest;
use super::*;
use ody_login::AuthKeyringBackendKind;
use pretty_assertions::assert_eq;
use std::io;

fn remote_control_enrollment(
    remote_control_url: &str,
    remote_control_token: &str,
) -> RemoteControlEnrollment {
    RemoteControlEnrollment {
        remote_control_target: normalize_remote_control_url(remote_control_url)
            .expect("target should normalize"),
        account_id: "account-id".to_string(),
        environment_id: "environment-id".to_string(),
        server_id: "server-id".to_string(),
        server_name: "server-name".to_string(),
        remote_control_token: Some(remote_control_token.to_string()),
        expires_at: Some(
            OffsetDateTime::from_unix_timestamp(33_336_362_096)
                .expect("future timestamp should parse"),
        ),
    }
}

async fn pairing_error(status: &'static str, body: &'static str) -> (String, String) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let remote_control_url = remote_control_url_for_listener(&listener);
    let expected_pair_url = normalize_remote_control_url(&remote_control_url)
        .expect("target should normalize")
        .pair_url;
    let server_task = tokio::spawn(async move {
        let pairing_request = accept_http_request(&listener).await;
        respond_with_status_and_headers(
            pairing_request.stream,
            status,
            &[("x-request-id", "request-123"), ("cf-ray", "ray-123")],
            body,
        )
        .await;
    });

    let err = remote_control_enrollment(&remote_control_url, "remote-control-token")
        .start_pairing(StartRemoteControlPairingRequest { manual_code: false })
        .await
        .expect_err("pairing should fail");
    server_task.await.expect("server task should finish");
    (err.to_string(), expected_pair_url)
}

async fn pairing_response_error(body: serde_json::Value) -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let remote_control_url = remote_control_url_for_listener(&listener);
    let server_task = tokio::spawn(async move {
        let pairing_request = accept_http_request(&listener).await;
        respond_with_json(pairing_request.stream, body).await;
    });

    let err = remote_control_enrollment(&remote_control_url, "remote-control-token")
        .start_pairing(StartRemoteControlPairingRequest { manual_code: false })
        .await
        .expect_err("pairing should fail");
    server_task.await.expect("server task should finish");
    err.to_string()
}

async fn pairing_status_error(status: &'static str, body: &'static str) -> (io::Error, String) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let remote_control_url = remote_control_url_for_listener(&listener);
    let expected_status_url = normalize_remote_control_url(&remote_control_url)
        .expect("target should normalize")
        .pair_status_url;
    let server_task = tokio::spawn(async move {
        let status_request = accept_http_request(&listener).await;
        respond_with_status_and_headers(
            status_request.stream,
            status,
            &[("x-request-id", "request-123"), ("cf-ray", "ray-123")],
            body,
        )
        .await;
    });

    let err = remote_control_enrollment(&remote_control_url, "remote-control-token")
        .pairing_status(RemoteControlPairingStatusRequest {
            pairing_code: Some("pairing-code".to_string()),
            manual_pairing_code: None,
        })
        .await
        .expect_err("pairing status should fail");
    server_task.await.expect("server task should finish");
    (err, expected_status_url)
}


#[tokio::test]
async fn remote_control_pairing_status_returns_pending() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let remote_control_url = remote_control_url_for_listener(&listener);
    let server_task = tokio::spawn(async move {
        let status_request = accept_http_request(&listener).await;
        assert_eq!(
            status_request.request_line,
            "POST /backend-api/wham/remote/control/server/pair/status HTTP/1.1"
        );
        assert_eq!(
            status_request.headers.get("authorization"),
            Some(&"Bearer remote-control-token".to_string())
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&status_request.body)
                .expect("status request body should deserialize"),
            json!({ "pairing_code": "pairing-code" })
        );
        respond_with_json(status_request.stream, json!({ "claimed": false })).await;
    });

    let response = remote_control_enrollment(&remote_control_url, "remote-control-token")
        .pairing_status(RemoteControlPairingStatusRequest {
            pairing_code: Some("pairing-code".to_string()),
            manual_pairing_code: None,
        })
        .await
        .expect("pairing status should succeed");
    server_task.await.expect("server task should finish");

    assert!(!response.claimed);
}

#[tokio::test]
async fn remote_control_pairing_status_accepts_manual_pairing_code() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let remote_control_url = remote_control_url_for_listener(&listener);
    let server_task = tokio::spawn(async move {
        let status_request = accept_http_request(&listener).await;
        assert_eq!(
            status_request.request_line,
            "POST /backend-api/wham/remote/control/server/pair/status HTTP/1.1"
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&status_request.body)
                .expect("status request body should deserialize"),
            json!({ "manual_pairing_code": "ABCD-EFGH" })
        );
        respond_with_json(status_request.stream, json!({ "claimed": false })).await;
    });

    let response = remote_control_enrollment(&remote_control_url, "remote-control-token")
        .pairing_status(RemoteControlPairingStatusRequest {
            pairing_code: None,
            manual_pairing_code: Some("ABCD-EFGH".to_string()),
        })
        .await
        .expect("pairing status should succeed");
    server_task.await.expect("server task should finish");

    assert!(!response.claimed);
}

#[tokio::test]
async fn remote_control_pairing_status_returns_claimed() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let remote_control_url = remote_control_url_for_listener(&listener);
    let server_task = tokio::spawn(async move {
        let status_request = accept_http_request(&listener).await;
        assert_eq!(
            status_request.request_line,
            "POST /backend-api/wham/remote/control/server/pair/status HTTP/1.1"
        );
        respond_with_json(status_request.stream, json!({ "claimed": true })).await;
    });

    let response = remote_control_enrollment(&remote_control_url, "remote-control-token")
        .pairing_status(RemoteControlPairingStatusRequest {
            pairing_code: Some("pairing-code".to_string()),
            manual_pairing_code: None,
        })
        .await
        .expect("pairing status should succeed");
    server_task.await.expect("server task should finish");

    assert!(response.claimed);
}

#[tokio::test]
async fn remote_control_pairing_status_maps_user_actionable_backend_errors() {
    for (status, expected_kind) in [
        ("403 Forbidden", io::ErrorKind::PermissionDenied),
        ("404 Not Found", io::ErrorKind::InvalidInput),
        ("410 Gone", io::ErrorKind::InvalidInput),
    ] {
        let (err, _expected_status_url) = pairing_status_error(status, "not available").await;
        assert_eq!(err.kind(), expected_kind);
    }
}

#[tokio::test]
async fn remote_control_pairing_status_preserves_decode_error_context() {
    let (err, expected_status_url) = pairing_status_error("200 OK", "{").await;
    let err = err.to_string();

    assert!(err.contains(&format!(
        "failed to parse remote control pairing status response from `{expected_status_url}`: HTTP 200 OK"
    )));
    assert!(err.contains("request-id: request-123"));
    assert!(err.contains("cf-ray: ray-123"));
    assert!(err.contains("body: {"));
    assert!(err.contains("decode error:"));
}

#[tokio::test]
async fn start_remote_control_pairing_preserves_backend_error_context() {
    let (err, expected_pair_url) =
        pairing_error("503 Service Unavailable", "pairing unavailable").await;

    assert_eq!(
        err,
        format!(
            "remote control pairing failed at `{expected_pair_url}`: HTTP 503 Service Unavailable, request-id: request-123, cf-ray: ray-123, body: pairing unavailable"
        )
    );
}

#[tokio::test]
async fn start_remote_control_pairing_preserves_decode_error_context() {
    let (err, expected_pair_url) = pairing_error("200 OK", "{").await;
    assert!(err.contains(&format!(
        "failed to parse remote control pairing response from `{expected_pair_url}`: HTTP 200 OK"
    )));
    assert!(err.contains("request-id: request-123"));
    assert!(err.contains("cf-ray: ray-123"));
    assert!(err.contains("body: {"));
    assert!(err.contains("decode error:"));
}

#[tokio::test]
async fn start_remote_control_pairing_rejects_mismatched_backend_enrollment() {
    assert_eq!(
        pairing_response_error(json!({
            "pairing_code": "pairing-code",
            "manual_pairing_code": "ABCD-EFGH",
            "server_id": "other-server-id",
            "environment_id": "other-environment-id",
            "expires_at": "3026-05-22T12:34:56Z",
        }))
        .await,
        "remote control pairing returned mismatched enrollment: expected server_id=server-id, environment_id=environment-id; got server_id=other-server-id, environment_id=other-environment-id"
    );
}

#[tokio::test]
async fn start_remote_control_pairing_preserves_expiry_parse_error_context() {
    let err = pairing_response_error(json!({
        "pairing_code": "pairing-code",
        "manual_pairing_code": "ABCD-EFGH",
        "server_id": "server-id",
        "environment_id": "environment-id",
        "expires_at": "not-a-timestamp",
    }))
    .await;

    assert!(err.contains("failed to parse remote control pairing response"));
    assert!(err.contains("HTTP 200 OK"));
    assert!(err.contains("request-id: <none>"));
    assert!(err.contains("cf-ray: <none>"));
    assert!(err.contains("\"expires_at\":\"not-a-timestamp\""));
    assert!(err.contains("expires_at parse error:"));
}

#[tokio::test]
async fn remote_control_handle_disable_keeps_current_enrollment() {
    let remote_handle = remote_control_handle_with_current_enrollment(
        TEST_REMOTE_CONTROL_URL,
        remote_control_auth_manager(),
    );

    remote_handle
        .desired_state_tx
        .send_replace(RemoteControlDesiredState::Disabled);
    assert!(
        remote_handle.current_enrollment.lock().await.is_some(),
        "disabled remote control should keep the selected pairing server"
    );
}
