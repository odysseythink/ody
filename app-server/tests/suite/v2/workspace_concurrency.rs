//! E4 T05: two windows (two connections to one app-server) cannot interleave
//! writes on the same project. A foreign lock holder is rejected before any
//! file write, lock state is visible through `workspace/project/get`'s
//! `lockedBy`, and release lets the other window proceed.

use anyhow::Result;
use app_test_support::create_mock_responses_server_sequence_unchecked;
use app_test_support::to_response;
use ody_app_server_protocol::ClientInfo;
use ody_app_server_protocol::InitializeCapabilities;
use ody_app_server_protocol::InitializeParams;
use ody_app_server_protocol::JSONRPCError;
use ody_app_server_protocol::JSONRPCResponse;
use ody_app_server_protocol::WorkspaceProjectGetResponse;
use serde_json::json;
use sha2::Digest;
use sha2::Sha256;
use std::fs;
use tempfile::TempDir;
use tokio::time::timeout;

use super::connection_handling_websocket::DEFAULT_READ_TIMEOUT;
use super::connection_handling_websocket::WsClient;
use super::connection_handling_websocket::connect_websocket;
use super::connection_handling_websocket::create_config_toml;
use super::connection_handling_websocket::read_error_for_id;
use super::connection_handling_websocket::read_response_for_id;
use super::connection_handling_websocket::send_request;
use super::connection_handling_websocket::spawn_websocket_server;

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

async fn send_experimental_initialize(
    ws: &mut WsClient,
    id: i64,
    client_name: &str,
) -> Result<()> {
    let params = InitializeParams {
        client_info: ClientInfo {
            name: client_name.to_string(),
            title: Some("Workspace Concurrency Test Client".to_string()),
            version: "0.1.0".to_string(),
        },
        capabilities: Some(InitializeCapabilities {
            experimental_api: true,
            request_attestation: false,
            opt_out_notification_methods: None,
            mcp_server_form_elicitation: false,
        }),
    };
    send_request(ws, "initialize", id, Some(serde_json::to_value(params)?)).await
}

async fn read_ok_response(ws: &mut WsClient, id: i64) -> Result<JSONRPCResponse> {
    timeout(DEFAULT_READ_TIMEOUT, read_response_for_id(ws, id)).await?
}

#[tokio::test]
async fn second_window_rejects_apply_while_first_holds_lock() -> Result<()> {
    let server = create_mock_responses_server_sequence_unchecked(Vec::new()).await;
    let ody_home = TempDir::new()?;
    create_config_toml(ody_home.path(), &server.uri(), "never")?;

    // One app-server process; two windows as two websocket connections.
    let (mut process, bind_addr) = spawn_websocket_server(ody_home.path()).await?;
    let mut window1 = connect_websocket(bind_addr).await?;
    let mut window2 = connect_websocket(bind_addr).await?;
    send_experimental_initialize(&mut window1, 1, "ws_window_one").await?;
    send_experimental_initialize(&mut window2, 1, "ws_window_two").await?;
    read_ok_response(&mut window1, 1).await?;
    read_ok_response(&mut window2, 1).await?;

    let fixture = TempDir::new()?;
    fs::create_dir_all(fixture.path().join("src"))?;
    fs::write(fixture.path().join("src/a.ts"), "old\n")?;
    let root = fs::canonicalize(fixture.path())?;

    // Window 1 binds the project and prepares a changeset.
    send_request(
        &mut window1,
        "workspace/project/bind",
        2,
        Some(json!({
            "id": "p1",
            "name": "concurrency-fixture",
            "roots": [root],
            "idempotencyKey": "bind-1",
        })),
    )
    .await?;
    read_ok_response(&mut window1, 2).await?;

    send_request(
        &mut window1,
        "workspace/source/changeset/create",
        3,
        Some(json!({
            "projectId": "p1",
            "title": "update a",
            "changes": [{
                "rootIndex": 0,
                "path": "src/a.ts",
                "kind": "update",
                "baseHash": sha256_hex(b"old\n"),
                "content": "new\n",
            }],
            "idempotencyKey": "cs-1",
        })),
    )
    .await?;
    let created = read_ok_response(&mut window1, 3).await?;
    let changeset_id = created
        .result
        .get("changeset")
        .and_then(|cs| cs.get("id"))
        .and_then(|id| id.as_str())
        .expect("changeset id")
        .to_owned();

    // Window 2 declares write intent.
    send_request(
        &mut window2,
        "workspace/project/lock",
        2,
        Some(json!({ "projectId": "p1" })),
    )
    .await?;
    read_ok_response(&mut window2, 2).await?;

    // Window 1's apply is rejected and nothing reaches disk.
    send_request(
        &mut window1,
        "workspace/source/changeset/apply",
        4,
        Some(json!({ "changesetId": changeset_id })),
    )
    .await?;
    let error: JSONRPCError =
        timeout(DEFAULT_READ_TIMEOUT, read_error_for_id(&mut window1, 4)).await??;
    assert!(
        error.error.message.contains("locked by another session"),
        "got: {}",
        error.error.message
    );
    assert_eq!(fs::read_to_string(fixture.path().join("src/a.ts"))?, "old\n");

    // Lock state is diagnosable from any window.
    send_request(
        &mut window1,
        "workspace/project/get",
        5,
        Some(json!({ "projectId": "p1" })),
    )
    .await?;
    let get_response = read_ok_response(&mut window1, 5).await?;
    let project: WorkspaceProjectGetResponse = to_response(get_response)?;
    assert!(
        project.project.locked_by.is_some(),
        "lockedBy must be exposed while a window holds the lock"
    );

    // Window 2 releases; window 1's retry applies and writes.
    send_request(
        &mut window2,
        "workspace/project/unlock",
        3,
        Some(json!({ "projectId": "p1" })),
    )
    .await?;
    read_ok_response(&mut window2, 3).await?;
    send_request(
        &mut window1,
        "workspace/source/changeset/apply",
        6,
        Some(json!({ "changesetId": changeset_id })),
    )
    .await?;
    read_ok_response(&mut window1, 6).await?;
    assert_eq!(fs::read_to_string(fixture.path().join("src/a.ts"))?, "new\n");

    // After unlock, lockedBy is cleared again.
    send_request(
        &mut window1,
        "workspace/project/get",
        7,
        Some(json!({ "projectId": "p1" })),
    )
    .await?;
    let get_response = read_ok_response(&mut window1, 7).await?;
    let project: WorkspaceProjectGetResponse = to_response(get_response)?;
    assert!(project.project.locked_by.is_none());

    process
        .kill()
        .await
        .map_err(|error| anyhow::anyhow!("failed to stop app-server: {error}"))?;
    Ok(())
}
