//! E4 exit criteria: four adversarial scenarios must never silently lose or
//! overwrite code (strategy §9.2): external IDE edit, Runtime restart,
//! service crash, dirty worktree. Every assertion binds disk bytes or a
//! protocol field — never logs.
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use app_test_support::TestAppServer;
use app_test_support::to_response;
use ody_app_server_protocol::ClientInfo;
use ody_app_server_protocol::InitializeCapabilities;
use ody_app_server_protocol::JSONRPCMessage;
use ody_app_server_protocol::JSONRPCNotification;
use ody_app_server_protocol::RequestId;
use ody_app_server_protocol::WorkspaceAuditListParams;
use ody_app_server_protocol::WorkspaceAuditListResponse;
use ody_app_server_protocol::WorkspaceChangeSet;
use ody_app_server_protocol::WorkspaceChangeSetApplyParams;
use ody_app_server_protocol::WorkspaceChangeSetCheckpoint;
use ody_app_server_protocol::WorkspaceChangeSetCreateParams;
use ody_app_server_protocol::WorkspaceChangeSetCreateResponse;
use ody_app_server_protocol::WorkspaceChangeSetGetParams;
use ody_app_server_protocol::WorkspaceChangeSetGetResponse;
use ody_app_server_protocol::WorkspaceChangeSetListParams;
use ody_app_server_protocol::WorkspaceChangeSetListResponse;
use ody_app_server_protocol::WorkspaceChangeSetRestoreParams;
use ody_app_server_protocol::WorkspaceChangeSetStatus;
use ody_app_server_protocol::WorkspaceFileChange;
use ody_app_server_protocol::WorkspaceFileChangeKind;
use ody_app_server_protocol::WorkspacePreviewCheckParams;
use ody_app_server_protocol::WorkspacePreviewCheckResponse;
use ody_app_server_protocol::WorkspacePreviewDiagnoseParams;
use ody_app_server_protocol::WorkspacePreviewDiagnoseResponse;
use ody_app_server_protocol::WorkspaceObservedNetworkFailure;
use ody_app_server_protocol::WorkspaceProjectBindParams;
use ody_app_server_protocol::WorkspaceProjectGetParams;
use ody_app_server_protocol::WorkspaceProjectGetResponse;
use ody_app_server_protocol::WorkspaceServiceDefineParams;
use ody_app_server_protocol::WorkspaceServiceHealthCheck;
use ody_app_server_protocol::WorkspaceServiceListParams;
use ody_app_server_protocol::WorkspaceServiceListResponse;
use ody_app_server_protocol::WorkspaceServiceRef;
use ody_app_server_protocol::WorkspaceServiceStartAllParams;
use ody_app_server_protocol::WorkspaceServiceStartAllResponse;
use ody_app_server_protocol::WorkspaceServiceStartParams;
use ody_app_server_protocol::WorkspaceServiceStartResponse;
use ody_app_server_protocol::WorkspaceServiceStatus;
use ody_app_server_protocol::WorkspaceServiceStopAllParams;
use ody_app_server_protocol::WorkspaceServiceStopAllResponse;
use ody_app_server_protocol::WorkspaceWatchParams;
use ody_utils_absolute_path::test_support::PathBufExt;
use sha2::Digest;
use sha2::Sha256;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);
const PROJECT: &str = "p1";

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

fn node_available() -> bool {
    std::process::Command::new("node")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn bind_params(id: &str, roots: Vec<PathBuf>) -> WorkspaceProjectBindParams {
    WorkspaceProjectBindParams {
        id: id.to_owned(),
        name: "reliability fixture".to_owned(),
        roots: roots.into_iter().map(|path| path.abs()).collect(),
        idempotency_key: format!("key-{id}"),
    }
}

async fn bind_project(mcp: &mut TestAppServer, roots: Vec<PathBuf>) -> Result<()> {
    let request_id = mcp
        .send_workspace_project_bind_request(bind_params(PROJECT, roots))
        .await?;
    read_response::<serde::de::IgnoredAny>(mcp, request_id).await?;
    Ok(())
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

async fn read_error(mcp: &mut TestAppServer, request_id: i64) -> Result<String> {
    let message = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;
    Ok(message.error.message)
}

async fn expect_notification(
    mcp: &mut TestAppServer,
    description: &str,
    matches: impl Fn(&JSONRPCNotification) -> bool,
) -> Result<JSONRPCNotification> {
    Ok(timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_matching_notification(description, matches),
    )
    .await??)
}

async fn create_update_changeset(
    mcp: &mut TestAppServer,
    path: &str,
    base: &str,
    content: &str,
    idem: &str,
) -> Result<WorkspaceChangeSet> {
    let request_id = mcp
        .send_workspace_source_changeset_create_request(WorkspaceChangeSetCreateParams {
            project_id: PROJECT.to_owned(),
            title: format!("update {path}"),
            changes: vec![WorkspaceFileChange {
                root_index: 0,
                path: path.to_owned(),
                kind: WorkspaceFileChangeKind::Update,
                base_hash: Some(sha256_hex(base.as_bytes())),
                content: Some(content.to_owned()),
            }],
            idempotency_key: idem.to_owned(),
        })
        .await?;
    Ok(read_response::<WorkspaceChangeSetCreateResponse>(mcp, request_id).await?.changeset)
}

async fn apply_changeset(
    mcp: &mut TestAppServer,
    changeset_id: &str,
) -> Result<WorkspaceChangeSet> {
    let request_id = mcp
        .send_workspace_source_changeset_apply_request(WorkspaceChangeSetApplyParams {
            changeset_id: changeset_id.to_owned(),
            commit_message: None,
        })
        .await?;
    Ok(read_response::<WorkspaceChangeSetGetResponse>(mcp, request_id)
        .await?
        .changeset)
}

// --- Scenario 1: simultaneous external IDE edit ----------------------------

#[tokio::test(flavor = "multi_thread")]
async fn external_ide_edit_invalidates_changeset_without_overwriting() -> Result<()> {
    let ody_home = TempDir::new()?;
    let fixture = TempDir::new()?;
    fs::create_dir_all(fixture.path().join("src"))?;
    fs::write(fixture.path().join("src/a.ts"), "old\n")?;
    let root = fs::canonicalize(fixture.path())?;

    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    bind_project(&mut mcp, vec![root.clone()]).await?;

    let watch_id = mcp
        .send_workspace_watch_request(WorkspaceWatchParams {
            project_id: PROJECT.to_owned(),
        })
        .await?;
    read_response::<serde::de::IgnoredAny>(&mut mcp, watch_id).await?;

    let cs = create_update_changeset(&mut mcp, "src/a.ts", "old\n", "agent\n", "cs-1").await?;

    // The IDE saves underneath the Runtime (bypasses every Ody write path).
    fs::write(fixture.path().join("src/a.ts"), "ide edit\n")?;

    let changed = expect_notification(&mut mcp, "workspace/changed invalidated", |n| {
        n.method == "workspace/changed"
            && n.params.as_ref().is_some_and(|p| {
                p["projectId"] == PROJECT
                    && p["changes"]
                        .as_array()
                        .is_some_and(|changes| changes.iter().any(|c| c["path"] == "src/a.ts"))
                    && p["invalidatedChangesets"]
                        .as_array()
                        .is_some_and(|ids| ids.iter().any(|id| id == &cs.id))
            })
    })
    .await?;
    let params = changed.params.expect("params");
    assert_eq!(params["overflow"], false);

    // The IDE's bytes survive untouched — no silent overwrite.
    assert_eq!(fs::read_to_string(fixture.path().join("src/a.ts"))?, "ide edit\n");

    // The invalidated changeset cannot apply.
    let request_id = mcp
        .send_workspace_source_changeset_apply_request(WorkspaceChangeSetApplyParams {
            changeset_id: cs.id.clone(),
            commit_message: None,
        })
        .await?;
    let err = read_error(&mut mcp, request_id).await?;
    assert!(err.contains("invalidated"), "got: {err}");
    assert_eq!(fs::read_to_string(fixture.path().join("src/a.ts"))?, "ide edit\n");

    // Work is not lost: re-base on the IDE's content and apply cleanly.
    let cs2 = create_update_changeset(&mut mcp, "src/a.ts", "ide edit\n", "agent\n", "cs-2").await?;
    let applied = apply_changeset(&mut mcp, &cs2.id).await?;
    assert_eq!(applied.status, WorkspaceChangeSetStatus::Applied);
    assert_eq!(fs::read_to_string(fixture.path().join("src/a.ts"))?, "agent\n");
    Ok(())
}

// --- Scenario 2: Runtime restart --------------------------------------------

/// Long-running service fixture plus a source file for the changeset.
fn restart_fixture() -> Result<TempDir> {
    let dir = TempDir::new()?;
    fs::write(
        dir.path().join("package.json"),
        serde_json::json!({
            "name": "restart-fixture",
            "scripts": { "dev": "node server.js" }
        })
        .to_string(),
    )?;
    fs::write(
        dir.path().join("server.js"),
        r#"const http = require('http');
const port = Number(process.env.PORT || 5173);
http.createServer((req, res) => {
    res.writeHead(200, { 'content-type': 'text/html' });
    res.end('<html><head><title>Restart Fixture</title></head><body>ok</body></html>');
}).listen(port, '127.0.0.1');
"#,
    )?;
    fs::create_dir_all(dir.path().join("src"))?;
    fs::write(dir.path().join("src/a.ts"), "old\n")?;
    Ok(dir)
}

async fn start_one_service(
    mcp: &mut TestAppServer,
    name: &str,
    script: &str,
    port: Option<u16>,
    idem: &str,
) -> Result<WorkspaceServiceRef> {
    let request_id = mcp
        .send_workspace_service_start_request(WorkspaceServiceStartParams {
            project_id: PROJECT.to_owned(),
            name: name.to_owned(),
            root_index: 0,
            script: script.to_owned(),
            port,
            ready_timeout_ms: Some(10_000),
            env: None,
            secret_values: None,
            idempotency_key: idem.to_owned(),
        })
        .await?;
    Ok(read_response::<WorkspaceServiceStartResponse>(mcp, request_id).await?.service)
}

#[tokio::test(flavor = "multi_thread")]
async fn runtime_restart_normalizes_and_audits_without_data_loss() -> Result<()> {
    if !node_available() {
        eprintln!("skipping: node is not available");
        return Ok(());
    }
    let ody_home = TempDir::new()?;
    let fixture = restart_fixture()?;
    let root = fs::canonicalize(fixture.path())?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    bind_project(&mut mcp, vec![root]).await?;

    let service = start_one_service(
        &mut mcp,
        "api",
        "dev",
        Some(app_test_support::next_test_port()),
        "start-api",
    )
    .await?;
    assert_eq!(service.status, WorkspaceServiceStatus::Ready);
    let service_pid = service.pid;

    let cs = create_update_changeset(&mut mcp, "src/a.ts", "old\n", "new\n", "cs-r1").await?;

    // Simulate a Runtime crash (SIGKILL): no graceful persist, no cleanup.
    mcp.kill_hard()?;
    drop(mcp);

    let mut mcp2 = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp2).await?;

    // The restart is announced and the service normalizes to an attributable
    // terminal state.
    let notice = expect_notification(&mut mcp2, "service/changed runtimeRestarted", |n| {
        n.method == "workspace/service/changed"
            && n.params
                .as_ref()
                .is_some_and(|p| p["reason"] == "runtimeRestarted")
    })
    .await?;
    let params = notice.params.expect("params");
    assert_eq!(params["services"][0]["name"], "api");

    let request_id = mcp2
        .send_workspace_service_list_request(WorkspaceServiceListParams {
            project_id: PROJECT.to_owned(),
        })
        .await?;
    let services = read_response::<WorkspaceServiceListResponse>(&mut mcp2, request_id)
        .await?
        .services;
    let api = services.iter().find(|s| s.name == "api").expect("api record");
    assert_eq!(api.status, WorkspaceServiceStatus::Stopped);
    assert!(
        api.error
            .as_deref()
            .is_some_and(|error| error.contains("runtime restarted")),
        "error should explain the restart, got {:?}",
        api.error
    );

    // Project, changeset, and audit trail all survive the restart.
    let request_id = mcp2
        .send_workspace_project_get_request(WorkspaceProjectGetParams {
            project_id: PROJECT.to_owned(),
        })
        .await?;
    let project = read_response::<WorkspaceProjectGetResponse>(&mut mcp2, request_id).await?.project;
    assert_eq!(project.roots.len(), 1);

    let request_id = mcp2
        .send_workspace_source_changeset_list_request(WorkspaceChangeSetListParams {
            project_id: PROJECT.to_owned(),
        })
        .await?;
    let changesets = read_response::<WorkspaceChangeSetListResponse>(&mut mcp2, request_id).await?;
    assert_eq!(changesets.changesets.len(), 1);
    assert_eq!(changesets.changesets[0].id, cs.id);

    let request_id = mcp2
        .send_workspace_audit_list_request(WorkspaceAuditListParams {
            project_id: Some(PROJECT.to_owned()),
            limit: Some(50),
        })
        .await?;
    let audit = read_response::<WorkspaceAuditListResponse>(&mut mcp2, request_id).await?;
    assert!(
        audit.events.iter().any(|event| event.operation == "project.bind"),
        "audit trail must span the restart, got {:?}",
        audit.events.iter().map(|e| &e.operation).collect::<Vec<_>>()
    );

    // The SIGKILLed Runtime orphaned its service process tree; clean it up
    // so the test leaves no stray process behind.
    #[cfg(unix)]
    if let Some(pid) = service_pid {
        let _ = std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status();
    }
    Ok(())
}

// --- Scenario 3: service crash ----------------------------------------------

/// Backend that crashes 100ms after boot; frontend proxies /api/* to the
/// injected BACKEND_URL (dependency env channel).
fn crash_backend_fixture() -> Result<TempDir> {
    let dir = TempDir::new()?;
    fs::write(
        dir.path().join("package.json"),
        serde_json::json!({
            "name": "crash-backend",
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
const port = Number(process.env.PORT || 8787);
http.createServer((req, res) => {
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify({ ok: true, route: req.url }));
}).listen(port, '127.0.0.1');
"#,
    )?;
    Ok(dir)
}

fn crash_frontend_fixture() -> Result<TempDir> {
    let dir = TempDir::new()?;
    fs::write(
        dir.path().join("package.json"),
        serde_json::json!({
            "name": "crash-frontend",
            "dependencies": { "vite": "^5.0.0" },
            "scripts": { "dev": "node server.js" }
        })
        .to_string(),
    )?;
    fs::write(
        dir.path().join("server.js"),
        r#"const http = require('http');
const backend = process.env.BACKEND_URL;
const port = Number(process.env.PORT || 5173);
http.createServer((req, res) => {
    if (req.url.startsWith('/api/')) {
        if (!backend) {
            res.writeHead(502).end('BACKEND_URL not injected');
            return;
        }
        http.get(backend.replace(/\/$/, '') + req.url, (up) => {
            res.writeHead(up.statusCode, { 'content-type': up.headers['content-type'] || 'application/json' });
            up.pipe(res);
        }).on('error', (err) => {
            console.error(`upstream error: ${err.message}`);
            res.writeHead(502).end('upstream: ' + err.message);
        });
        return;
    }
    res.writeHead(200, { 'content-type': 'text/html' });
    res.end('<html><head><title>Crash Frontend</title></head><body>page</body></html>');
}).listen(port, '127.0.0.1');
"#,
    )?;
    Ok(dir)
}

async fn define_spec(
    mcp: &mut TestAppServer,
    name: &str,
    root_index: u32,
    script: &str,
    depends_on: Vec<&str>,
    health_path: Option<&str>,
    idem: &str,
) -> Result<()> {
    let request_id = mcp
        .send_workspace_service_define_request(WorkspaceServiceDefineParams {
            project_id: PROJECT.to_owned(),
            name: name.to_owned(),
            root_index,
            script: script.to_owned(),
            port: Some(app_test_support::next_test_port()),
            cwd: None,
            depends_on: depends_on.iter().map(|s| s.to_string()).collect(),
            health_check: health_path.map(|path| WorkspaceServiceHealthCheck {
                path: path.to_owned(),
                expect_status: Some(200),
                timeout_ms: None,
            }),
            ready_timeout_ms: Some(10_000),
            env_refs: None,
            idempotency_key: idem.to_owned(),
        })
        .await?;
    read_response::<serde::de::IgnoredAny>(mcp, request_id).await?;
    Ok(())
}

async fn start_all_specs(
    mcp: &mut TestAppServer,
    names: Vec<&str>,
    idem: &str,
) -> Result<WorkspaceServiceStartAllResponse> {
    let request_id = mcp
        .send_workspace_service_start_all_request(WorkspaceServiceStartAllParams {
            project_id: PROJECT.to_owned(),
            names: names.iter().map(|s| s.to_string()).collect(),
            secret_values: None,
            idempotency_key: idem.to_owned(),
        })
        .await?;
    read_response::<WorkspaceServiceStartAllResponse>(mcp, request_id).await
}

async fn stop_all_specs(mcp: &mut TestAppServer) -> Result<()> {
    let request_id = mcp
        .send_workspace_service_stop_all_request(WorkspaceServiceStopAllParams {
            project_id: PROJECT.to_owned(),
            names: Vec::new(),
        })
        .await?;
    let _: WorkspaceServiceStopAllResponse = read_response(mcp, request_id).await?;
    Ok(())
}

async fn preview_check(
    mcp: &mut TestAppServer,
    service_id: &str,
    url: Option<String>,
) -> Result<WorkspacePreviewCheckResponse> {
    let request_id = mcp
        .send_workspace_preview_check_request(WorkspacePreviewCheckParams {
            service_id: service_id.to_owned(),
            url,
            timeout_ms: None,
        })
        .await?;
    read_response::<WorkspacePreviewCheckResponse>(mcp, request_id).await
}

#[tokio::test(flavor = "multi_thread")]
async fn service_crash_notifies_and_diagnose_still_correlates() -> Result<()> {
    if !node_available() {
        eprintln!("skipping: node is not available");
        return Ok(());
    }
    let ody_home = TempDir::new()?;
    let frontend = crash_frontend_fixture()?;
    let backend = crash_backend_fixture()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    // Frontend = root 0 (primary), backend = root 1.
    bind_project(
        &mut mcp,
        vec![
            fs::canonicalize(frontend.path())?,
            fs::canonicalize(backend.path())?,
        ],
    )
    .await?;
    define_spec(&mut mcp, "backend", 1, "crash", vec![], None, "spec-backend").await?;
    define_spec(
        &mut mcp,
        "frontend",
        0,
        "dev",
        vec!["backend"],
        None,
        "spec-frontend",
    )
    .await?;

    let first = start_all_specs(&mut mcp, vec![], "orch-1").await?;
    let crashed = first
        .services
        .iter()
        .find(|service| service.name == "backend")
        .expect("backend record");
    let backend_port = crashed.port;

    // The crash reaches the client with the real exit code (no polling).
    let notice = expect_notification(&mut mcp, "service/changed failed exitCode 3", |n| {
        n.method == "workspace/service/changed"
            && n.params.as_ref().is_some_and(|p| {
                p["reason"] == "failed"
                    && p["services"].as_array().is_some_and(|services| {
                        services.iter().any(|s| s["exitCode"] == 3)
                    })
            })
    })
    .await?;
    let _ = notice;

    // Diagnosis still correlates: a loopback failure on the backend origin
    // surfaces the crashed backend (preferred recent terminal record).
    let request_id = mcp
        .send_workspace_preview_diagnose_request(WorkspacePreviewDiagnoseParams {
            project_id: PROJECT.to_owned(),
            failures: vec![WorkspaceObservedNetworkFailure {
                url: format!("http://127.0.0.1:{backend_port}/api/items"),
                method: Some("GET".to_owned()),
                status: None,
                error: Some("net::ERR_CONNECTION_REFUSED".to_owned()),
                occurred_at_ms: None,
            }],
            log_tail_bytes: None,
            max_candidates: None,
        })
        .await?;
    let diagnosis = read_response::<WorkspacePreviewDiagnoseResponse>(&mut mcp, request_id).await?;
    let matched = diagnosis.results[0]
        .matched_service
        .as_ref()
        .expect("crashed backend must be diagnosable");
    assert_eq!(matched.name, "backend");
    assert_eq!(matched.port, backend_port);

    // Fix the backend, re-orchestrate, and the frontend proxy serves again —
    // capability is restored, never silently lost.
    fs::write(
        backend.path().join("crasher.js"),
        fs::read_to_string(backend.path().join("server.js"))?,
    )?;
    stop_all_specs(&mut mcp).await?;
    let second = start_all_specs(&mut mcp, vec![], "orch-2").await?;
    let frontend_ref = second
        .services
        .iter()
        .find(|service| service.name == "frontend")
        .expect("frontend record");
    assert_eq!(frontend_ref.status, WorkspaceServiceStatus::Ready);
    let backend_ref = second
        .services
        .iter()
        .find(|service| service.name == "backend")
        .expect("backend record");
    assert_eq!(backend_ref.status, WorkspaceServiceStatus::Ready);

    let proxied = preview_check(
        &mut mcp,
        &frontend_ref.id,
        Some(format!("http://127.0.0.1:{}/api/items", frontend_ref.port)),
    )
    .await?;
    assert_eq!(
        proxied.http_status,
        Some(200),
        "frontend proxy must recover after re-orchestration: {proxied:?}"
    );
    Ok(())
}

// --- Scenario 4: dirty worktree ----------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn dirty_worktree_never_loses_user_changes() -> Result<()> {
    let ody_home = TempDir::new()?;
    let fixture = TempDir::new()?;
    fs::create_dir_all(fixture.path().join("src"))?;
    fs::write(fixture.path().join("src/a.ts"), "base\n")?;
    fs::write(fixture.path().join("src/user-owned.ts"), "original\n")?;
    // Deliberately NOT a git repo: the gix-baseline checkpoint path applies.
    let root = fs::canonicalize(fixture.path())?;

    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    bind_project(&mut mcp, vec![root]).await?;

    let cs = create_update_changeset(&mut mcp, "src/a.ts", "base\n", "agent\n", "cs-d1").await?;

    // The user's unsaved-to-Ody work in another file (not in the changeset).
    fs::write(fixture.path().join("src/user-owned.ts"), "user work in progress\n")?;

    let applied = apply_changeset(&mut mcp, &cs.id).await?;
    assert_eq!(applied.status, WorkspaceChangeSetStatus::Applied);
    // The changeset wrote only its declared path; user bytes are untouched.
    assert_eq!(fs::read_to_string(fixture.path().join("src/user-owned.ts"))?, "user work in progress\n");
    assert_eq!(fs::read_to_string(fixture.path().join("src/a.ts"))?, "agent\n");

    // A checkpoint was established (no git root -> gix baseline, recorded as
    // Git with no HEAD), so restore can put the world back.
    let request_id = mcp
        .send_workspace_source_changeset_get_request(WorkspaceChangeSetGetParams {
            changeset_id: cs.id.clone(),
        })
        .await?;
    let stored = read_response::<WorkspaceChangeSetGetResponse>(&mut mcp, request_id).await?;
    assert!(
        matches!(stored.changeset.checkpoint, WorkspaceChangeSetCheckpoint::Git { .. }),
        "checkpoint must be established, got {:?}",
        stored.changeset.checkpoint
    );

    let request_id = mcp
        .send_workspace_source_changeset_restore_request(WorkspaceChangeSetRestoreParams {
            changeset_id: cs.id.clone(),
        })
        .await?;
    let restored = read_response::<WorkspaceChangeSetGetResponse>(&mut mcp, request_id).await?;
    assert_eq!(restored.changeset.status, WorkspaceChangeSetStatus::Restored);
    assert_eq!(fs::read_to_string(fixture.path().join("src/a.ts"))?, "base\n");
    assert_eq!(fs::read_to_string(fixture.path().join("src/user-owned.ts"))?, "user work in progress\n");
    Ok(())
}
