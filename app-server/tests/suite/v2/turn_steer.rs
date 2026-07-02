#![cfg(unix)]

use anyhow::Context;
use anyhow::Result;
use app_test_support::TestAppServer;
use app_test_support::create_mock_responses_server_sequence;
use app_test_support::create_mock_responses_server_sequence_unchecked;
use app_test_support::create_shell_command_sse_response;
use app_test_support::to_response;
use app_test_support::write_mock_responses_config_toml_simple;
use ody_app_server::INPUT_TOO_LARGE_ERROR_CODE;
use ody_app_server::INVALID_PARAMS_ERROR_CODE;
use ody_app_server_protocol::AdditionalContextEntry;
use ody_app_server_protocol::AdditionalContextKind;
use ody_app_server_protocol::ItemStartedNotification;
use ody_app_server_protocol::JSONRPCError;
use ody_app_server_protocol::JSONRPCNotification;
use ody_app_server_protocol::JSONRPCResponse;
use ody_app_server_protocol::RequestId;
use ody_app_server_protocol::ThreadItem;
use ody_app_server_protocol::ThreadStartParams;
use ody_app_server_protocol::ThreadStartResponse;
use ody_app_server_protocol::TurnStartParams;
use ody_app_server_protocol::TurnStartResponse;
use ody_app_server_protocol::TurnSteerParams;
use ody_app_server_protocol::TurnSteerResponse;
use ody_app_server_protocol::UserInput as V2UserInput;
use ody_protocol::user_input::MAX_USER_INPUT_TEXT_CHARS;
use serde_json::Value;
use std::collections::HashMap;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[tokio::test]
async fn turn_steer_requires_active_turn() -> Result<()> {
    let tmp = TempDir::new()?;
    let ody_home = tmp.path().join("ody_home");
    std::fs::create_dir(&ody_home)?;

    let server = create_mock_responses_server_sequence(vec![]).await;
    write_mock_responses_config_toml_simple(&ody_home, &server.uri())?;

    let mut mcp = TestAppServer::new_without_managed_config(&ody_home).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let thread_req = mcp
        .send_thread_start_request(ThreadStartParams {
            model: Some("mock-model".to_string()),
            ..Default::default()
        })
        .await?;
    let thread_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(thread_req)),
    )
    .await??;
    let ThreadStartResponse { thread, .. } = to_response::<ThreadStartResponse>(thread_resp)?;

    let steer_req = mcp
        .send_turn_steer_request(TurnSteerParams {
            thread_id: thread.id.clone(),
            client_user_message_id: Some("client-steer-message-1".to_string()),
            input: vec![V2UserInput::Text {
                text: "steer".to_string(),
                text_elements: Vec::new(),
            }],
            responsesapi_client_metadata: None,
            additional_context: None,
            expected_turn_id: "turn-does-not-exist".to_string(),
        })
        .await?;
    let steer_err: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(steer_req)),
    )
    .await??;
    assert_eq!(steer_err.error.code, -32600);

    Ok(())
}

#[tokio::test]
async fn turn_steer_rejects_context_only_input_without_merging_context() -> Result<()> {
    let tmp = TempDir::new()?;
    let ody_home = tmp.path().join("ody_home");
    std::fs::create_dir(&ody_home)?;
    let working_directory = tmp.path().join("workdir");
    std::fs::create_dir(&working_directory)?;

    let server = create_mock_responses_server_sequence_unchecked(vec![
        create_shell_command_sse_response(
            vec!["sleep".to_string(), "1".to_string()],
            Some(&working_directory),
            Some(10_000),
            "call_sleep",
        )?,
        app_test_support::create_final_assistant_message_sse_response("Done")?,
    ])
    .await;
    write_mock_responses_config_toml_simple(&ody_home, &server.uri())?;

    let mut mcp = TestAppServer::new_without_managed_config(&ody_home).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let thread_req = mcp
        .send_thread_start_request(ThreadStartParams {
            model: Some("mock-model".to_string()),
            ..Default::default()
        })
        .await?;
    let thread_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(thread_req)),
    )
    .await??;
    let ThreadStartResponse { thread, .. } = to_response::<ThreadStartResponse>(thread_resp)?;

    let turn_req = mcp
        .send_turn_start_request(TurnStartParams {
            thread_id: thread.id.clone(),
            client_user_message_id: None,
            input: vec![V2UserInput::Text {
                text: "run sleep".to_string(),
                text_elements: Vec::new(),
            }],
            cwd: Some(working_directory),
            ..Default::default()
        })
        .await?;
    let turn_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(turn_req)),
    )
    .await??;
    let TurnStartResponse { turn } = to_response::<TurnStartResponse>(turn_resp)?;
    timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("turn/started"),
    )
    .await??;

    let additional_context = Some(HashMap::from([(
        "browser_info".to_string(),
        AdditionalContextEntry {
            value: "tab one".to_string(),
            kind: AdditionalContextKind::Untrusted,
        },
    )]));
    let steer_req = mcp
        .send_turn_steer_request(TurnSteerParams {
            thread_id: thread.id.clone(),
            client_user_message_id: None,
            input: Vec::new(),
            responsesapi_client_metadata: None,
            additional_context,
            expected_turn_id: turn.id,
        })
        .await?;
    let steer_error: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(steer_req)),
    )
    .await??;
    assert_eq!(steer_error.error.code, -32600);
    assert_eq!(steer_error.error.message, "input must not be empty");

    timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("turn/completed"),
    )
    .await??;

    let requests = server
        .received_requests()
        .await
        .context("failed to fetch received requests")?;
    let response_requests = requests
        .iter()
        .filter(|request| request.url.path().ends_with("/responses"))
        .collect::<Vec<_>>();
    assert_eq!(response_requests.len(), 2);
    let body = response_requests[1]
        .body_json::<Value>()
        .context("request body should be JSON")?;
    assert!(
        !body
            .to_string()
            .contains("<external_browser_info>tab one</external_browser_info>")
    );

    Ok(())
}
