//! E4 T04: service crash is observable via workspace/service/changed and
//! Runtime restart normalization is durable + announced.
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
use ody_app_server_protocol::WorkspaceProjectBindParams;
use ody_app_server_protocol::WorkspaceServiceListParams;
use ody_app_server_protocol::WorkspaceServiceListResponse;
use ody_app_server_protocol::WorkspaceServiceRef;
use ody_app_server_protocol::WorkspaceServiceStartParams;
use ody_app_server_protocol::WorkspaceServiceStartResponse;
use ody_utils_absolute_path::test_support::PathBufExt;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);

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

fn bind_params(id: &str, roots: Vec<PathBuf>) -> WorkspaceProjectBindParams {
    WorkspaceProjectBindParams {
        id: id.to_owned(),
        name: "fixture project".to_owned(),
        roots: roots.into_iter().map(|path| path.abs()).collect(),
        idempotency_key: format!("key-{id}"),
    }
}

fn service_fixture() -> Result<TempDir> {
    let dir = TempDir::new()?;
    fs::write(
        dir.path().join("package.json"),
        serde_json::json!({
            "name": "recovery-fixture",
            "scripts": {
                "crash": "node crasher.js",
                "dev": "node server.js"
            }
        })
        .to_string(),
    )?;
    fs::write(
        dir.path().join("crasher.js"),
        "setTimeout(() => process.exit(3), 100);\n",
    )?;
    fs::write(
        dir.path().join("server.js"),
        r#"const http = require('http');
const port = Number(process.env.PORT || 5173);
const server = http.createServer((req, res) => {
    res.writeHead(200, { 'content-type': 'text/html' });
    res.end('ok');
});
server.listen(port, '127.0.0.1');
"#,
    )?;
    Ok(dir)
}

async fn bind_fixture_project(mcp: &mut TestAppServer, root: &TempDir) -> Result<()> {
    let request_id = mcp
        .send_workspace_project_bind_request(bind_params(
            "ws-1",
            vec![root.path().to_path_buf()],
        ))
        .await?;
    let message = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let _: serde::de::IgnoredAny = to_response(message)?;
    Ok(())
}

async fn start_service(
    mcp: &mut TestAppServer,
    name: &str,
    script: &str,
    idem: &str,
) -> Result<WorkspaceServiceRef> {
    let request_id = mcp
        .send_workspace_service_start_request(WorkspaceServiceStartParams {
            project_id: "ws-1".to_owned(),
            name: name.to_owned(),
            root_index: 0,
            script: script.to_owned(),
            port: None,
            ready_timeout_ms: Some(10_000),
            env: None,
            idempotency_key: idem.to_owned(),
        })
        .await?;
    let message = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    Ok(to_response::<WorkspaceServiceStartResponse>(message)?.service)
}

async fn list_services(mcp: &mut TestAppServer) -> Result<Vec<WorkspaceServiceRef>> {
    let request_id = mcp
        .send_workspace_service_list_request(WorkspaceServiceListParams {
            project_id: "ws-1".to_owned(),
        })
        .await?;
    let message = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    Ok(to_response::<WorkspaceServiceListResponse>(message)?.services)
}

#[tokio::test]
async fn service_crash_reaches_client_and_survives_runtime_restart() -> Result<()> {
    if tokio::process::Command::new("node")
        .arg("--version")
        .output()
        .await
        .is_err()
    {
        eprintln!("node unavailable; skipping");
        return Ok(());
    }
    let ody_home = TempDir::new()?;
    let fixture = service_fixture()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    bind_fixture_project(&mut mcp, &fixture).await?;

    // Phase 1: a real subprocess crash must reach the client with the real
    // exit code (no polling).
    let crasher = start_service(&mut mcp, "crasher", "crash", "crash-1").await?;
    let crash_notice = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_matching_notification("service/changed failed", |n| {
            n.method == "workspace/service/changed"
                && n.params.as_ref().is_some_and(|p| {
                    p["reason"] == "failed" && p["services"][0]["id"] == crasher.id
                })
        }),
    )
    .await??;
    let params = crash_notice.params.expect("params");
    assert_eq!(params["services"][0]["exitCode"], 3);

    // Phase 2: a long-running service reaches Ready and announces Started.
    let dev = start_service(&mut mcp, "dev", "dev", "dev-1").await?;
    assert_eq!(
        dev.status,
        ody_app_server_protocol::WorkspaceServiceStatus::Ready
    );
    let started_notice = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_matching_notification("service/changed started", |n| {
            n.method == "workspace/service/changed"
                && n.params.as_ref().is_some_and(|p| {
                    p["reason"] == "started" && p["services"][0]["id"] == dev.id
                })
        }),
    )
    .await??;
    let _ = started_notice;

    // Phase 3: Runtime crash (SIGKILL — graceful shutdown would persist a
    // terminal state and leave nothing to normalize). Same ody_home, new
    // server process; the non-terminal `dev` service normalizes to Stopped
    // and the restart is announced; the already-terminal crasher record is
    // left untouched.
    let dev_pid = dev.pid;
    let mut mcp = mcp;
    mcp.kill_hard()?;
    drop(mcp);
    let mut mcp2 = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp2).await?;
    let restart_notice = timeout(
        DEFAULT_TIMEOUT,
        mcp2.read_stream_until_matching_notification("service/changed runtimeRestarted", |n| {
            n.method == "workspace/service/changed"
                && n.params
                    .as_ref()
                    .is_some_and(|p| p["reason"] == "runtimeRestarted")
        }),
    )
    .await??;
    let params = restart_notice.params.expect("params");
    assert_eq!(params["services"].as_array().expect("services").len(), 1);
    assert_eq!(params["services"][0]["name"], "dev");

    let services = list_services(&mut mcp2).await?;
    let dev = services
        .iter()
        .find(|service| service.name == "dev")
        .expect("dev record");
    assert_eq!(
        dev.status,
        ody_app_server_protocol::WorkspaceServiceStatus::Stopped
    );
    assert!(
        dev.error
            .as_deref()
            .is_some_and(|error| error.contains("runtime restarted")),
        "error should explain restart, got {:?}",
        dev.error
    );
    let crasher = services
        .iter()
        .find(|service| service.name == "crasher")
        .expect("crasher record");
    assert_eq!(
        crasher.status,
        ody_app_server_protocol::WorkspaceServiceStatus::Failed
    );
    assert_eq!(crasher.exit_code, Some(3));

    // The SIGKILLed Runtime could not clean up its service process tree;
    // kill the orphaned dev server so the test leaves no stray process.
    #[cfg(unix)]
    if let Some(pid) = dev_pid {
        let _ = std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status();
    }
    Ok(())
}
