//! E4 T03: audit log records workspace operations end-to-end.
use std::fs;
use std::time::Duration;

use anyhow::Result;
use app_test_support::TestAppServer;
use app_test_support::to_response;
use ody_app_server_protocol::ClientInfo;
use ody_app_server_protocol::InitializeCapabilities;
use ody_app_server_protocol::JSONRPCMessage;
use ody_app_server_protocol::RequestId;
use ody_app_server_protocol::WorkspaceAuditListParams;
use ody_app_server_protocol::WorkspaceAuditListResponse;
use ody_app_server_protocol::WorkspaceChangeSet;
use ody_app_server_protocol::WorkspaceChangeSetApplyParams;
use ody_app_server_protocol::WorkspaceChangeSetCreateParams;
use ody_app_server_protocol::WorkspaceFileChange;
use ody_app_server_protocol::WorkspaceFileChangeKind;
use ody_app_server_protocol::WorkspaceProjectBindParams;
use ody_utils_absolute_path::test_support::PathBufExt;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

fn default_client_info() -> ClientInfo {
    ClientInfo {
        name: app_test_support::DEFAULT_CLIENT_NAME.to_string(),
        title: None,
        version: "0.1.0".to_string(),
    }
}

async fn init_experimental(mcp: &mut TestAppServer) -> Result<()> {
    let init = mcp
        .initialize_with_capabilities(
            default_client_info(),
            Some(InitializeCapabilities {
                experimental_api: true,
                request_attestation: false,
                opt_out_notification_methods: None,
                mcp_server_form_elicitation: false,
            }),
        )
        .await?;
    let JSONRPCMessage::Response(_) = init else {
        anyhow::bail!("expected initialize response, got {init:?}");
    };
    Ok(())
}

async fn read_audit(
    mcp: &mut TestAppServer,
    request_id: i64,
) -> Result<WorkspaceAuditListResponse> {
    let message = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    to_response(message)
}

async fn read_changeset(mcp: &mut TestAppServer, request_id: i64) -> Result<WorkspaceChangeSet> {
    let message = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    #[derive(serde::Deserialize)]
    struct Envelope {
        changeset: WorkspaceChangeSet,
    }
    Ok(to_response::<Envelope>(message)?.changeset)
}

#[tokio::test]
async fn bind_create_apply_produces_ordered_audit_trail() -> Result<()> {
    let ody_home = TempDir::new()?;
    let fixture = TempDir::new()?;
    fs::write(fixture.path().join("a.ts"), "old\n")?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;

    let bind_id = mcp
        .send_workspace_project_bind_request(WorkspaceProjectBindParams {
            id: "ws-audit-1".to_owned(),
            name: "audit fixture".to_owned(),
            roots: vec![fixture.path().to_path_buf().abs()],
            idempotency_key: "audit-bind-1".to_owned(),
        })
        .await?;
    let message = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(bind_id)),
    )
    .await??;
    let bound: serde_json::Value = to_response(message)?;
    assert!(bound.get("project").is_some(), "bind failed: {bound}");

    // base hash via sha256 of current content (workspace_changeset format).
    let base_hash = {
        use sha2::Digest as _;
        let mut hasher = sha2::Sha256::new();
        hasher.update(b"old\n");
        format!("{:x}", hasher.finalize())
    };
    let create_id = mcp
        .send_workspace_source_changeset_create_request(WorkspaceChangeSetCreateParams {
            project_id: "ws-audit-1".to_owned(),
            title: "update a".to_owned(),
            changes: vec![WorkspaceFileChange {
                root_index: 0,
                path: "a.ts".to_owned(),
                kind: WorkspaceFileChangeKind::Update,
                base_hash: Some(base_hash),
                content: Some("new\n".to_owned()),
            }],
            idempotency_key: "audit-cs-1".to_owned(),
        })
        .await?;
    let changeset = read_changeset(&mut mcp, create_id).await?;

    let apply_id = mcp
        .send_workspace_source_changeset_apply_request(WorkspaceChangeSetApplyParams {
            changeset_id: changeset.id.clone(),
        })
        .await?;
    let applied = read_changeset(&mut mcp, apply_id).await?;
    assert_eq!(applied.id, changeset.id);
    assert_eq!(fs::read_to_string(fixture.path().join("a.ts"))?, "new\n");

    let list_id = mcp
        .send_workspace_audit_list_request(WorkspaceAuditListParams {
            project_id: Some("ws-audit-1".to_owned()),
            limit: None,
        })
        .await?;
    let response = read_audit(&mut mcp, list_id).await?;
    let ops: Vec<&str> = response
        .events
        .iter()
        .map(|event| event.operation.as_str())
        .collect();
    assert!(
        ops.contains(&"project.bind"),
        "expected project.bind in {ops:?}"
    );
    assert!(
        ops.contains(&"changeset.create"),
        "expected changeset.create in {ops:?}"
    );
    assert!(
        ops.contains(&"changeset.apply"),
        "expected changeset.apply in {ops:?}"
    );
    // Newest first and monotonic seq.
    assert!(response
        .events
        .windows(2)
        .all(|window| window[0].seq > window[1].seq));

    // Key discipline: no file content anywhere in the trail.
    let raw = fs::read_to_string(ody_home.path().join("workspace-audit").join("v1.jsonl"))?;
    assert!(!raw.contains("new\n"), "audit must not contain file content");
    Ok(())
}
