use anyhow::Result;
use anyhow::bail;
use app_test_support::ApiKeyAuthFixture;
use app_test_support::TestAppServer;
use app_test_support::to_response;
use app_test_support::write_api_key_auth;
use app_test_support::write_models_cache;
use ody_app_server_protocol::AttestationGenerateResponse;
use ody_app_server_protocol::ClientInfo;
use ody_app_server_protocol::InitializeCapabilities;
use ody_app_server_protocol::JSONRPCMessage;
use ody_app_server_protocol::JSONRPCResponse;
use ody_app_server_protocol::RequestId;
use ody_app_server_protocol::ServerRequest;
use ody_app_server_protocol::ThreadStartParams;
use ody_app_server_protocol::ThreadStartResponse;
use ody_app_server_protocol::TurnStartParams;
use ody_app_server_protocol::TurnStartResponse;
use ody_app_server_protocol::UserInput as V2UserInput;
use ody_config::types::AuthCredentialsStoreMode;
use core_test_support::responses;
use core_test_support::responses::WebSocketConnectionConfig;
use core_test_support::responses::start_websocket_server_with_headers;
use core_test_support::skip_if_no_network;
use pretty_assertions::assert_eq;
use std::path::Path;
use tempfile::TempDir;
use tokio::time::Duration;
use tokio::time::timeout;

const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(60);
const ATTESTATION_HEADER: &str = "v1.integration-test";
const APP_SERVER_ATTESTATION_HEADER: &str = r#"{"v":1,"s":0,"t":"v1.integration-test"}"#;

#[tokio::test]
async fn attestation_generate_round_trip_adds_header_to_responses_websocket_handshake() -> Result<()>
{
    skip_if_no_network!(Ok(()));

    let websocket_server = start_websocket_server_with_headers(vec![WebSocketConnectionConfig {
        requests: vec![
            vec![
                responses::ev_response_created("warm-1"),
                responses::ev_completed("warm-1"),
            ],
            vec![
                responses::ev_response_created("resp-1"),
                responses::ev_assistant_message("msg-1", "Done"),
                responses::ev_completed("resp-1"),
            ],
        ],
        response_headers: Vec::new(),
        accept_delay: None,
        close_after_requests: true,
    }])
    .await;

    let ody_home = TempDir::new()?;
    write_models_cache(ody_home.path())?;
    create_legacy_websocket_config(
        ody_home.path(),
        &websocket_server.uri().replacen("ws://", "http://", 1),
    )?;
    write_api_key_auth(
        ody_home.path(),
        ApiKeyAuthFixture::new("access-key").plan_type("pro"),
        AuthCredentialsStoreMode::File,
    )?;

    let mut mcp =
        TestAppServer::new_with_env(ody_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    let initialized = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.initialize_with_capabilities(
            ClientInfo {
                name: "ody_desktop".to_string(),
                title: Some("Ody Desktop".to_string()),
                version: "0.1.0".to_string(),
            },
            Some(InitializeCapabilities {
                experimental_api: true,
                request_attestation: true,
                opt_out_notification_methods: None,
                mcp_server_odysseythink_form_elicitation: false,
            }),
        ),
    )
    .await??;
    let JSONRPCMessage::Response(_) = initialized else {
        bail!("expected initialize response, got {initialized:?}");
    };

    let thread_request_id = mcp
        .send_thread_start_request(ThreadStartParams::default())
        .await?;
    let thread_response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(thread_request_id)),
    )
    .await??;
    let ThreadStartResponse { thread, .. } = to_response(thread_response)?;

    let turn_request_id = mcp
        .send_turn_start_request(TurnStartParams {
            thread_id: thread.id,
            client_user_message_id: None,
            input: vec![V2UserInput::Text {
                text: "Hello".to_string(),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    let turn_response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(turn_request_id)),
    )
    .await??;
    let _: TurnStartResponse = to_response(turn_response)?;

    let mut attestation_requests = 0;
    timeout(DEFAULT_READ_TIMEOUT, async {
        loop {
            match mcp.read_next_message().await? {
                JSONRPCMessage::Request(request) => {
                    let request = ServerRequest::try_from(request)?;
                    let ServerRequest::AttestationGenerate { request_id, .. } = request else {
                        bail!("expected attestation/generate request, got {request:?}");
                    };
                    attestation_requests += 1;
                    mcp.send_response(
                        request_id,
                        serde_json::to_value(AttestationGenerateResponse {
                            token: ATTESTATION_HEADER.to_string(),
                        })?,
                    )
                    .await?;
                }
                JSONRPCMessage::Notification(notification)
                    if notification.method == "turn/completed" =>
                {
                    break Ok(());
                }
                _ => {}
            }
        }
    })
    .await??;
    assert!(attestation_requests > 0);

    assert!(
        websocket_server
            .wait_for_handshakes(/*expected*/ 1, DEFAULT_READ_TIMEOUT)
            .await
    );
    let handshake = websocket_server.single_handshake();
    assert_eq!(
        handshake.header("x-oai-attestation").as_deref(),
        Some(APP_SERVER_ATTESTATION_HEADER)
    );

    websocket_server.shutdown().await;
    Ok(())
}

fn create_legacy_websocket_config(ody_home: &Path, server_uri: &str) -> std::io::Result<()> {
    std::fs::write(
        ody_home.join("config.toml"),
        format!(
            r#"
model = "mock-model"
approval_policy = "never"
sandbox_mode = "read-only"

model_provider = "mock_provider"

[model_providers.mock_provider]
name = "Mock Legacy provider for test"
base_url = "{server_uri}/v1"
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0
requires_odysseythink_auth = true
supports_websockets = true
"#
        ),
    )
}
