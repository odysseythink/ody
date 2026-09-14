//! E3 acceptance: fullstack orchestration (dual-root + monorepo),
//! preview diagnosis, contract-mismatch fix loop, cross-root
//! validation/diff, and cleanup discipline. Node-gated where a real
//! process tree is required (E2 precedent).

use std::fs;
use std::net::TcpListener;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use app_test_support::DEFAULT_CLIENT_NAME;
use app_test_support::TestAppServer;
use app_test_support::to_response;
use ody_app_server_protocol::ClientInfo;
use ody_app_server_protocol::InitializeCapabilities;
use ody_app_server_protocol::JSONRPCMessage;
use ody_app_server_protocol::RequestId;
use ody_app_server_protocol::WorkspaceChangeSetApplyParams;
use ody_app_server_protocol::WorkspaceChangeSetCreateParams;
use ody_app_server_protocol::WorkspaceFileChange;
use ody_app_server_protocol::WorkspaceFileChangeKind;
use ody_app_server_protocol::WorkspaceNetworkDiagnosis;
use ody_app_server_protocol::WorkspaceObservedNetworkFailure;
use ody_app_server_protocol::WorkspacePreviewCheckParams;
use ody_app_server_protocol::WorkspacePreviewCheckResponse;
use ody_app_server_protocol::WorkspacePreviewDiagnoseParams;
use ody_app_server_protocol::WorkspacePreviewDiagnoseResponse;
use ody_app_server_protocol::WorkspaceProjectBindParams;
use ody_app_server_protocol::WorkspaceProjectCloseParams;
use ody_app_server_protocol::WorkspaceServiceDefineParams;
use ody_app_server_protocol::WorkspaceServiceHealthCheck;
use ody_app_server_protocol::WorkspaceServiceHealthParams;
use ody_app_server_protocol::WorkspaceServiceHealthResponse;
use ody_app_server_protocol::WorkspaceServiceSpec;
use ody_app_server_protocol::WorkspaceServiceSpecsParams;
use ody_app_server_protocol::WorkspaceServiceSpecsResponse;
use ody_app_server_protocol::WorkspaceServiceStartAllParams;
use ody_app_server_protocol::WorkspaceServiceStartAllResponse;
use ody_app_server_protocol::WorkspaceServiceStatus;
use ody_app_server_protocol::WorkspaceServiceStopAllParams;
use ody_app_server_protocol::WorkspaceServiceStopAllResponse;
use ody_app_server_protocol::WorkspaceSourceDiffParams;
use ody_app_server_protocol::WorkspaceSourceDiffResponse;
use ody_app_server_protocol::WorkspaceSourceValidateParams;
use ody_app_server_protocol::WorkspaceSourceValidateResponse;
use ody_app_server_protocol::WorkspaceValidationCheck;
use ody_app_server_protocol::WorkspaceValidationKind;
use ody_utils_absolute_path::test_support::PathBufExt;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60); // startAll waits ready per service

/// Every spec gets an explicit port from the shared test allocator: parallel
/// tests otherwise collide on framework-fallback ports (the tech-less backend
/// fixture falls back to vite's 5173 too), and `pick_port`'s free-probe races
/// the child process's bind (EADDRINUSE -> exit 1).

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

// --- fixtures ------------------------------------------------------------

/// Backend root: dynamic route server — `/api/<stem>` is live iff
/// `src/routes/<stem>.js` exists on disk (read per request), so a changeset
/// that adds a route file makes the endpoint callable with no restart.
/// Built-ins: `/api/health` 200, `/api/broken` 500; 404s are logged to
/// stdout for the diagnose excerpt assertion.
fn backend_fixture() -> Result<TempDir> {
    let dir = TempDir::new()?;
    fs::write(
        dir.path().join("package.json"),
        serde_json::json!({
            "name": "backend-fixture",
            "scripts": {
                "dev": "node server.js",
                "test": "node test.js",
                "build": "node build.js"
            }
        })
        .to_string(),
    )?;
    fs::create_dir_all(dir.path().join("src/routes"))?;
    fs::write(
        dir.path().join("src/routes/items.js"),
        "// items route fixture\nexport const items = true;\n",
    )?;
    fs::write(
        dir.path().join("server.js"),
        r#"const http = require('http');
const fs = require('fs');
const path = require('path');
const port = Number(process.env.PORT || 8787);
const routesDir = path.join(__dirname, 'src', 'routes');
const stems = () => {
    try {
        return fs.readdirSync(routesDir).filter((f) => f.endsWith('.js')).map((f) => f.replace(/\.js$/, ''));
    } catch { return []; }
};
const server = http.createServer((req, res) => {
    const url = new URL(req.url, 'http://x');
    const stem = url.pathname.replace(/^\/api\//, '');
    if (url.pathname === '/') {
        // Readiness probe target: start_one gates on a 2xx/3xx root response.
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(JSON.stringify({ ok: true, service: 'backend' }));
    } else if (url.pathname === '/api/health') {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(JSON.stringify({ ok: true }));
    } else if (url.pathname === '/api/broken') {
        res.writeHead(500).end('broken');
    } else if (stems().includes(stem)) {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(JSON.stringify({ route: stem }));
    } else {
        console.log(`[req] ${req.method} ${url.pathname} 404`);
        res.writeHead(404).end('not found');
    }
});
server.listen(port, '127.0.0.1');
"#,
    )?;
    fs::write(
        dir.path().join("test.js"),
        "console.log('backend tests pass');\nprocess.exit(0);\n",
    )?;
    fs::write(
        dir.path().join("build.js"),
        "require('fs').mkdirSync('dist', { recursive: true });\nrequire('fs').writeFileSync('dist/out.txt', 'built');\nconsole.log('backend built');\n",
    )?;
    Ok(dir)
}

/// Frontend root: reads the injected `BACKEND_URL` (env channel) and proves
/// it two ways: the index HTML embeds it (`data-backend`), and `/api/*`
/// requests are proxied to it (real end-to-end linkage through the
/// frontend origin). Vite dependency declaration drives tech detection.
fn frontend_fixture() -> Result<TempDir> {
    let dir = TempDir::new()?;
    fs::write(
        dir.path().join("package.json"),
        serde_json::json!({
            "name": "frontend-fixture",
            "dependencies": { "vite": "^5.0.0" },
            "scripts": {
                "dev": "node server.js",
                "build": "node build.js"
            }
        })
        .to_string(),
    )?;
    fs::write(
        dir.path().join("server.js"),
        r#"const http = require('http');
const backend = process.env.BACKEND_URL;
const port = Number(process.env.PORT || 5173);
const server = http.createServer((req, res) => {
    if (req.url.startsWith('/api/')) {
        if (!backend) {
            console.error('BACKEND_URL not injected');
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
    res.end(`<html><head><title>Storefront</title></head><body data-backend="${backend || ''}">page ${req.url}</body></html>`);
});
server.listen(port, '127.0.0.1');
"#,
    )?;
    fs::write(
        dir.path().join("build.js"),
        "require('fs').mkdirSync('dist', { recursive: true });\nrequire('fs').writeFileSync('dist/index.html', 'built');\nconsole.log('frontend built');\n",
    )?;
    Ok(dir)
}

/// Single-root monorepo: backend/ and frontend/ sub-packages, each with
/// its own package.json (spec.cwd targets these subdirs).
fn monorepo_fixture() -> Result<TempDir> {
    let dir = TempDir::new()?;
    let backend = backend_fixture()?;
    let frontend = frontend_fixture()?;
    fs::create_dir_all(dir.path().join("backend"))?;
    fs::create_dir_all(dir.path().join("frontend"))?;
    for name in ["package.json", "server.js", "test.js", "build.js"] {
        fs::copy(
            backend.path().join(name),
            dir.path().join("backend").join(name),
        )?;
    }
    fs::create_dir_all(dir.path().join("backend/src/routes"))?;
    fs::copy(
        backend.path().join("src/routes/items.js"),
        dir.path().join("backend/src/routes/items.js"),
    )?;
    for name in ["package.json", "server.js", "build.js"] {
        fs::copy(
            frontend.path().join(name),
            dir.path().join("frontend").join(name),
        )?;
    }
    Ok(dir)
}

// --- helpers --------------------------------------------------------------

fn bind_params(id: &str, roots: Vec<PathBuf>) -> WorkspaceProjectBindParams {
    WorkspaceProjectBindParams {
        id: id.to_owned(),
        name: "fullstack fixture".to_owned(),
        roots: roots.into_iter().map(|path| path.abs()).collect(),
        idempotency_key: format!("key-{id}"),
    }
}

async fn bind_project(mcp: &mut TestAppServer, id: &str, roots: Vec<PathBuf>) -> Result<()> {
    let request_id = mcp
        .send_workspace_project_bind_request(bind_params(id, roots))
        .await?;
    let message = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let _: serde::de::IgnoredAny = to_response(message)?;
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

async fn define_spec(
    mcp: &mut TestAppServer,
    name: &str,
    root_index: u32,
    script: &str,
    cwd: Option<&str>,
    depends_on: Vec<&str>,
    health_path: Option<&str>,
    idem: &str,
) -> Result<WorkspaceServiceSpec> {
    let request_id = mcp
        .send_workspace_service_define_request(WorkspaceServiceDefineParams {
            project_id: "ws-1".to_owned(),
            name: name.to_owned(),
            root_index,
            script: script.to_owned(),
            port: Some(app_test_support::next_test_port()),
            cwd: cwd.map(str::to_owned),
            depends_on: depends_on.iter().map(|s| s.to_string()).collect(),
            health_check: health_path.map(|path| WorkspaceServiceHealthCheck {
                path: path.to_owned(),
                expect_status: Some(200),
                timeout_ms: None,
            }),
            ready_timeout_ms: None,
            idempotency_key: idem.to_owned(),
        })
        .await?;
    let response: ody_app_server_protocol::WorkspaceServiceDefineResponse =
        read_response(mcp, request_id).await?;
    Ok(response.spec)
}

async fn start_all(
    mcp: &mut TestAppServer,
    names: Vec<&str>,
    idem: &str,
) -> Result<WorkspaceServiceStartAllResponse> {
    let request_id = mcp
        .send_workspace_service_start_all_request(WorkspaceServiceStartAllParams {
            project_id: "ws-1".to_owned(),
            names: names.iter().map(|s| s.to_string()).collect(),
            idempotency_key: idem.to_owned(),
        })
        .await?;
    read_response(mcp, request_id).await
}

async fn check(
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
    read_response(mcp, request_id).await
}

/// Dual-root bind: frontend = root 0 (primary), backend = root 1.
async fn bind_dual_root(
    mcp: &mut TestAppServer,
    frontend: &TempDir,
    backend: &TempDir,
) -> Result<()> {
    bind_project(
        mcp,
        "ws-1",
        vec![frontend.path().to_path_buf(), backend.path().to_path_buf()],
    )
    .await
}

async fn define_default_specs(mcp: &mut TestAppServer) -> Result<()> {
    define_spec(
        mcp,
        "backend",
        1,
        "dev",
        None,
        vec![],
        Some("/api/health"),
        "spec-backend",
    )
    .await?;
    define_spec(
        mcp,
        "web",
        0,
        "dev",
        None,
        vec!["backend"],
        None,
        "spec-web",
    )
    .await?;
    Ok(())
}

// --- 1. define validation (no node required) -------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn define_rejects_unknown_dep_cycle_and_out_of_bounds_root() -> Result<()> {
    let ody_home = TempDir::new()?;
    let backend = backend_fixture()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    bind_project(&mut mcp, "ws-1", vec![backend.path().to_path_buf()]).await?;

    // Out-of-bounds root_index.
    let bad_id = mcp
        .send_workspace_service_define_request(WorkspaceServiceDefineParams {
            project_id: "ws-1".to_owned(),
            name: "backend".to_owned(),
            root_index: 9,
            script: "dev".to_owned(),
            port: None,
            cwd: None,
            depends_on: vec![],
            health_check: None,
            ready_timeout_ms: None,
            idempotency_key: "bad-root".to_owned(),
        })
        .await?;
    let message = read_error(&mut mcp, bad_id).await?;
    assert!(
        message.contains("root_index 9") && message.contains("1 roots"),
        "{message}"
    );

    // Unknown dependency.
    let bad_id = mcp
        .send_workspace_service_define_request(WorkspaceServiceDefineParams {
            project_id: "ws-1".to_owned(),
            name: "backend".to_owned(),
            root_index: 0,
            script: "dev".to_owned(),
            port: None,
            cwd: None,
            depends_on: vec!["ghost".to_owned()],
            health_check: None,
            ready_timeout_ms: None,
            idempotency_key: "bad-dep".to_owned(),
        })
        .await?;
    let message = read_error(&mut mcp, bad_id).await?;
    assert!(message.contains("\"ghost\""), "{message}");

    // Happy path, then a cycle through upsert: web -> backend, backend -> web.
    define_spec(
        &mut mcp,
        "backend",
        0,
        "dev",
        None,
        vec![],
        Some("/api/health"),
        "s-backend",
    )
    .await?;
    define_spec(
        &mut mcp,
        "web",
        0,
        "dev",
        None,
        vec!["backend"],
        None,
        "s-web",
    )
    .await?;
    let bad_id = mcp
        .send_workspace_service_define_request(WorkspaceServiceDefineParams {
            project_id: "ws-1".to_owned(),
            name: "backend".to_owned(),
            root_index: 0,
            script: "dev".to_owned(),
            port: None,
            cwd: None,
            depends_on: vec!["web".to_owned()],
            health_check: None,
            ready_timeout_ms: None,
            idempotency_key: "s-backend-cycle".to_owned(),
        })
        .await?;
    let message = read_error(&mut mcp, bad_id).await?;
    assert!(message.contains("cycle"), "{message}");
    Ok(())
}

// --- 2. orchestration order + env injection (dual root) --------------------

#[tokio::test(flavor = "multi_thread")]
async fn start_all_dual_root_orders_and_injects_dependency_env() -> Result<()> {
    if !node_available() {
        eprintln!("skipping: node is not available");
        return Ok(());
    }
    let ody_home = TempDir::new()?;
    let frontend = frontend_fixture()?;
    let backend = backend_fixture()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    bind_dual_root(&mut mcp, &frontend, &backend).await?;
    define_default_specs(&mut mcp).await?;

    let response = start_all(&mut mcp, vec![], "orch-1").await?;
    assert!(response.failed.is_empty(), "failed: {:?}", response.failed);
    assert_eq!(response.started.len(), 2);
    assert!(response.reused.is_empty());
    let backend_ref = response
        .services
        .iter()
        .find(|service| service.name == "backend")
        .expect("backend service");
    let web_ref = response
        .services
        .iter()
        .find(|service| service.name == "web")
        .expect("web service");
    assert_eq!(backend_ref.status, WorkspaceServiceStatus::Ready);
    assert_eq!(web_ref.status, WorkspaceServiceStatus::Ready);
    // Dependency order: web was created after backend became ready.
    assert!(
        web_ref.created_at_ms >= backend_ref.updated_at_ms,
        "backend must be ready before web starts: backend={:?} web={:?}",
        backend_ref,
        web_ref
    );

    // Env injection proof: the frontend HTML answers and carries the
    // Storefront title; the proxied API call below returning 200 JSON from
    // the backend is the end-to-end linkage proof (BACKEND_URL was visible
    // to the frontend process at spawn time).
    let page = check(&mut mcp, &web_ref.id, None).await?;
    assert_eq!(page.http_status, Some(200));
    assert_eq!(page.title.as_deref(), Some("Storefront"));
    let proxied = check(
        &mut mcp,
        &web_ref.id,
        Some(format!("http://127.0.0.1:{}/api/items", web_ref.port)),
    )
    .await?;
    assert_eq!(
        proxied.http_status,
        Some(200),
        "proxy to backend failed: {proxied:?}"
    );
    Ok(())
}

// --- 3. health method -------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn health_method_uses_spec_health_check_and_expect_status() -> Result<()> {
    if !node_available() {
        eprintln!("skipping: node is not available");
        return Ok(());
    }
    let ody_home = TempDir::new()?;
    let backend = backend_fixture()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    bind_project(&mut mcp, "ws-1", vec![backend.path().to_path_buf()]).await?;
    define_spec(
        &mut mcp,
        "backend",
        0,
        "dev",
        None,
        vec![],
        Some("/api/health"),
        "spec-h",
    )
    .await?;
    let response = start_all(&mut mcp, vec!["backend"], "orch-h").await?;
    let service = response.services.first().expect("service");

    let request_id = mcp
        .send_workspace_service_health_request(WorkspaceServiceHealthParams {
            service_id: service.id.clone(),
            path: None,
        })
        .await?;
    let health: WorkspaceServiceHealthResponse = read_response(&mut mcp, request_id).await?;
    assert!(health.health.ok, "{health:?}");
    assert_eq!(health.probed_path, "/api/health");
    assert_eq!(health.expected_status, Some(200));

    // Explicit path override without expectation: 500 answers, 2xx tolerance fails.
    let request_id = mcp
        .send_workspace_service_health_request(WorkspaceServiceHealthParams {
            service_id: service.id.clone(),
            path: Some("/api/broken".to_owned()),
        })
        .await?;
    let health: WorkspaceServiceHealthResponse = read_response(&mut mcp, request_id).await?;
    assert!(
        !health.health.ok,
        "500 must fail a 2xx-tolerant probe: {health:?}"
    );
    assert_eq!(health.health.status_code, Some(500));
    Ok(())
}

// --- 4. idempotent replay ---------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn start_all_idempotent_replay_does_not_respawn() -> Result<()> {
    if !node_available() {
        eprintln!("skipping: node is not available");
        return Ok(());
    }
    let ody_home = TempDir::new()?;
    let backend = backend_fixture()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    bind_project(&mut mcp, "ws-1", vec![backend.path().to_path_buf()]).await?;
    define_spec(
        &mut mcp,
        "backend",
        0,
        "dev",
        None,
        vec![],
        Some("/api/health"),
        "spec-i",
    )
    .await?;

    let first = start_all(&mut mcp, vec![], "orch-idem").await?;
    assert_eq!(first.started.len(), 1);
    let pid = first.services[0].pid;

    // Retry with the same key: replayed verbatim, nothing respawned.
    let second = start_all(&mut mcp, vec![], "orch-idem").await?;
    assert!(
        second.started.is_empty(),
        "replay must not respawn: {second:?}"
    );
    assert_eq!(
        second.services[0].pid, pid,
        "same process behind the replay"
    );

    // Fresh key with the service still active: reuse, not respawn.
    let third = start_all(&mut mcp, vec![], "orch-idem-2").await?;
    assert!(third.started.is_empty());
    assert_eq!(third.reused.len(), 1);
    assert_eq!(third.services[0].pid, pid);
    Ok(())
}

// --- 5. diagnose: service + logs + source candidates ------------------------

#[tokio::test(flavor = "multi_thread")]
async fn diagnose_matches_backend_logs_and_source_candidates() -> Result<()> {
    if !node_available() {
        eprintln!("skipping: node is not available");
        return Ok(());
    }
    let ody_home = TempDir::new()?;
    let backend = backend_fixture()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    bind_project(&mut mcp, "ws-1", vec![backend.path().to_path_buf()]).await?;
    define_spec(
        &mut mcp,
        "backend",
        0,
        "dev",
        None,
        vec![],
        Some("/api/health"),
        "spec-d",
    )
    .await?;
    let started = start_all(&mut mcp, vec![], "orch-d").await?;
    let service = started.services.first().expect("service");

    // Contract failure: the route file does not exist yet -> 404 + log line.
    let miss_url = format!("http://127.0.0.1:{}/api/miss", service.port);
    let miss = check(&mut mcp, &service.id, Some(miss_url.clone())).await?;
    assert_eq!(miss.http_status, Some(404));

    let request_id = mcp
        .send_workspace_preview_diagnose_request(WorkspacePreviewDiagnoseParams {
            project_id: "ws-1".to_owned(),
            failures: vec![WorkspaceObservedNetworkFailure {
                url: miss_url.clone(),
                method: Some("GET".to_owned()),
                status: Some(404),
                error: None,
                occurred_at_ms: None,
            }],
            log_tail_bytes: None,
            max_candidates: None,
        })
        .await?;
    let response: WorkspacePreviewDiagnoseResponse = read_response(&mut mcp, request_id).await?;
    let diagnosis: &WorkspaceNetworkDiagnosis = response.results.first().expect("one result");
    let matched = diagnosis.matched_service.as_ref().expect("matched backend");
    assert_eq!(matched.name, "backend");
    assert_eq!(matched.port, service.port);
    assert!(diagnosis.health.as_ref().expect("health").ok);
    let stdout = diagnosis.stdout_tail.as_ref().expect("stdout tail");
    let excerpt = diagnosis.log_excerpt.as_ref().expect("log excerpt");
    assert!(stdout.contains("/api/miss"), "stdout tail: {stdout}");
    assert!(excerpt.contains("/api/miss"), "excerpt: {excerpt}");
    // Source candidates: Name match on the last path segment resolves the
    // indexed items route file (candidates are WorkspaceSourceRef entries
    // with file paths from the backend root).
    assert!(
        !diagnosis.source_candidates.is_empty(),
        "expected source candidates, notes: {:?}",
        diagnosis.notes
    );

    // Non-loopback URL: no match, explanatory note, no panic.
    let request_id = mcp
        .send_workspace_preview_diagnose_request(WorkspacePreviewDiagnoseParams {
            project_id: "ws-1".to_owned(),
            failures: vec![WorkspaceObservedNetworkFailure {
                url: "https://example.com/api/items".to_owned(),
                method: None,
                status: Some(403),
                error: None,
                occurred_at_ms: None,
            }],
            log_tail_bytes: None,
            max_candidates: None,
        })
        .await?;
    let response: WorkspacePreviewDiagnoseResponse = read_response(&mut mcp, request_id).await?;
    let diagnosis = response.results.first().expect("one result");
    assert!(diagnosis.matched_service.is_none());
    assert!(
        diagnosis
            .notes
            .iter()
            .any(|note| note.contains("not a loopback")),
        "notes: {:?}",
        diagnosis.notes
    );
    Ok(())
}

// --- 6. exit criterion: fix a frontend/backend contract mismatch -----------

#[tokio::test(flavor = "multi_thread")]
async fn contract_mismatch_fix_loop_diagnose_changeset_verify() -> Result<()> {
    if !node_available() {
        eprintln!("skipping: node is not available");
        return Ok(());
    }
    let ody_home = TempDir::new()?;
    let backend = backend_fixture()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    bind_project(&mut mcp, "ws-1", vec![backend.path().to_path_buf()]).await?;
    define_spec(
        &mut mcp,
        "backend",
        0,
        "dev",
        None,
        vec![],
        Some("/api/health"),
        "spec-f",
    )
    .await?;
    let started = start_all(&mut mcp, vec![], "orch-f").await?;
    let service = started.services.first().expect("service");

    // Mismatch: frontend calls /api/miss; backend 404s.
    let miss_url = format!("http://127.0.0.1:{}/api/miss", service.port);
    assert_eq!(
        check(&mut mcp, &service.id, Some(miss_url.clone()))
            .await?
            .http_status,
        Some(404)
    );

    // Diagnose points at the backend and its log evidence.
    let request_id = mcp
        .send_workspace_preview_diagnose_request(WorkspacePreviewDiagnoseParams {
            project_id: "ws-1".to_owned(),
            failures: vec![WorkspaceObservedNetworkFailure {
                url: miss_url.clone(),
                method: Some("GET".to_owned()),
                status: Some(404),
                error: None,
                occurred_at_ms: None,
            }],
            log_tail_bytes: None,
            max_candidates: None,
        })
        .await?;
    let diagnosis: WorkspacePreviewDiagnoseResponse = read_response(&mut mcp, request_id).await?;
    assert_eq!(
        diagnosis.results[0]
            .matched_service
            .as_ref()
            .map(|service| service.name.as_str()),
        Some("backend")
    );

    // Fix: changeset adds the missing route file (dynamic server picks it
    // up per request — no restart needed for this fixture).
    let request_id = mcp
        .send_workspace_source_changeset_create_request(WorkspaceChangeSetCreateParams {
            project_id: "ws-1".to_owned(),
            title: "add /api/miss route".to_owned(),
            changes: vec![WorkspaceFileChange {
                root_index: 0,
                path: "src/routes/miss.js".to_owned(),
                kind: WorkspaceFileChangeKind::Add,
                base_hash: None,
                content: Some("// miss route\nexport const miss = true;\n".to_owned()),
            }],
            idempotency_key: "fix-miss".to_owned(),
        })
        .await?;
    let create: ody_app_server_protocol::WorkspaceChangeSetCreateResponse =
        read_response(&mut mcp, request_id).await?;
    let request_id = mcp
        .send_workspace_source_changeset_apply_request(WorkspaceChangeSetApplyParams {
            changeset_id: create.changeset.id,
        })
        .await?;
    let _: ody_app_server_protocol::WorkspaceChangeSetApplyResponse =
        read_response(&mut mcp, request_id).await?;
    assert!(
        backend.path().join("src/routes/miss.js").is_file(),
        "change must land in the user's directory"
    );

    // Verify: the same URL now answers 200 through the managed service.
    let fixed = check(&mut mcp, &service.id, Some(miss_url.clone())).await?;
    assert_eq!(
        fixed.http_status,
        Some(200),
        "contract fix must verify: {fixed:?}"
    );
    Ok(())
}

// --- 7. exit criterion: new backend endpoint + new page, cross-root validate

#[tokio::test(flavor = "multi_thread")]
async fn new_endpoint_new_page_cross_root_validation() -> Result<()> {
    if !node_available() {
        eprintln!("skipping: node is not available");
        return Ok(());
    }
    let ody_home = TempDir::new()?;
    let frontend = frontend_fixture()?;
    let backend = backend_fixture()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    bind_dual_root(&mut mcp, &frontend, &backend).await?;
    define_default_specs(&mut mcp).await?;
    let started = start_all(&mut mcp, vec![], "orch-n").await?;
    let web = started
        .services
        .iter()
        .find(|s| s.name == "web")
        .expect("web")
        .clone();

    // Change: new backend endpoint file (root 1) + new page (root 0).
    let request_id = mcp
        .send_workspace_source_changeset_create_request(WorkspaceChangeSetCreateParams {
            project_id: "ws-1".to_owned(),
            title: "add newendpoint api and new page".to_owned(),
            changes: vec![
                WorkspaceFileChange {
                    root_index: 1,
                    path: "src/routes/newendpoint.js".to_owned(),
                    kind: WorkspaceFileChangeKind::Add,
                    base_hash: None,
                    content: Some("// newendpoint\nexport const newendpoint = true;\n".to_owned()),
                },
                WorkspaceFileChange {
                    root_index: 0,
                    path: "src/pages/new.js".to_owned(),
                    kind: WorkspaceFileChangeKind::Add,
                    base_hash: None,
                    content: Some("// new page\nexport const page = true;\n".to_owned()),
                },
            ],
            idempotency_key: "new-endpoint".to_owned(),
        })
        .await?;
    let create: ody_app_server_protocol::WorkspaceChangeSetCreateResponse =
        read_response(&mut mcp, request_id).await?;
    let request_id = mcp
        .send_workspace_source_changeset_apply_request(WorkspaceChangeSetApplyParams {
            changeset_id: create.changeset.id.clone(),
        })
        .await?;
    let _: ody_app_server_protocol::WorkspaceChangeSetApplyResponse =
        read_response(&mut mcp, request_id).await?;

    // Verify through the running services: new endpoint 200 via the
    // frontend proxy (end-to-end), new page renders.
    let proxied = check(
        &mut mcp,
        &web.id,
        Some(format!("http://127.0.0.1:{}/api/newendpoint", web.port)),
    )
    .await?;
    assert_eq!(proxied.http_status, Some(200), "{proxied:?}");
    let page = check(
        &mut mcp,
        &web.id,
        Some(format!("http://127.0.0.1:{}/new", web.port)),
    )
    .await?;
    assert_eq!(page.http_status, Some(200));

    // Cross-root validation: frontend build (root 0) + backend test (root 1)
    // in one request; each run echoes its root.
    let request_id = mcp
        .send_workspace_source_validate_request(WorkspaceSourceValidateParams {
            project_id: "ws-1".to_owned(),
            changeset_id: Some(create.changeset.id),
            checks: vec![
                WorkspaceValidationCheck {
                    kind: WorkspaceValidationKind::Build,
                    script: "build".to_owned(),
                    root_index: Some(0),
                },
                WorkspaceValidationCheck {
                    kind: WorkspaceValidationKind::Test,
                    script: "test".to_owned(),
                    root_index: Some(1),
                },
            ],
            timeout_ms: None,
        })
        .await?;
    let validated: WorkspaceSourceValidateResponse = read_response(&mut mcp, request_id).await?;
    assert_eq!(validated.report.runs.len(), 2);
    assert_eq!(validated.report.runs[0].root_index, Some(0));
    assert_eq!(validated.report.runs[1].root_index, Some(1));
    assert!(
        validated
            .report
            .runs
            .iter()
            .all(|run| run.status == ody_app_server_protocol::WorkspaceValidationStatus::Succeeded),
        "all cross-root checks must pass: {:?}",
        validated.report
    );
    Ok(())
}

// --- 8. cross-root diff -----------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn cross_root_diff_reports_per_root_git_diff() -> Result<()> {
    let ody_home = TempDir::new()?;
    let frontend = frontend_fixture()?;
    let backend = backend_fixture()?;
    // Both roots as real git repos with a baseline commit.
    for root in [frontend.path(), backend.path()] {
        for args in [
            vec!["init"],
            vec!["add", "."],
            vec![
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-m",
                "baseline",
            ],
        ] {
            let status = std::process::Command::new("git")
                .args(args)
                .current_dir(root)
                .output()?
                .status;
            assert!(status.success(), "git setup failed in {}", root.display());
        }
    }
    fs::write(frontend.path().join("note.txt"), "frontend change\n")?;
    fs::write(backend.path().join("note.txt"), "backend change\n")?;
    for root in [frontend.path(), backend.path()] {
        let status = std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(root)
            .output()?
            .status;
        assert!(status.success(), "git add failed in {}", root.display());
    }

    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    bind_dual_root(&mut mcp, &frontend, &backend).await?;

    let request_id = mcp
        .send_workspace_source_diff_request(WorkspaceSourceDiffParams {
            project_id: "ws-1".to_owned(),
        })
        .await?;
    let diff: WorkspaceSourceDiffResponse = read_response(&mut mcp, request_id).await?;
    assert_eq!(diff.root_git_diffs.len(), 2, "{diff:?}");
    assert_eq!(diff.root_git_diffs[0].root_index, 0);
    assert_eq!(diff.root_git_diffs[1].root_index, 1);
    assert!(diff.root_git_diffs[0].git.available);
    assert!(diff.root_git_diffs[1].git.available);
    let front_diff = diff.root_git_diffs[0]
        .git
        .unified_diff
        .as_ref()
        .expect("front diff");
    let back_diff = diff.root_git_diffs[1]
        .git
        .unified_diff
        .as_ref()
        .expect("back diff");
    assert!(front_diff.contains("frontend change"), "{front_diff}");
    assert!(back_diff.contains("backend change"), "{back_diff}");
    // Back-compat: legacy primary-root field still carries root 0's diff.
    let legacy = diff.git_diff.expect("legacy git_diff");
    assert!(legacy.available);
    assert_eq!(legacy.unified_diff.as_deref(), Some(front_diff.as_str()));
    Ok(())
}

// --- 9. monorepo (single root + spec.cwd) -----------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn monorepo_specs_with_cwd_start_in_subdirs() -> Result<()> {
    if !node_available() {
        eprintln!("skipping: node is not available");
        return Ok(());
    }
    let ody_home = TempDir::new()?;
    let monorepo = monorepo_fixture()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    bind_project(&mut mcp, "ws-1", vec![monorepo.path().to_path_buf()]).await?;
    define_spec(
        &mut mcp,
        "backend",
        0,
        "dev",
        Some("backend"),
        vec![],
        Some("/api/health"),
        "m-backend",
    )
    .await?;
    define_spec(
        &mut mcp,
        "web",
        0,
        "dev",
        Some("frontend"),
        vec!["backend"],
        None,
        "m-web",
    )
    .await?;

    let response = start_all(&mut mcp, vec![], "orch-m").await?;
    assert!(response.failed.is_empty(), "{response:?}");
    assert_eq!(response.started.len(), 2);
    let web = response
        .services
        .iter()
        .find(|s| s.name == "web")
        .expect("web");
    let backend_ref = response
        .services
        .iter()
        .find(|s| s.name == "backend")
        .expect("backend");
    assert_eq!(backend_ref.status, WorkspaceServiceStatus::Ready);
    assert_eq!(web.status, WorkspaceServiceStatus::Ready);
    // Injection works identically from a monorepo sub-package.
    let proxied = check(
        &mut mcp,
        &web.id,
        Some(format!("http://127.0.0.1:{}/api/items", web.port)),
    )
    .await?;
    assert_eq!(proxied.http_status, Some(200), "{proxied:?}");
    Ok(())
}

// --- 10. cleanup: stopAll order, port release, close purge ------------------

#[tokio::test(flavor = "multi_thread")]
async fn stop_all_and_close_release_ports_and_purge_specs() -> Result<()> {
    if !node_available() {
        eprintln!("skipping: node is not available");
        return Ok(());
    }
    let ody_home = TempDir::new()?;
    let frontend = frontend_fixture()?;
    let backend = backend_fixture()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    bind_dual_root(&mut mcp, &frontend, &backend).await?;
    define_default_specs(&mut mcp).await?;
    let started = start_all(&mut mcp, vec![], "orch-c").await?;
    let backend_port = started
        .services
        .iter()
        .find(|s| s.name == "backend")
        .expect("backend")
        .port;

    // stopAll: reverse-order stop, everything terminal, port rebindable.
    let request_id = mcp
        .send_workspace_service_stop_all_request(WorkspaceServiceStopAllParams {
            project_id: "ws-1".to_owned(),
            names: vec![],
        })
        .await?;
    let stopped: WorkspaceServiceStopAllResponse = read_response(&mut mcp, request_id).await?;
    assert_eq!(stopped.stopped.len(), 2);
    assert!(
        stopped
            .stopped
            .iter()
            .all(|service| service.status == WorkspaceServiceStatus::Stopped),
        "{stopped:?}"
    );
    TcpListener::bind(("127.0.0.1", backend_port)).map_err(|err| {
        anyhow::anyhow!("backend port {backend_port} must be free after stopAll: {err}")
    })?;

    // Specs survive stopAll (declarative state); close purges them.
    let request_id = mcp
        .send_workspace_service_specs_request(WorkspaceServiceSpecsParams {
            project_id: "ws-1".to_owned(),
        })
        .await?;
    let specs: WorkspaceServiceSpecsResponse = read_response(&mut mcp, request_id).await?;
    assert_eq!(specs.specs.len(), 2);

    let request_id = mcp
        .send_workspace_project_close_request(WorkspaceProjectCloseParams {
            project_id: "ws-1".to_owned(),
        })
        .await?;
    let _: serde::de::IgnoredAny = read_response(&mut mcp, request_id).await?;

    let request_id = mcp
        .send_workspace_service_specs_request(WorkspaceServiceSpecsParams {
            project_id: "ws-1".to_owned(),
        })
        .await?;
    let message = read_error(&mut mcp, request_id).await?;
    assert!(message.contains("unknown project id"), "{message}");
    Ok(())
}
