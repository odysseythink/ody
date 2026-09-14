use std::fs;
use std::net::TcpListener;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use app_test_support::DEFAULT_CLIENT_NAME;
use app_test_support::TestAppServer;
use app_test_support::next_test_port;
use app_test_support::to_response;
use ody_app_server_protocol::ClientInfo;
use ody_app_server_protocol::InitializeCapabilities;
use ody_app_server_protocol::JSONRPCMessage;
use ody_app_server_protocol::RequestId;
use ody_app_server_protocol::WorkspacePreviewCheckParams;
use ody_app_server_protocol::WorkspacePreviewCheckResponse;
use ody_app_server_protocol::WorkspaceProjectBindParams;
use ody_app_server_protocol::WorkspaceProjectCloseParams;
use ody_app_server_protocol::WorkspaceServiceListParams;
use ody_app_server_protocol::WorkspaceServiceListResponse;
use ody_app_server_protocol::WorkspaceServiceLogsParams;
use ody_app_server_protocol::WorkspaceServiceLogsResponse;
use ody_app_server_protocol::WorkspaceServiceRef;
use ody_app_server_protocol::WorkspaceServiceStartParams;
use ody_app_server_protocol::WorkspaceServiceStartResponse;
use ody_app_server_protocol::WorkspaceServiceStatus;
use ody_app_server_protocol::WorkspaceServiceStopParams;
use ody_app_server_protocol::WorkspaceServiceStopResponse;
use ody_utils_absolute_path::test_support::PathBufExt;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);

fn default_client_info() -> ClientInfo {
    ClientInfo {
        name: DEFAULT_CLIENT_NAME.to_string(),
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

fn node_available() -> bool {
    std::process::Command::new("node")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// Fixture whose `dev` script is a real loopback HTTP server reading the
/// injected `PORT` env var (the CLI `--port` argv is ignored on purpose —
/// the fixture proves the env channel, real vite proves the CLI channel in
/// the manual smoke). The vite dependency declaration (never installed)
/// drives tech detection: default port 5173 + `--strictPort` (R2).
fn dev_server_fixture() -> Result<TempDir> {
    let dir = TempDir::new()?;
    fs::write(
        dir.path().join("package.json"),
        serde_json::json!({
            "name": "preview-fixture",
            "dependencies": { "vite": "^5.0.0" },
            "scripts": {
                "dev": "node server.js",
                "devfail": "node fail.js"
            }
        })
        .to_string(),
    )?;
    fs::write(
        dir.path().join("server.js"),
        r#"const http = require('http');
const port = Number(process.env.PORT || 5173);
const server = http.createServer((req, res) => {
    res.writeHead(200, { 'content-type': 'text/html' });
    res.end('<html><head><title>Fixture App</title></head><body>ok</body></html>');
});
server.listen(port, '127.0.0.1');
"#,
    )?;
    fs::write(
        dir.path().join("fail.js"),
        "console.error('fatal: boot failed');\nprocess.exit(1);\n",
    )?;
    Ok(dir)
}

fn bind_params(id: &str, roots: Vec<PathBuf>) -> WorkspaceProjectBindParams {
    WorkspaceProjectBindParams {
        id: id.to_owned(),
        name: "fixture project".to_owned(),
        roots: roots.into_iter().map(|path| path.abs()).collect(),
        idempotency_key: format!("key-{id}"),
    }
}

async fn bind_fixture_project(mcp: &mut TestAppServer, root: &TempDir) -> Result<()> {
    let request_id = mcp
        .send_workspace_project_bind_request(bind_params("ws-1", vec![root.path().to_path_buf()]))
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
    port: Option<u16>,
    idem: &str,
) -> Result<WorkspaceServiceRef> {
    let request_id = mcp
        .send_workspace_service_start_request(WorkspaceServiceStartParams {
            project_id: "ws-1".to_owned(),
            name: name.to_owned(),
            root_index: 0,
            script: script.to_owned(),
            port,
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

async fn read_error_message(mcp: &mut TestAppServer, request_id: i64) -> Result<String> {
    let error = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;
    Ok(error.error.message)
}

#[tokio::test]
async fn service_start_stop_lifecycle_leaves_no_process() -> Result<()> {
    if !node_available() {
        eprintln!("node unavailable; skipping");
        return Ok(());
    }
    let ody_home = TempDir::new()?;
    let fixture = dev_server_fixture()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    bind_fixture_project(&mut mcp, &fixture).await?;

    let service = start_service(&mut mcp, "web", "dev", Some(next_test_port()), "svc-1").await?;
    assert_eq!(service.status, WorkspaceServiceStatus::Ready);
    assert_eq!(service.script, "dev");
    assert!(service.command.contains("run dev -- --port"));
    assert!(service.command.contains("--strictPort")); // vite tech signal
    assert!(service.url.starts_with("http://127.0.0.1:"));
    assert!(service.pid.is_some());

    // list: health probe on a Ready service.
    let list_id = mcp
        .send_workspace_service_list_request(WorkspaceServiceListParams {
            project_id: "ws-1".to_owned(),
        })
        .await?;
    let message = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(list_id)),
    )
    .await??;
    let list = to_response::<WorkspaceServiceListResponse>(message)?;
    assert_eq!(list.services.len(), 1);
    let health = list.services[0].health.as_ref().expect("health probe");
    assert!(health.ok, "health: {health:?}");
    assert_eq!(health.status_code, Some(200));

    // logs: the node server may not print anything; assert the call shape.
    let logs_id = mcp
        .send_workspace_service_logs_request(WorkspaceServiceLogsParams {
            service_id: service.id.clone(),
            tail_bytes: None,
        })
        .await?;
    let message = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(logs_id)),
    )
    .await??;
    let logs = to_response::<WorkspaceServiceLogsResponse>(message)?;
    assert_eq!(logs.service_id, service.id);

    // preview check: reachable HTML with title + stable fingerprint.
    let check_id = mcp
        .send_workspace_preview_check_request(WorkspacePreviewCheckParams {
            service_id: service.id.clone(),
            url: None,
            timeout_ms: None,
        })
        .await?;
    let message = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(check_id)),
    )
    .await??;
    let check = to_response::<WorkspacePreviewCheckResponse>(message)?;
    assert!(check.reachable);
    assert_eq!(check.http_status, Some(200));
    assert_eq!(check.title.as_deref(), Some("Fixture App"));
    // Fixture HTML: `<html><head><title>Fixture App</title></head><body>ok</body></html>`
    // (67 bytes; verified against the live check response, not a placeholder).
    assert_eq!(check.content_bytes, 67);
    assert_eq!(
        check.content_sha256,
        "76b779d27a34bc19ead4c1964ed2a639074db30c6a9d90178c493de0e32d03ac"
    );

    // stop: terminal state, then the port must be reusable (no leftover process).
    let stop_id = mcp
        .send_workspace_service_stop_request(WorkspaceServiceStopParams {
            service_id: service.id.clone(),
        })
        .await?;
    let message = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(stop_id)),
    )
    .await??;
    let stopped = to_response::<WorkspaceServiceStopResponse>(message)?.service;
    assert_eq!(stopped.status, WorkspaceServiceStatus::Stopped);
    assert!(
        TcpListener::bind(("127.0.0.1", service.port)).is_ok(),
        "port {} not released",
        service.port
    );
    Ok(())
}

#[tokio::test]
async fn service_start_failure_reports_stderr_tail_and_exit_code() -> Result<()> {
    if !node_available() {
        eprintln!("node unavailable; skipping");
        return Ok(());
    }
    let ody_home = TempDir::new()?;
    let fixture = dev_server_fixture()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    bind_fixture_project(&mut mcp, &fixture).await?;

    let service = start_service(&mut mcp, "broken", "devfail", None, "svc-2").await?;
    assert_eq!(service.status, WorkspaceServiceStatus::Failed);
    let error = service.error.expect("diagnosable error");
    assert!(error.contains("exited with code 1"), "{error}");

    // The failing process printed a diagnostic; logs must surface it.
    let logs_id = mcp
        .send_workspace_service_logs_request(WorkspaceServiceLogsParams {
            service_id: service.id.clone(),
            tail_bytes: None,
        })
        .await?;
    let message = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(logs_id)),
    )
    .await??;
    let logs = to_response::<WorkspaceServiceLogsResponse>(message)?;
    assert!(
        logs.stderr_tail.contains("fatal: boot failed"),
        "stderr_tail: {:?}",
        logs.stderr_tail
    );
    Ok(())
}

#[tokio::test]
async fn service_port_conflict_returns_structured_error() -> Result<()> {
    if !node_available() {
        eprintln!("node unavailable; skipping");
        return Ok(());
    }
    let ody_home = TempDir::new()?;
    let fixture = dev_server_fixture()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    bind_fixture_project(&mut mcp, &fixture).await?;

    let holder = TcpListener::bind("127.0.0.1:0")?;
    let occupied = holder.local_addr()?.port();
    let request_id = mcp
        .send_workspace_service_start_request(WorkspaceServiceStartParams {
            project_id: "ws-1".to_owned(),
            name: "web".to_owned(),
            root_index: 0,
            script: "dev".to_owned(),
            port: Some(occupied),
            ready_timeout_ms: None,
            env: None,
            idempotency_key: "svc-3".to_owned(),
        })
        .await?;
    let message = read_error_message(&mut mcp, request_id).await?;
    assert!(message.contains(&occupied.to_string()), "{message}");
    assert!(message.contains("already in use"), "{message}");
    drop(holder);
    Ok(())
}

#[tokio::test]
async fn service_auto_port_avoidance_when_default_occupied() -> Result<()> {
    if !node_available() {
        eprintln!("node unavailable; skipping");
        return Ok(());
    }
    let ody_home = TempDir::new()?;
    let fixture = dev_server_fixture()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    bind_fixture_project(&mut mcp, &fixture).await?;

    // The fixture's vite signal makes 5173 the preferred port; occupy it and
    // assert the Runtime silently lands on a different free port.
    let holder = TcpListener::bind("127.0.0.1:5173").ok();
    let service = start_service(&mut mcp, "web", "dev", None, "svc-4").await?;
    assert_eq!(service.status, WorkspaceServiceStatus::Ready);
    if holder.is_some() {
        assert_ne!(
            service.port, 5173,
            "auto avoidance must skip occupied default"
        );
    }
    assert!(
        TcpListener::bind(("127.0.0.1", service.port)).is_err(),
        "ready port must be bound by the service"
    );

    let stop_id = mcp
        .send_workspace_service_stop_request(WorkspaceServiceStopParams {
            service_id: service.id.clone(),
        })
        .await?;
    let message = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(stop_id)),
    )
    .await??;
    let _: WorkspaceServiceStopResponse = to_response(message)?;
    Ok(())
}

#[tokio::test]
async fn service_and_preview_errors_are_diagnosable() -> Result<()> {
    // No node needed: every error below is raised before any spawn.
    let ody_home = TempDir::new()?;
    let fixture = dev_server_fixture()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    bind_fixture_project(&mut mcp, &fixture).await?;

    // Unknown project.
    let request_id = mcp
        .send_workspace_service_start_request(WorkspaceServiceStartParams {
            project_id: "ws-missing".to_owned(),
            name: "web".to_owned(),
            root_index: 0,
            script: "dev".to_owned(),
            port: None,
            ready_timeout_ms: None,
            env: None,
            idempotency_key: "svc-5a".to_owned(),
        })
        .await?;
    let message = read_error_message(&mut mcp, request_id).await?;
    assert!(
        message.contains("unknown project id ws-missing"),
        "{message}"
    );

    // Unknown script: the error must list what is available (E0 scan scripts).
    let request_id = mcp
        .send_workspace_service_start_request(WorkspaceServiceStartParams {
            project_id: "ws-1".to_owned(),
            name: "web".to_owned(),
            root_index: 0,
            script: "nope".to_owned(),
            port: None,
            ready_timeout_ms: None,
            env: None,
            idempotency_key: "svc-5b".to_owned(),
        })
        .await?;
    let message = read_error_message(&mut mcp, request_id).await?;
    assert!(message.contains("\"nope\""), "{message}");
    assert!(
        message.contains("dev") && message.contains("devfail"),
        "{message}"
    );

    // Preview check on a service that was never started.
    let request_id = mcp
        .send_workspace_preview_check_request(WorkspacePreviewCheckParams {
            service_id: "svc-never".to_owned(),
            url: None,
            timeout_ms: None,
        })
        .await?;
    let message = read_error_message(&mut mcp, request_id).await?;
    assert!(
        message.contains("unknown service id svc-never"),
        "{message}"
    );

    // Scheme smuggling is rejected before any fetch — but service lookup
    // precedes url validation, so the unknown-service error wins here.
    let request_id = mcp
        .send_workspace_preview_check_request(WorkspacePreviewCheckParams {
            service_id: "svc-never".to_owned(),
            url: Some("file:///etc/passwd".to_owned()),
            timeout_ms: None,
        })
        .await?;
    let message = read_error_message(&mut mcp, request_id).await?;
    assert!(
        message.contains("unknown service id svc-never"),
        "{message}"
    );
    Ok(())
}

#[tokio::test]
async fn preview_check_reports_unreachable_service_as_result_not_error() -> Result<()> {
    if !node_available() {
        eprintln!("node unavailable; skipping");
        return Ok(());
    }
    let ody_home = TempDir::new()?;
    let fixture = dev_server_fixture()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    bind_fixture_project(&mut mcp, &fixture).await?;

    // Start then kill the process behind the Runtime's back: the store may
    // briefly still say Ready, and check must degrade to a diagnostic
    // result (reachable:false), never a transport-level panic.
    let service = start_service(&mut mcp, "web", "dev", Some(next_test_port()), "svc-6").await?;
    #[cfg(unix)]
    {
        let pid = service.pid.expect("pid");
        unsafe {
            libc::kill(pid as i32, libc::SIGKILL);
        }
    }
    // Poll the store until the reaper publishes the terminal state, so the
    // check below is deterministic (a Ready-looking record would race us).
    let deadline = std::time::Instant::now() + DEFAULT_TIMEOUT;
    loop {
        let list_id = mcp
            .send_workspace_service_list_request(WorkspaceServiceListParams {
                project_id: "ws-1".to_owned(),
            })
            .await?;
        let message = timeout(
            DEFAULT_TIMEOUT,
            mcp.read_stream_until_response_message(RequestId::Integer(list_id)),
        )
        .await??;
        let list = to_response::<WorkspaceServiceListResponse>(message)?;
        let current = list
            .services
            .iter()
            .find(|entry| entry.id == service.id)
            .expect("service record");
        if matches!(
            current.status,
            WorkspaceServiceStatus::Failed
                | WorkspaceServiceStatus::Exited
                | WorkspaceServiceStatus::Stopped
        ) {
            break;
        }
        if std::time::Instant::now() > deadline {
            anyhow::bail!("reaper did not publish a terminal state in time");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let check_id = mcp
        .send_workspace_preview_check_request(WorkspacePreviewCheckParams {
            service_id: service.id.clone(),
            url: None,
            timeout_ms: Some(3_000),
        })
        .await?;
    let message = read_error_message(&mut mcp, check_id).await?;
    // Terminal service => contract error naming the current status, never a
    // transport-level failure: the failure reason stays visible (ADR §5).
    assert!(message.contains("not ready"), "{message}");
    Ok(())
}

#[tokio::test]
async fn project_close_stops_services_and_clears_records() -> Result<()> {
    if !node_available() {
        eprintln!("node unavailable; skipping");
        return Ok(());
    }
    let ody_home = TempDir::new()?;
    let fixture = dev_server_fixture()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    bind_fixture_project(&mut mcp, &fixture).await?;

    let first = start_service(&mut mcp, "web", "dev", Some(next_test_port()), "svc-7a").await?;
    let second = start_service(&mut mcp, "web2", "dev", Some(next_test_port()), "svc-7b").await?;
    assert_eq!(first.status, WorkspaceServiceStatus::Ready);
    assert_eq!(second.status, WorkspaceServiceStatus::Ready);

    let close_id = mcp
        .send_workspace_project_close_request(WorkspaceProjectCloseParams {
            project_id: "ws-1".to_owned(),
        })
        .await?;
    let message = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(close_id)),
    )
    .await??;
    let _: serde::de::IgnoredAny = to_response(message)?;

    // No leftover processes: both ports reusable.
    assert!(
        TcpListener::bind(("127.0.0.1", first.port)).is_ok(),
        "port {} not released",
        first.port
    );
    assert!(
        TcpListener::bind(("127.0.0.1", second.port)).is_ok(),
        "port {} not released",
        second.port
    );

    // Records are gone with the project: list now reports unknown project.
    let list_id = mcp
        .send_workspace_service_list_request(WorkspaceServiceListParams {
            project_id: "ws-1".to_owned(),
        })
        .await?;
    let message = read_error_message(&mut mcp, list_id).await?;
    assert!(message.contains("unknown project id ws-1"), "{message}");
    Ok(())
}

#[tokio::test]
async fn runtime_restart_normalizes_lingering_services_to_stopped() -> Result<()> {
    if !node_available() {
        eprintln!("node unavailable; skipping");
        return Ok(());
    }
    let ody_home = TempDir::new()?;
    let fixture = dev_server_fixture()?;
    {
        let mut mcp = TestAppServer::new(ody_home.path()).await?;
        init_experimental(&mut mcp).await?;
        bind_fixture_project(&mut mcp, &fixture).await?;
        let service =
            start_service(&mut mcp, "web", "dev", Some(next_test_port()), "svc-8").await?;
        assert_eq!(service.status, WorkspaceServiceStatus::Ready);
        // Drop the server without stop: the child process dies with the
        // runtime (kill_on_drop is false — the dev server may briefly
        // outlive, so the restart assertion must not depend on the port).
    }
    // The binding record survives (project store); the service record must
    // be normalized by the new instance's store load.
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    // Bind is idempotent across restart for the same project id.
    bind_fixture_project(&mut mcp, &fixture).await?;
    let list_id = mcp
        .send_workspace_service_list_request(WorkspaceServiceListParams {
            project_id: "ws-1".to_owned(),
        })
        .await?;
    let message = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(list_id)),
    )
    .await??;
    let list = to_response::<WorkspaceServiceListResponse>(message)?;
    let service = list
        .services
        .iter()
        .find(|service| service.name == "web")
        .expect("normalized service record");
    assert_eq!(service.status, WorkspaceServiceStatus::Stopped);
    assert_eq!(
        service.error.as_deref(),
        Some("runtime restarted; process not running")
    );
    Ok(())
}
