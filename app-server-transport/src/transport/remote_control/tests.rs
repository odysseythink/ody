use super::enroll::REMOTE_CONTROL_ACCOUNT_ID_HEADER;
use super::enroll::REMOTE_CONTROL_INSTALLATION_ID_HEADER;
use super::enroll::RemoteControlEnrollment;
use super::enroll::load_persisted_remote_control_enrollment;
use super::enroll::update_persisted_remote_control_enrollment;
use super::protocol::ClientEnvelope;
use super::protocol::ClientEvent;
use super::protocol::ClientId;
use super::protocol::StreamId;
use super::protocol::normalize_remote_control_url;
use super::websocket::REMOTE_CONTROL_PROTOCOL_VERSION;
use super::websocket::RemoteControlWebsocket;
use super::websocket::RemoteControlWebsocketConfig;
use super::*;
use crate::outgoing_message::OutgoingMessage;
use crate::outgoing_message::QueuedOutgoingMessage;
use crate::transport::CHANNEL_CAPACITY;
use crate::transport::ConnectionOrigin;
use crate::transport::TransportEvent;
use base64::Engine;
use ody_app_server_protocol::AuthMode;
use ody_app_server_protocol::ConfigWarningNotification;
use ody_app_server_protocol::JSONRPCMessage;
use ody_app_server_protocol::RemoteControlConnectionStatus;
use ody_app_server_protocol::RemoteControlPairingStartParams;
use ody_app_server_protocol::RemoteControlPairingStatusParams;
use ody_app_server_protocol::RemoteControlStatusChangedNotification;
use ody_app_server_protocol::ServerNotification;
use ody_config::types::AuthCredentialsStoreMode;
use ody_core::test_support::auth_manager_from_auth;
use ody_core::test_support::auth_manager_from_auth_with_home;
use ody_login::AuthDotJson;
use ody_login::AuthKeyringBackendKind;
use ody_login::AuthManager;
use ody_login::OdyAuth;
use ody_login::save_auth;

use ody_state::RemoteControlEnrollmentRecord;
use ody_state::StateRuntime;
use futures::SinkExt;
use futures::StreamExt;
use gethostname::gethostname;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;
use tempfile::TempDir;
use time::OffsetDateTime;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::sync::watch;
use tokio::time::Duration;
use tokio::time::timeout;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite;
use tokio_util::sync::CancellationToken;

mod pairing_tests;

const TEST_INSTALLATION_ID: &str = "11111111-1111-4111-8111-111111111111";
const TEST_REMOTE_CONTROL_URL: &str = "http://127.0.0.1:1/backend-api/wham/remote/control";
const TEST_REMOTE_CONTROL_SERVER_TOKEN: &str = "Remote Control Token";
const TEST_REFRESHED_REMOTE_CONTROL_SERVER_TOKEN: &str = "Refreshed Remote Control Token";
const TEST_REMOTE_CONTROL_SERVER_TOKEN_EXPIRES_AT: &str = "2999-01-01T00:00:00Z";

fn remote_control_auth_manager() -> Arc<AuthManager> {
    auth_manager_from_auth(OdyAuth::create_dummy_api_key_auth_for_testing())
}

fn remote_control_auth_manager_with_home(ody_home: &TempDir) -> Arc<AuthManager> {
    auth_manager_from_auth_with_home(
        OdyAuth::create_dummy_api_key_auth_for_testing(),
        ody_home.path().to_path_buf(),
    )
}

fn remote_control_auth_dot_json(_account_id: Option<&str>) -> AuthDotJson {
    AuthDotJson {
        auth_mode: Some(AuthMode::ApiKey),
        odysseythink_api_key: None,
    }
}

async fn remote_control_state_runtime(ody_home: &TempDir) -> Arc<StateRuntime> {
    StateRuntime::init(ody_home.path().to_path_buf(), "test-provider".to_string())
        .await
        .expect("state runtime should initialize")
}


#[tokio::test]
async fn explicit_disabled_start_ignores_persisted_enable() {
    let ody_home = TempDir::new().expect("temp dir should create");
    let state_db = remote_control_state_runtime(&ody_home).await;
    let remote_control_target = normalize_remote_control_url(TEST_REMOTE_CONTROL_URL)
        .expect("remote control target should normalize");
    let enrollment = RemoteControlEnrollmentRecord {
        websocket_url: remote_control_target.websocket_url,
        account_id: "account_id".to_string(),
        app_server_client_name: None,
        server_id: "server-id".to_string(),
        environment_id: "environment-id".to_string(),
        server_name: "server-name".to_string(),
        remote_control_enabled: Some(true),
    };
    state_db
        .upsert_remote_control_enrollment(&enrollment)
        .await
        .expect("enrollment should persist");
    let (transport_event_tx, _transport_event_rx) = mpsc::channel(CHANNEL_CAPACITY);
    let shutdown_token = CancellationToken::new();

    let (remote_task, remote_handle) = start_remote_control(
        RemoteControlStartConfig {
            remote_control_url: TEST_REMOTE_CONTROL_URL.to_string(),
            installation_id: TEST_INSTALLATION_ID.to_string(),
            policy: RemoteControlPolicy::Allowed,
        },
        Some(state_db.clone()),
        remote_control_auth_manager(),
        transport_event_tx,
        shutdown_token.clone(),
        /*app_server_client_name_rx*/ None,
        RemoteControlStartupMode::DisabledEphemeral,
    )
    .await
    .expect("remote control should start disabled");

    assert_eq!(
        *remote_handle.desired_state_tx.borrow(),
        RemoteControlDesiredState::Disabled
    );
    assert_eq!(
        state_db
            .get_remote_control_enrollment(
                &enrollment.websocket_url,
                &enrollment.account_id,
                /*app_server_client_name*/ None,
            )
            .await
            .expect("enrollment should load"),
        Some(enrollment)
    );

    shutdown_token.cancel();
    remote_task.await.expect("remote control task should join");
}

#[tokio::test]
async fn managed_disable_overrides_startup_and_persisted_enablement() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let remote_control_url = remote_control_url_for_listener(&listener);
    let ody_home = TempDir::new().expect("temp dir should create");
    let state_db = remote_control_state_runtime(&ody_home).await;
    let remote_control_target = normalize_remote_control_url(&remote_control_url)
        .expect("remote control target should normalize");
    let enrollment = RemoteControlEnrollmentRecord {
        websocket_url: remote_control_target.websocket_url,
        account_id: "account_id".to_string(),
        app_server_client_name: None,
        server_id: "server-id".to_string(),
        environment_id: "environment-id".to_string(),
        server_name: "server-name".to_string(),
        remote_control_enabled: Some(true),
    };
    state_db
        .upsert_remote_control_enrollment(&enrollment)
        .await
        .expect("enrollment should persist");
    let (transport_event_tx, _transport_event_rx) = mpsc::channel(CHANNEL_CAPACITY);
    let shutdown_token = CancellationToken::new();

    let (remote_task, remote_handle) = start_remote_control(
        RemoteControlStartConfig {
            remote_control_url,
            installation_id: TEST_INSTALLATION_ID.to_string(),
            policy: RemoteControlPolicy::DisabledByRequirements,
        },
        Some(state_db.clone()),
        remote_control_auth_manager(),
        transport_event_tx,
        shutdown_token.clone(),
        /*app_server_client_name_rx*/ None,
        RemoteControlStartupMode::EnabledEphemeral,
    )
    .await
    .expect("remote control should start disabled");

    assert_eq!(
        remote_handle.status().status,
        RemoteControlConnectionStatus::Disabled
    );
    assert_eq!(
        remote_handle.ensure_remote_control_allowed(),
        Err(RemoteControlDisabledByRequirements)
    );
    assert!(
        !remote_handle
            .resolve_persisted_preference(/*app_server_client_name*/ None)
            .await
            .expect("managed disable should resolve without loading persistence")
    );
    assert_eq!(
        remote_handle
            .enable_ephemeral()
            .expect_err("managed requirements should reject ephemeral enable"),
        RemoteControlEnableError::DisabledByRequirements(RemoteControlDisabledByRequirements)
    );
    let enable_error = remote_handle
        .enable(/*app_server_client_name*/ None)
        .await
        .expect_err("managed requirements should reject durable enable");
    assert_eq!(enable_error.kind(), std::io::ErrorKind::PermissionDenied);
    assert_eq!(
        enable_error.to_string(),
        "remote control is disabled by managed requirements"
    );
    let disable_error = remote_handle
        .disable(/*app_server_client_name*/ None)
        .await
        .expect_err("managed requirements should reject durable disable");
    assert_eq!(disable_error.kind(), std::io::ErrorKind::PermissionDenied);
    assert_eq!(
        disable_error.to_string(),
        "remote control is disabled by managed requirements"
    );
    assert_eq!(
        state_db
            .get_remote_control_enrollment(
                &enrollment.websocket_url,
                &enrollment.account_id,
                /*app_server_client_name*/ None,
            )
            .await
            .expect("enrollment should load"),
        Some(enrollment)
    );
    timeout(Duration::from_millis(100), listener.accept())
        .await
        .expect_err("managed requirements should prevent backend contact");

    shutdown_token.cancel();
    remote_task.await.expect("remote control task should join");
}

fn remote_control_url_for_listener(listener: &TcpListener) -> String {
    let addr = listener
        .local_addr()
        .expect("listener should have a local addr");
    format!("http://{addr}/backend-api/")
}

fn test_server_name() -> String {
    gethostname().to_string_lossy().trim().to_string()
}

fn remote_control_handle_with_current_enrollment(
    remote_control_url: &str,
    auth_manager: Arc<AuthManager>,
) -> RemoteControlHandle {
    let (desired_state_tx, _desired_state_rx) =
        watch::channel(RemoteControlDesiredState::Enabled {
            persistence_preference: None,
        });
    let (status_tx, _status_rx) = watch::channel(RemoteControlStatusChangedNotification {
        status: RemoteControlConnectionStatus::Connecting,
        server_name: test_server_name(),
        installation_id: TEST_INSTALLATION_ID.to_string(),
        environment_id: Some("env_test".to_string()),
    });
    let remote_control_target = normalize_remote_control_url(remote_control_url)
        .expect("remote control target should normalize");
    let current_enrollment = Arc::new(RemoteControlEnrollmentState::new(Some(
        RemoteControlEnrollment {
            remote_control_target,
            account_id: "account_id".to_string(),
            environment_id: "env_test".to_string(),
            server_id: "srv_e_test".to_string(),
            server_name: test_server_name(),
            remote_control_token: Some(TEST_REMOTE_CONTROL_SERVER_TOKEN.to_string()),
            expires_at: Some(
                OffsetDateTime::from_unix_timestamp(33_336_362_096)
                    .expect("future timestamp should parse"),
            ),
        },
    )));
    RemoteControlHandle {
        policy: RemoteControlPolicy::Allowed,
        desired_state_tx: Arc::new(desired_state_tx),
        desired_state_rpc_lock: Arc::new(Semaphore::new(1)),
        desired_state_persistence_lock: Arc::new(Semaphore::new(1)),
        status_tx: Arc::new(status_tx),
        state_db: None,
        remote_control_url: remote_control_url.to_string(),
        current_enrollment,
        pairing_persistence_key: watch::channel(None).0,
        pairing_persistence_key_required: false,
        auth_manager,
    }
}

#[tokio::test]
async fn ephemeral_enable_preserves_durable_preference() {
    let ody_home = TempDir::new().expect("temp dir should create");
    let mut remote_handle = remote_control_handle_with_current_enrollment(
        TEST_REMOTE_CONTROL_URL,
        remote_control_auth_manager(),
    );
    remote_handle.state_db = Some(remote_control_state_runtime(&ody_home).await);
    remote_handle
        .desired_state_tx
        .send_replace(RemoteControlDesiredState::Enabled {
            persistence_preference: Some(true),
        });

    remote_handle
        .enable_ephemeral()
        .expect("ephemeral enable should succeed");
    assert_eq!(
        *remote_handle.desired_state_tx.borrow(),
        RemoteControlDesiredState::Enabled {
            persistence_preference: Some(true),
        }
    );

    remote_handle
        .desired_state_tx
        .send_replace(RemoteControlDesiredState::Disabled);
    remote_handle
        .enable_ephemeral()
        .expect("ephemeral enable should succeed");
    assert_eq!(
        *remote_handle.desired_state_tx.borrow(),
        RemoteControlDesiredState::Enabled {
            persistence_preference: None,
        }
    );
}

fn remote_control_server_token_response(
    server_id: &str,
    environment_id: &str,
    remote_control_token: &str,
) -> serde_json::Value {
    json!({
        "server_id": server_id,
        "environment_id": environment_id,
        "remote_control_token": remote_control_token,
        "expires_at": TEST_REMOTE_CONTROL_SERVER_TOKEN_EXPIRES_AT,
    })
}

async fn expect_remote_control_status(
    status_rx: &mut watch::Receiver<RemoteControlStatusChangedNotification>,
    expected_status: Option<RemoteControlConnectionStatus>,
    expected_environment_id: Option<&str>,
) {
    timeout(Duration::from_secs(5), status_rx.changed())
        .await
        .expect("remote control status event should arrive in time")
        .expect("remote control status watch should remain open");
    let status = status_rx.borrow();
    if let Some(expected_status) = expected_status {
        assert_eq!(status.status, expected_status);
    }
    assert_eq!(status.server_name, test_server_name());
    assert_eq!(status.installation_id, TEST_INSTALLATION_ID);
    assert_eq!(status.environment_id.as_deref(), expected_environment_id);
}

async fn expect_remote_control_status_snapshot(
    status_rx: &mut watch::Receiver<RemoteControlStatusChangedNotification>,
    expected_status: RemoteControlStatusChangedNotification,
) {
    if *status_rx.borrow() == expected_status {
        return;
    }

    let expected_status_for_wait = expected_status.clone();
    let result = timeout(Duration::from_secs(5), async {
        loop {
            status_rx
                .changed()
                .await
                .expect("remote control status watch should remain open");
            if *status_rx.borrow() == expected_status_for_wait {
                return;
            }
        }
    })
    .await;
    assert!(
        result.is_ok(),
        "remote control status snapshot should arrive in time; expected {expected_status:?}, latest {:?}",
        status_rx.borrow().clone()
    );
}




#[tokio::test]
async fn remote_control_start_allows_remote_control_invalid_url_when_disabled() {
    let (transport_event_tx, _transport_event_rx) =
        mpsc::channel::<TransportEvent>(CHANNEL_CAPACITY);
    let shutdown_token = CancellationToken::new();
    let (remote_task, _remote_handle) = start_remote_control(
        RemoteControlStartConfig {
            remote_control_url: "https://internal.example.com/backend-api/".to_string(),
            installation_id: TEST_INSTALLATION_ID.to_string(),
            policy: RemoteControlPolicy::Allowed,
        },
        /*state_db*/ None,
        remote_control_auth_manager(),
        transport_event_tx,
        shutdown_token.clone(),
        /*app_server_client_name_rx*/ None,
        RemoteControlStartupMode::ResolvePersisted,
    )
    .await
    .expect("disabled remote control should not validate the URL at startup");

    shutdown_token.cancel();
    timeout(Duration::from_secs(1), remote_task)
        .await
        .expect("remote control task should stop")
        .expect("remote control task should join");
}

#[tokio::test]
async fn remote_control_start_allows_missing_auth_when_enabled() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let remote_control_url = remote_control_url_for_listener(&listener);
    let ody_home = TempDir::new().expect("temp dir should create");
    let auth_manager = AuthManager::shared(
        ody_home.path().to_path_buf(),
        /*enable_ody_api_key_env*/ false,
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
        /*auth_route_config*/ None,
    )
    .await;
    let (transport_event_tx, _transport_event_rx) =
        mpsc::channel::<TransportEvent>(CHANNEL_CAPACITY);
    let shutdown_token = CancellationToken::new();
    let (remote_task, _remote_handle) = start_remote_control(
        RemoteControlStartConfig {
            remote_control_url,
            installation_id: TEST_INSTALLATION_ID.to_string(),
            policy: RemoteControlPolicy::Allowed,
        },
        Some(remote_control_state_runtime(&ody_home).await),
        auth_manager,
        transport_event_tx,
        shutdown_token.clone(),
        /*app_server_client_name_rx*/ None,
        RemoteControlStartupMode::EnabledEphemeral,
    )
    .await
    .expect("remote control should start before account auth is available");

    timeout(Duration::from_millis(100), listener.accept())
        .await
        .expect_err("remote control should wait for auth before connecting");

    shutdown_token.cancel();
    timeout(Duration::from_secs(1), remote_task)
        .await
        .expect("remote control task should stop")
        .expect("remote control task should join");
}

#[tokio::test]
async fn remote_control_start_reports_missing_state_db_as_disabled_when_enabled() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener should bind");
    let remote_control_url = remote_control_url_for_listener(&listener);
    let (transport_event_tx, _transport_event_rx) =
        mpsc::channel::<TransportEvent>(CHANNEL_CAPACITY);
    let shutdown_token = CancellationToken::new();
    let (remote_task, remote_handle) = start_remote_control(
        RemoteControlStartConfig {
            remote_control_url,
            installation_id: TEST_INSTALLATION_ID.to_string(),
            policy: RemoteControlPolicy::Allowed,
        },
        /*state_db*/ None,
        remote_control_auth_manager(),
        transport_event_tx,
        shutdown_token.clone(),
        /*app_server_client_name_rx*/ None,
        RemoteControlStartupMode::EnabledEphemeral,
    )
    .await
    .expect("remote control should start disabled without sqlite state db");
    let mut status_rx = remote_handle.status_receiver();
    assert_eq!(
        status_rx.borrow().clone(),
        RemoteControlStatusChangedNotification {
            status: RemoteControlConnectionStatus::Disabled,
            server_name: test_server_name(),
            installation_id: TEST_INSTALLATION_ID.to_string(),
            environment_id: None,
        }
    );

    timeout(Duration::from_millis(100), listener.accept())
        .await
        .expect_err("remote control should not connect without sqlite state db");

    assert_eq!(
        remote_handle
            .enable_ephemeral()
            .expect_err("enable should fail"),
        RemoteControlEnableError::Unavailable(super::RemoteControlUnavailable)
    );
    timeout(Duration::from_millis(100), listener.accept())
        .await
        .expect_err("remote control should remain disabled without sqlite state db");
    timeout(Duration::from_millis(20), status_rx.changed())
        .await
        .expect_err("status should remain disabled without sqlite state db");

    shutdown_token.cancel();
    timeout(Duration::from_secs(1), remote_task)
        .await
        .expect("remote control task should stop")
        .expect("remote control task should join");
}



#[derive(Debug)]
struct CapturedHttpRequest {
    stream: TcpStream,
    request_line: String,
    headers: BTreeMap<String, String>,
    body: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CapturedWebSocketRequest {
    path: String,
    headers: BTreeMap<String, String>,
}

async fn accept_remote_control_connection(listener: &TcpListener) -> WebSocketStream<TcpStream> {
    let (stream, _) = timeout(Duration::from_secs(5), listener.accept())
        .await
        .expect("remote control should connect in time")
        .expect("listener accept should succeed");
    accept_async(stream)
        .await
        .expect("websocket handshake should succeed")
}

async fn accept_http_request(listener: &TcpListener) -> CapturedHttpRequest {
    let (stream, _) = timeout(Duration::from_secs(5), listener.accept())
        .await
        .expect("HTTP request should arrive in time")
        .expect("listener accept should succeed");
    let mut reader = BufReader::new(stream);

    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .await
        .expect("request line should read");
    let request_line = request_line.trim_end_matches("\r\n").to_string();

    let mut headers = BTreeMap::new();
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .await
            .expect("header line should read");
        if line == "\r\n" {
            break;
        }
        let line = line.trim_end_matches("\r\n");
        let (name, value) = line.split_once(':').expect("header should contain colon");
        headers.insert(name.to_ascii_lowercase(), value.trim().to_string());
    }

    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0; content_length];
    reader
        .read_exact(&mut body)
        .await
        .expect("request body should read");

    CapturedHttpRequest {
        stream: reader.into_inner(),
        request_line,
        headers,
        body: String::from_utf8(body).expect("body should be utf-8"),
    }
}

async fn respond_with_json(mut stream: TcpStream, body: serde_json::Value) {
    let body = body.to_string();
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .await
        .expect("response should write");
    stream.flush().await.expect("response should flush");
}

async fn respond_with_status(stream: TcpStream, status: &str, body: &str) {
    respond_with_status_and_headers(stream, status, &[], body).await;
}

async fn respond_with_status_and_headers(
    mut stream: TcpStream,
    status: &str,
    headers: &[(&str, &str)],
    body: &str,
) {
    let extra_headers = headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect::<String>();
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n{extra_headers}\r\n{body}",
        body.len(),
    );
    stream
        .write_all(response.as_bytes())
        .await
        .expect("response should write");
    stream.flush().await.expect("response should flush");
}

async fn accept_remote_control_backend_connection(
    listener: &TcpListener,
) -> (CapturedWebSocketRequest, WebSocketStream<TcpStream>) {
    let (stream, _) = timeout(Duration::from_secs(5), listener.accept())
        .await
        .expect("websocket request should arrive in time")
        .expect("listener accept should succeed");
    let captured_request = Arc::new(std::sync::Mutex::new(None::<CapturedWebSocketRequest>));
    let captured_request_for_callback = captured_request.clone();
    let websocket = accept_hdr_async(
        stream,
        move |request: &tungstenite::handshake::server::Request,
              response: tungstenite::handshake::server::Response| {
            let headers = request
                .headers()
                .iter()
                .map(|(name, value)| {
                    (
                        name.as_str().to_ascii_lowercase(),
                        value
                            .to_str()
                            .expect("header should be valid utf-8")
                            .to_string(),
                    )
                })
                .collect::<BTreeMap<_, _>>();
            *captured_request_for_callback
                .lock()
                .expect("capture lock should acquire") = Some(CapturedWebSocketRequest {
                path: request.uri().path().to_string(),
                headers,
            });
            Ok(response)
        },
    )
    .await
    .expect("websocket handshake should succeed");
    let captured_request = captured_request
        .lock()
        .expect("capture lock should acquire")
        .clone()
        .expect("websocket request should be captured");
    (captured_request, websocket)
}

async fn send_client_event(
    websocket: &mut WebSocketStream<TcpStream>,
    client_envelope: ClientEnvelope,
) {
    let payload = serde_json::to_string(&client_envelope).expect("client event should serialize");
    websocket
        .send(tungstenite::Message::Text(payload.into()))
        .await
        .expect("client event should send");
}

async fn read_server_event(websocket: &mut WebSocketStream<TcpStream>) -> serde_json::Value {
    read_server_event_with_stream_id(websocket).await.0
}

async fn read_server_event_with_stream_id(
    websocket: &mut WebSocketStream<TcpStream>,
) -> (serde_json::Value, StreamId) {
    loop {
        let frame = timeout(Duration::from_secs(5), websocket.next())
            .await
            .expect("server event should arrive in time")
            .expect("websocket should stay open")
            .expect("websocket frame should be readable");
        match frame {
            tungstenite::Message::Text(text) => {
                let mut event: serde_json::Value =
                    serde_json::from_str(text.as_ref()).expect("server event should deserialize");
                let stream_id = event
                    .as_object_mut()
                    .and_then(|event| event.remove("stream_id"))
                    .expect("stream_id should be present");
                let stream_id = stream_id
                    .as_str()
                    .expect("stream_id should be a string")
                    .to_string();
                return (event, StreamId(stream_id));
            }
            tungstenite::Message::Ping(payload) => {
                websocket
                    .send(tungstenite::Message::Pong(payload))
                    .await
                    .expect("websocket pong should send");
            }
            tungstenite::Message::Pong(_) => {}
            tungstenite::Message::Close(frame) => {
                panic!("unexpected websocket close frame: {frame:?}");
            }
            tungstenite::Message::Binary(_) => {
                panic!("unexpected binary websocket frame");
            }
            tungstenite::Message::Frame(_) => {}
        }
    }
}
