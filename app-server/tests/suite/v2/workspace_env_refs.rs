//! E4 T06: spec envRefs are validated at define, secretValues inject at
//! start, and secret values never reach any persisted surface.
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use app_test_support::TestAppServer;
use app_test_support::to_response;
use ody_app_server_protocol::ClientInfo;
use ody_app_server_protocol::InitializeCapabilities;
use ody_app_server_protocol::JSONRPCMessage;
use ody_app_server_protocol::RequestId;
use ody_app_server_protocol::WorkspacePreviewCheckParams;
use ody_app_server_protocol::WorkspacePreviewCheckResponse;
use ody_app_server_protocol::WorkspaceProjectBindParams;
use ody_app_server_protocol::WorkspaceServiceDefineParams;
use ody_app_server_protocol::WorkspaceServiceDefineResponse;
use ody_app_server_protocol::WorkspaceServiceSpecsParams;
use ody_app_server_protocol::WorkspaceServiceSpecsResponse;
use ody_app_server_protocol::WorkspaceServiceStartAllParams;
use ody_app_server_protocol::WorkspaceServiceStartAllResponse;
use ody_utils_absolute_path::test_support::PathBufExt;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);
const SECRET_VALUE: &str = "sk-integration-777";

fn default_client_info() -> ClientInfo {
    ClientInfo {
        name: app_test_support::DEFAULT_CLIENT_NAME.to_string(),
        title: None,
        version: "0.1.0".to_string(),
    }
}

fn node_available() -> bool {
    std::process::Command::new("node")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
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

/// Fixture whose dev server folds the injected STRIPE_KEY into the HTML
/// `<title>`; the preview check then proves the child process received it.
fn secret_fixture() -> Result<TempDir> {
    let dir = TempDir::new()?;
    fs::write(
        dir.path().join("package.json"),
        serde_json::json!({
            "name": "env-refs-fixture",
            "scripts": { "dev": "node server.js" }
        })
        .to_string(),
    )?;
    fs::write(
        dir.path().join("server.js"),
        r#"const http = require('http');
const port = Number(process.env.PORT || 5173);
const server = http.createServer((req, res) => {
    res.writeHead(200, { 'content-type': 'text/html' });
    res.end(`<html><head><title>stripe:${process.env.STRIPE_KEY || 'missing'}</title></head><body>ok</body></html>`);
});
server.listen(port, '127.0.0.1');
"#,
    )?;
    Ok(dir)
}

async fn read_response<T: serde::de::DeserializeOwned>(
    mcp: &mut TestAppServer,
    request_id: i64,
) -> Result<T> {
    let message = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    Ok(to_response::<T>(message)?)
}

#[tokio::test]
async fn start_all_with_secret_reaches_backend_and_leaves_no_trace() -> Result<()> {
    if !node_available() {
        eprintln!("node unavailable; skipping");
        return Ok(());
    }
    let ody_home = TempDir::new()?;
    let fixture = secret_fixture()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;

    let bind_id = mcp
        .send_workspace_project_bind_request(WorkspaceProjectBindParams {
            id: "ws-1".to_owned(),
            name: "env refs fixture".to_owned(),
            roots: vec![fixture.path().to_path_buf().abs()],
            idempotency_key: "bind-1".to_owned(),
        })
        .await?;
    let _: serde::de::IgnoredAny = read_response(&mut mcp, bind_id).await?;

    let define_id = mcp
        .send_workspace_service_define_request(WorkspaceServiceDefineParams {
            project_id: "ws-1".to_owned(),
            name: "api".to_owned(),
            root_index: 0,
            script: "dev".to_owned(),
            port: None,
            cwd: None,
            depends_on: Vec::new(),
            health_check: None,
            ready_timeout_ms: Some(15_000),
            env_refs: Some(vec!["STRIPE_KEY".to_owned()]),
            idempotency_key: "define-api".to_owned(),
        })
        .await?;
    let defined = read_response::<WorkspaceServiceDefineResponse>(&mut mcp, define_id).await?;
    assert_eq!(defined.spec.env_refs, vec!["STRIPE_KEY".to_owned()]);

    // Illegal ref names are rejected over the wire with a structured error.
    let bad_define_id = mcp
        .send_workspace_service_define_request(WorkspaceServiceDefineParams {
            project_id: "ws-1".to_owned(),
            name: "bad".to_owned(),
            root_index: 0,
            script: "dev".to_owned(),
            port: None,
            cwd: None,
            depends_on: Vec::new(),
            health_check: None,
            ready_timeout_ms: None,
            env_refs: Some(vec!["lowercase".to_owned()]),
            idempotency_key: "define-bad".to_owned(),
        })
        .await?;
    let bad_error = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(bad_define_id)),
    )
    .await??;
    assert!(
        bad_error.error.message.contains("UPPER_SNAKE"),
        "{}",
        bad_error.error.message
    );

    let start_all_id = mcp
        .send_workspace_service_start_all_request(WorkspaceServiceStartAllParams {
            project_id: "ws-1".to_owned(),
            names: vec!["api".to_owned()],
            secret_values: Some(BTreeMap::from([(
                "STRIPE_KEY".to_owned(),
                SECRET_VALUE.to_owned(),
            )])),
            idempotency_key: "start-all-1".to_owned(),
        })
        .await?;
    let orchestration =
        read_response::<WorkspaceServiceStartAllResponse>(&mut mcp, start_all_id).await?;
    assert_eq!(orchestration.started.len(), 1, "{orchestration:?}");
    assert!(orchestration.failed.is_empty(), "{orchestration:?}");
    let service_id = orchestration.services[0].id.clone();

    // The child process folds the secret into its HTML title; the preview
    // check response is the proof the value arrived over the env channel.
    let check_id = mcp
        .send_workspace_preview_check_request(WorkspacePreviewCheckParams {
            service_id,
            url: None,
            timeout_ms: None,
        })
        .await?;
    let check = read_response::<WorkspacePreviewCheckResponse>(&mut mcp, check_id).await?;
    assert!(check.reachable, "{check:?}");
    assert_eq!(
        check.title.as_deref(),
        Some(format!("stripe:{SECRET_VALUE}").as_str()),
        "backend received the secret"
    );

    // Spec round-trip keeps the ref names (serialization survives persist).
    let specs_id = mcp
        .send_workspace_service_specs_request(WorkspaceServiceSpecsParams {
            project_id: "ws-1".to_owned(),
        })
        .await?;
    let specs = read_response::<WorkspaceServiceSpecsResponse>(&mut mcp, specs_id).await?;
    let api = specs
        .specs
        .iter()
        .find(|spec| spec.name == "api")
        .expect("api spec");
    assert_eq!(api.env_refs, vec!["STRIPE_KEY".to_owned()]);

    // Persistence-surface scan: ref names stay, values never appear.
    for file in ["workspace-service/v1.json", "workspace-audit/v1.jsonl"] {
        let raw = fs::read_to_string(ody_home.path().join(file))?;
        assert!(raw.contains("STRIPE_KEY"), "{file} keeps the ref name");
        assert!(
            !raw.contains(SECRET_VALUE),
            "{file} must never contain the secret value"
        );
    }
    Ok(())
}
