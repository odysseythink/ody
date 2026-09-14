//! E4 T08 manual smoke: the §9.2 exit criteria against a REAL vite (react-ts)
//! + express sample, driven through the app-server protocol exactly as a
//! renderer client would, with headless Chrome producing the evidence.
//!
//! Steps (evidence under `.ody-code/spikes/e4-reliability-smoke/`):
//!   1. watch + external IDE edit -> workspace/changed (+ invalidation)
//!   2. dual-connection write lock over a second Runtime (websocket transport)
//!   3. kill -9 backend -> service/changed failed + diagnose -> re-orchestrate
//!   4. SIGKILL the Runtime itself -> restart normalization + audit trail
//!   5. cleanup: ports free, close purges specs, audit human-readable
//!
//! Run manually (requires npm registry access and local Chrome):
//!   cargo test -p ody-app-server --test all workspace_reliability_smoke -- --ignored --nocapture

use std::fs;
use std::net::TcpListener;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use app_test_support::TestAppServer;
use app_test_support::next_test_port;
use app_test_support::to_response;
use ody_app_server_protocol::ClientInfo;
use ody_app_server_protocol::InitializeCapabilities;
use ody_app_server_protocol::InitializeParams;
use ody_app_server_protocol::JSONRPCMessage;
use ody_app_server_protocol::RequestId;
use ody_app_server_protocol::WorkspaceAuditListParams;
use ody_app_server_protocol::WorkspaceAuditListResponse;
use ody_app_server_protocol::WorkspaceChangeSetApplyParams;
use ody_app_server_protocol::WorkspaceChangeSetCreateParams;
use ody_app_server_protocol::WorkspaceChangeSetCreateResponse;
use ody_app_server_protocol::WorkspaceChangeSet;
use ody_app_server_protocol::WorkspaceFileChange;
use ody_app_server_protocol::WorkspaceFileChangeKind;
use ody_app_server_protocol::WorkspaceObservedNetworkFailure;
use ody_app_server_protocol::WorkspacePreviewCheckParams;
use ody_app_server_protocol::WorkspacePreviewCheckResponse;
use ody_app_server_protocol::WorkspacePreviewDiagnoseParams;
use ody_app_server_protocol::WorkspacePreviewDiagnoseResponse;
use ody_app_server_protocol::WorkspaceProjectBindParams;
use ody_app_server_protocol::WorkspaceProjectCloseParams;
use ody_app_server_protocol::WorkspaceServiceDefineParams;
use ody_app_server_protocol::WorkspaceServiceHealthCheck;
use ody_app_server_protocol::WorkspaceServiceSpecsParams;
use ody_app_server_protocol::WorkspaceServiceStartAllParams;
use ody_app_server_protocol::WorkspaceServiceStartAllResponse;
use ody_app_server_protocol::WorkspaceServiceStatus;
use ody_app_server_protocol::WorkspaceServiceStopAllParams;
use ody_app_server_protocol::WorkspaceServiceStopAllResponse;
use ody_app_server_protocol::WorkspaceWatchParams;
use ody_utils_absolute_path::test_support::PathBufExt;
use sha2::Digest;
use sha2::Sha256;
use tempfile::TempDir;
use tokio::time::timeout;

use super::connection_handling_websocket::DEFAULT_READ_TIMEOUT;
use super::connection_handling_websocket::WsClient;
use super::connection_handling_websocket::connect_websocket;
use super::connection_handling_websocket::read_error_for_id;
use super::connection_handling_websocket::read_response_for_id;
use super::connection_handling_websocket::send_request;
use super::connection_handling_websocket::spawn_websocket_server;

const PROTOCOL_TIMEOUT: Duration = Duration::from_secs(240); // startAll waits vite ready
const NPM_TIMEOUT: Duration = Duration::from_secs(300);
const CHROME: &str = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const PROJECT: &str = "p1";

fn evidence_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../.ody-code/spikes/e4-reliability-smoke")
}

fn sha256_hex(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn npm_ok() -> bool {
    Command::new("npm")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn chrome_ok() -> bool {
    Path::new(CHROME).is_file()
}

async fn run_npm(args: &[&str], cwd: &Path) -> Result<()> {
    let output = timeout(NPM_TIMEOUT, async {
        tokio::process::Command::new("npm")
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .output()
            .await
    })
    .await
    .context("npm timed out")??;
    anyhow::ensure!(
        output.status.success(),
        "npm {args:?} failed in {}:\nstdout: {}\nstderr: {}",
        cwd.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

// --- real fixtures (same construction as the E3 fullstack smoke) -------------

async fn backend_fixture(dir: &Path) -> Result<()> {
    fs::write(
        dir.join("package.json"),
        serde_json::json!({
            "name": "smoke-backend",
            "scripts": { "dev": "node server.js" }
        })
        .to_string(),
    )?;
    fs::write(
        dir.join("server.js"),
        r#"const express = require('express');
const app = express();
app.use((req, res, next) => {
    res.setHeader('Access-Control-Allow-Origin', '*');
    res.setHeader('Access-Control-Allow-Methods', 'GET, OPTIONS');
    res.setHeader('Access-Control-Allow-Headers', '*');
    res.setHeader('Access-Control-Allow-Private-Network', 'true');
    if (req.method === 'OPTIONS') return res.sendStatus(204);
    next();
});
app.get('/', (req, res) => res.json({ ok: true, service: 'backend' }));
app.get('/api/health', (req, res) => res.json({ ok: true }));
app.get('/api/items', (req, res) => res.json([{ id: 1, name: 'Item Alpha' }]));
const port = Number(process.env.PORT || 8787);
app.listen(port, '127.0.0.1', () => console.log(`backend listening on ${port}`));
"#,
    )?;
    run_npm(&["install", "express"], dir).await?;
    Ok(())
}

async fn frontend_fixture(parent: &Path) -> Result<()> {
    run_npm(
        &["create", "vite@latest", "frontend", "--", "--template", "react-ts"],
        parent,
    )
    .await?;
    let dir = parent.join("frontend");
    run_npm(&["install"], &dir).await?;
    fs::write(
        dir.join("vite.config.ts"),
        r#"import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// The dev proxy target AND the page global both consume the injected
// BACKEND_URL (managed-service dependency env channel).
export default defineConfig({
  plugins: [react()],
  server: {
    host: '127.0.0.1',
    proxy: {
      '/api': {
        target: process.env.BACKEND_URL || 'http://127.0.0.1:8787',
        changeOrigin: true,
      },
    },
  },
  define: {
    __BACKEND_URL__: JSON.stringify(process.env.BACKEND_URL || ''),
  },
});
"#,
    )?;
    fs::write(
        dir.join("index.html"),
        r#"<!doctype html>
<html>
  <head><title>E4 Reliability Smoke</title></head>
  <body>
    <div id="root"></div>
    <script type="module" src="/src/main.tsx"></script>
  </body>
</html>
"#,
    )?;
    fs::write(
        dir.join("src/main.tsx"),
        r#"import React from 'react';
import { createRoot } from 'react-dom/client';

function App() {
  const [items, setItems] = React.useState<string>('loading');
  React.useEffect(() => {
    fetch(`${(window as any).__BACKEND_URL__ || ''}/api/items`)
      .then((r) => (r.ok ? r.json() : Promise.reject(new Error(`direct ${r.status}`))))
      .then((data) => setItems(`direct-item:${data[0].name}`))
      .catch((err) => setItems(`direct 404: ${err.message}`));
  }, []);
  return (
    <main>
      <h1>E4 Reliability Smoke</h1>
      <p data-testid="backend">{items}</p>
    </main>
  );
}

createRoot(document.getElementById('root')!).render(<App />);
"#,
    )?;
    Ok(())
}

// --- chrome evidence ----------------------------------------------------------

fn chrome_spawn(profile: &Path, extra: &[String], url: &str) -> Result<std::process::Child> {
    Ok(Command::new(CHROME)
        .args([
            "--headless=new",
            "--disable-gpu",
            "--no-first-run",
            "--disable-extensions",
            "--disable-dev-shm-usage",
            &format!("--user-data-dir={}", profile.display()),
            "--virtual-time-budget=20000",
        ])
        .args(extra)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("launch headless Chrome")?)
}

fn chrome_shot(profile: &Path, url: &str, png: &Path) -> Result<()> {
    let flag = format!("--screenshot={}", png.display());
    let mut child = chrome_spawn(profile, &[flag, "--window-size=1280,800".to_owned()], url)?;
    let deadline = std::time::Instant::now() + Duration::from_secs(45);
    loop {
        if png.is_file() && png.metadata()?.len() > 0 {
            break;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            anyhow::bail!("screenshot timed out: {}", png.display());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let _ = child.kill();
    let _ = child.wait();
    Ok(())
}

/// Render a JSON payload as an HTML page and screenshot it as evidence.
fn shot_json(work: &Path, name: &str, title: &str, payload: &serde_json::Value) -> Result<()> {
    let html = work.join(format!("{name}.html"));
    fs::write(
        &html,
        format!(
            "<!doctype html><html><head><meta charset='utf-8'><title>{title}</title></head>\
             <body><h1>{title}</h1><pre>{}</pre></body></html>",
            serde_json::to_string_pretty(payload)?
        ),
    )?;
    chrome_shot(
        &work.join(format!("chrome-{name}")),
        &format!("file://{}", html.display()),
        &evidence_dir().join(format!("{name}.png")),
    )
}

// --- protocol helpers ----------------------------------------------------------

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

async fn read_response<T: serde::de::DeserializeOwned>(
    mcp: &mut TestAppServer,
    request_id: i64,
) -> Result<T> {
    let message = timeout(
        PROTOCOL_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    Ok(to_response::<T>(message)?)
}

async fn bind(mcp: &mut TestAppServer, roots: Vec<PathBuf>) -> Result<()> {
    let request_id = mcp
        .send_workspace_project_bind_request(WorkspaceProjectBindParams {
            id: PROJECT.to_owned(),
            name: "e4 reliability smoke".to_owned(),
            roots: roots.into_iter().map(|path| path.abs()).collect(),
            idempotency_key: "smoke-bind".to_owned(),
        })
        .await?;
    read_response::<serde::de::IgnoredAny>(mcp, request_id).await?;
    Ok(())
}

async fn watch(mcp: &mut TestAppServer) -> Result<()> {
    let request_id = mcp
        .send_workspace_watch_request(WorkspaceWatchParams {
            project_id: PROJECT.to_owned(),
        })
        .await?;
    read_response::<serde::de::IgnoredAny>(mcp, request_id).await?;
    Ok(())
}

async fn create_update_changeset(
    mcp: &mut TestAppServer,
    title: &str,
    path: &str,
    base: &str,
    content: &str,
    idem: &str,
) -> Result<WorkspaceChangeSet> {
    let request_id = mcp
        .send_workspace_source_changeset_create_request(WorkspaceChangeSetCreateParams {
            project_id: PROJECT.to_owned(),
            title: title.to_owned(),
            changes: vec![WorkspaceFileChange {
                root_index: 0,
                path: path.to_owned(),
                kind: WorkspaceFileChangeKind::Update,
                base_hash: Some(sha256_hex(base)),
                content: Some(content.to_owned()),
            }],
            idempotency_key: idem.to_owned(),
        })
        .await?;
    Ok(read_response::<WorkspaceChangeSetCreateResponse>(mcp, request_id)
        .await?
        .changeset)
}

async fn apply_changeset(mcp: &mut TestAppServer, changeset_id: &str) -> Result<()> {
    let request_id = mcp
        .send_workspace_source_changeset_apply_request(WorkspaceChangeSetApplyParams {
            changeset_id: changeset_id.to_owned(),
        })
        .await?;
    read_response::<serde::de::IgnoredAny>(mcp, request_id).await?;
    Ok(())
}

async fn define(
    mcp: &mut TestAppServer,
    name: &str,
    root_index: u32,
    script: &str,
    depends_on: Vec<&str>,
    health_path: Option<&str>,
    port: u16,
    idem: &str,
) -> Result<()> {
    let request_id = mcp
        .send_workspace_service_define_request(WorkspaceServiceDefineParams {
            project_id: PROJECT.to_owned(),
            name: name.to_owned(),
            root_index,
            script: script.to_owned(),
            port: Some(port),
            cwd: None,
            depends_on: depends_on.iter().map(|s| s.to_string()).collect(),
            health_check: health_path.map(|path| WorkspaceServiceHealthCheck {
                path: path.to_owned(),
                expect_status: Some(200),
                timeout_ms: None,
            }),
            ready_timeout_ms: Some(120_000),
            env_refs: None,
            idempotency_key: idem.to_owned(),
        })
        .await?;
    read_response::<serde::de::IgnoredAny>(mcp, request_id).await?;
    Ok(())
}

async fn start_all(mcp: &mut TestAppServer, idem: &str) -> Result<WorkspaceServiceStartAllResponse> {
    let request_id = mcp
        .send_workspace_service_start_all_request(WorkspaceServiceStartAllParams {
            project_id: PROJECT.to_owned(),
            names: Vec::new(),
            secret_values: None,
            idempotency_key: idem.to_owned(),
        })
        .await?;
    read_response::<WorkspaceServiceStartAllResponse>(mcp, request_id).await
}

async fn stop_all(mcp: &mut TestAppServer) -> Result<WorkspaceServiceStopAllResponse> {
    let request_id = mcp
        .send_workspace_service_stop_all_request(WorkspaceServiceStopAllParams {
            project_id: PROJECT.to_owned(),
            names: Vec::new(),
        })
        .await?;
    read_response::<WorkspaceServiceStopAllResponse>(mcp, request_id).await
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
    read_response::<WorkspacePreviewCheckResponse>(mcp, request_id).await
}

async fn diagnose(
    mcp: &mut TestAppServer,
    failure_url: String,
) -> Result<WorkspacePreviewDiagnoseResponse> {
    let request_id = mcp
        .send_workspace_preview_diagnose_request(WorkspacePreviewDiagnoseParams {
            project_id: PROJECT.to_owned(),
            failures: vec![WorkspaceObservedNetworkFailure {
                url: failure_url,
                method: Some("GET".to_owned()),
                status: None,
                error: Some("net::ERR_CONNECTION_REFUSED".to_owned()),
                occurred_at_ms: None,
            }],
            log_tail_bytes: None,
            max_candidates: None,
        })
        .await?;
    read_response::<WorkspacePreviewDiagnoseResponse>(mcp, request_id).await
}

async fn audit_list(mcp: &mut TestAppServer) -> Result<WorkspaceAuditListResponse> {
    let request_id = mcp
        .send_workspace_audit_list_request(WorkspaceAuditListParams {
            project_id: Some(PROJECT.to_owned()),
            limit: Some(50),
        })
        .await?;
    read_response::<WorkspaceAuditListResponse>(mcp, request_id).await
}

fn port_free(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_ok()
}

/// Port release trails process death by a moment (socket teardown is
/// asynchronous); poll instead of asserting on the first instant.
fn port_free_wait(port: u16) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        if port_free(port) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

// --- websocket (second Runtime) helpers for the lock step ---------------------

async fn ws_init(ws: &mut WsClient, id: i64, client_name: &str) -> Result<()> {
    let params = InitializeParams {
        client_info: ClientInfo {
            name: client_name.to_string(),
            title: Some("E4 smoke window".to_string()),
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

async fn ws_ok(ws: &mut WsClient, id: i64) -> Result<serde_json::Value> {
    let response = timeout(DEFAULT_READ_TIMEOUT, read_response_for_id(ws, id)).await??;
    Ok(response.result)
}

// --- the smoke -----------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
#[ignore = "manual E4 smoke: requires npm registry access and local Chrome; run with -- --ignored --nocapture"]
async fn smoke_e4_exit_criteria() -> Result<()> {
    if !npm_ok() {
        eprintln!("smoke skipped: npm is not available");
        return Ok(());
    }
    if !chrome_ok() {
        eprintln!("smoke skipped: Chrome is not available at {CHROME}");
        return Ok(());
    }
    let evidence = evidence_dir();
    fs::create_dir_all(&evidence)?;
    let scratch = TempDir::new()?;
    let work = scratch.path();

    println!("[smoke] setup: scaffold real sample (vite react-ts + express)");
    let backend_dir = work.join("backend");
    fs::create_dir_all(&backend_dir)?;
    backend_fixture(&backend_dir).await?;
    frontend_fixture(work).await?;
    let frontend_dir = work.join("frontend");
    let backend_port = next_test_port();
    let web_port = next_test_port();

    let ody_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    bind(&mut mcp, vec![frontend_dir.clone(), backend_dir.clone()]).await?;
    watch(&mut mcp).await?;

    println!("[smoke] step 1: external IDE edit is observed, never overwritten");
    let app_ts = frontend_dir.join("src/App.tsx");
    let app_original = fs::read_to_string(&app_ts)?;
    let cs = create_update_changeset(
        &mut mcp,
        "agent edit App.tsx",
        "src/App.tsx",
        &app_original,
        "ide edit via changeset\n",
        "smoke-cs-1",
    )
    .await?;
    // The IDE saves underneath the Runtime (a raw fs write bypasses Ody).
    let ide_edit = format!("// IDE edit at {}\n{}", chrono_now_ms(), app_original);
    fs::write(&app_ts, &ide_edit)?;
    let changed = timeout(
        Duration::from_secs(20),
        mcp.read_stream_until_matching_notification("workspace/changed invalidated", |n| {
            n.method == "workspace/changed"
                && n.params.as_ref().is_some_and(|p| {
                    p["invalidatedChangesets"]
                        .as_array()
                        .is_some_and(|ids| ids.iter().any(|id| id == &cs.id))
                })
        }),
    )
    .await??;
    println!(
        "[smoke]   workspace/changed received; invalidatedChangesets={:?}",
        changed.params.as_ref().and_then(|p| p["invalidatedChangesets"].as_array())
    );
    shot_json(work, "step1-watch-notification", "Step 1: workspace/changed after IDE edit", &serde_json::json!({
        "notification": changed,
        "diskBytesAfterIdeEdit": ide_edit.len(),
    }))?;
    // The IDE bytes survive; the invalidated changeset cannot apply; re-base
    // and apply so no work is lost.
    anyhow::ensure!(fs::read_to_string(&app_ts)? == ide_edit, "IDE bytes must be untouched");
    let cs2 = create_update_changeset(
        &mut mcp,
        "rebase on IDE content",
        "src/App.tsx",
        &ide_edit,
        "agent edit after IDE\n",
        "smoke-cs-2",
    )
    .await?;
    apply_changeset(&mut mcp, &cs2.id).await?;
    println!("[smoke]   rebased changeset applied after IDE edit");

    println!("[smoke] step 2: dual-connection write lock (second Runtime, websocket)");
    // A second Runtime on the same ody_home; two windows as two websocket
    // connections. (stdio TestAppServer holds one connection per process, so
    // the second connection uses the websocket transport — same process, two
    // connections, which is exactly the multi-window topology.)
    let (mut ws_process, bind_addr) = spawn_websocket_server(ody_home.path()).await?;
    let mut window1 = connect_websocket(bind_addr).await?;
    let mut window2 = connect_websocket(bind_addr).await?;
    ws_init(&mut window1, 1, "e4_smoke_window_one").await?;
    ws_init(&mut window2, 1, "e4_smoke_window_two").await?;
    ws_ok(&mut window1, 1).await?;
    ws_ok(&mut window2, 1).await?;
    let disk = fs::read_to_string(&app_ts)?;
    send_request(
        &mut window1,
        "workspace/source/changeset/create",
        2,
        Some(serde_json::json!({
            "projectId": PROJECT,
            "title": "lock-step changeset",
            "changes": [{
                "rootIndex": 0,
                "path": "src/App.tsx",
                "kind": "update",
                "baseHash": sha256_hex(&disk),
                "content": "lock step\n",
            }],
            "idempotencyKey": "smoke-cs-lock",
        })),
    )
    .await?;
    let created = ws_ok(&mut window1, 2).await?;
    let lock_cs_id = created["changeset"]["id"].as_str().expect("changeset id").to_owned();
    send_request(
        &mut window2,
        "workspace/project/lock",
        2,
        Some(serde_json::json!({ "projectId": PROJECT })),
    )
    .await?;
    ws_ok(&mut window2, 2).await?;
    send_request(
        &mut window1,
        "workspace/source/changeset/apply",
        3,
        Some(serde_json::json!({ "changesetId": lock_cs_id })),
    )
    .await?;
    let rejection = timeout(DEFAULT_READ_TIMEOUT, read_error_for_id(&mut window1, 3)).await??;
    println!("[smoke]   foreign apply rejected: {}", rejection.error.message);
    anyhow::ensure!(
        rejection.error.message.contains("locked by another session"),
        "lock rejection expected, got {:?}",
        rejection.error.message
    );
    anyhow::ensure!(fs::read_to_string(&app_ts)? == disk, "rejected apply must not write");
    shot_json(work, "step2-lock-rejection", "Step 2: apply rejected across windows", &serde_json::json!({
        "error": rejection.error,
    }))?;
    send_request(
        &mut window2,
        "workspace/project/unlock",
        3,
        Some(serde_json::json!({ "projectId": PROJECT })),
    )
    .await?;
    ws_ok(&mut window2, 3).await?;
    drop(window1);
    drop(window2);
    ws_process.kill().await.context("stop ws runtime")?;
    println!("[smoke]   lock released; second Runtime stopped");

    println!("[smoke] step 3: kill -9 backend -> notify + diagnose -> re-orchestrate");
    define(
        &mut mcp,
        "backend",
        1,
        "dev",
        vec![],
        Some("/api/health"),
        backend_port,
        "smoke-spec-backend",
    )
    .await?;
    define(
        &mut mcp,
        "web",
        0,
        "dev",
        vec!["backend"],
        None,
        web_port,
        "smoke-spec-web",
    )
    .await?;
    let started = start_all(&mut mcp, "smoke-orch-1").await?;
    anyhow::ensure!(started.failed.is_empty(), "startAll failed: {started:?}");
    let backend_ref = started
        .services
        .iter()
        .find(|s| s.name == "backend")
        .expect("backend")
        .clone();
    let web_ref = started
        .services
        .iter()
        .find(|s| s.name == "web")
        .expect("web")
        .clone();
    anyhow::ensure!(backend_ref.status == WorkspaceServiceStatus::Ready);
    anyhow::ensure!(web_ref.status == WorkspaceServiceStatus::Ready);
    println!(
        "[smoke]   backend ready on {} (pid {:?}), web ready on {}",
        backend_ref.url, backend_ref.pid, web_ref.url
    );

    // Hard-kill the backend process tree (simulates OOM-killer / external
    // kill of the whole service group; the Runtime assigns the service pid
    // as its process group, so a negative pid kills wrapper + grandchildren
    // — killing the wrapper alone would orphan the node grandchild on the
    // port and re-orchestration would fail with a port-in-use error, which
    // the smoke intentionally avoids here).
    let backend_pid = backend_ref.pid.expect("backend pid");
    kill_group(backend_pid);
    let failed = timeout(
        Duration::from_secs(30),
        mcp.read_stream_until_matching_notification("service/changed failed backend", |n| {
            n.method == "workspace/service/changed"
                && n.params.as_ref().is_some_and(|p| {
                    p["reason"] == "failed"
                        && p["services"].as_array().is_some_and(|services| {
                            services.iter().any(|s| s["name"] == "backend")
                        })
                })
        }),
    )
    .await??;
    println!("[smoke]   backend crash notification received");
    shot_json(work, "step3-crash-diagnose", "Step 3: backend crash notification", &serde_json::json!({
        "notification": failed,
    }))?;

    let diagnosis = diagnose(&mut mcp, format!("{}api/items", backend_ref.url)).await?;
    let matched = diagnosis.results[0]
        .matched_service
        .as_ref()
        .expect("diagnose must surface the crashed backend");
    anyhow::ensure!(matched.name == "backend", "diagnose matched {}", matched.name);
    println!("[smoke]   diagnose matched backend (status {:?})", matched.status);

    let stopped = stop_all(&mut mcp).await?;
    println!("[smoke]   stopAll stopped {} services", stopped.stopped.len());
    let recovered = start_all(&mut mcp, "smoke-orch-2").await?;
    anyhow::ensure!(recovered.failed.is_empty(), "recovery failed: {recovered:?}");
    let web_after = recovered
        .services
        .iter()
        .find(|s| s.name == "web")
        .expect("web after recovery")
        .clone();
    let proxied = check(
        &mut mcp,
        &web_after.id,
        Some(format!("{}api/items", web_after.url)),
    )
    .await?;
    anyhow::ensure!(
        proxied.http_status == Some(200),
        "proxy must recover: {proxied:?}"
    );
    println!("[smoke]   re-orchestrated: GET /api/items via proxy -> 200");
    chrome_shot(
        &work.join("chrome-s3"),
        &format!("{}api/items", web_after.url),
        &evidence.join("step3-recovered.png"),
    )?;

    println!("[smoke] step 4: SIGKILL the Runtime -> restart normalization + audit");
    // Grab the surviving service pids first: a SIGKILLed Runtime cannot clean
    // up its process tree, so the smoke must kill the orphans it created.
    let mut orphan_pids = Vec::new();
    for service in &recovered.services {
        if let Some(pid) = service.pid {
            orphan_pids.push(pid);
        }
    }
    mcp.kill_hard()?;
    drop(mcp);

    let mut mcp2 = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp2).await?;
    let restart_notice = timeout(
        Duration::from_secs(30),
        mcp2.read_stream_until_matching_notification("service/changed runtimeRestarted", |n| {
            n.method == "workspace/service/changed"
                && n.params
                    .as_ref()
                    .is_some_and(|p| p["reason"] == "runtimeRestarted")
        }),
    )
    .await??;
    println!("[smoke]   runtimeRestarted announcement received");
    let audit = audit_list(&mut mcp2).await?;
    anyhow::ensure!(
        audit.events.iter().any(|event| event.operation == "project.bind"),
        "audit trail must span the Runtime restart"
    );
    println!("[smoke]   audit trail spans restart ({} events)", audit.events.len());
    shot_json(work, "step4-restart", "Step 4: restart normalization + audit trail", &serde_json::json!({
        "notification": restart_notice,
        "auditEvents": audit.events.iter().map(|event| serde_json::json!({
            "operation": event.operation,
            "outcome": event.outcome,
        })).collect::<Vec<_>>(),
    }))?;
    // The SIGKILLed Runtime orphaned its service process trees; clean them
    // up (process-group kill, as in step 3) so step 5's port assertions are
    // meaningful.
    for pid in orphan_pids {
        kill_group(pid);
    }

    println!("[smoke] step 5: cleanup (ports free, close purges specs, audit readable)");
    anyhow::ensure!(
        port_free_wait(backend_port),
        "backend port {backend_port} still bound"
    );
    anyhow::ensure!(port_free_wait(web_port), "web port {web_port} still bound");
    let request_id = mcp2
        .send_workspace_project_close_request(WorkspaceProjectCloseParams {
            project_id: PROJECT.to_owned(),
        })
        .await?;
    read_response::<serde::de::IgnoredAny>(&mut mcp2, request_id).await?;
    let request_id = mcp2
        .send_workspace_service_specs_request(WorkspaceServiceSpecsParams {
            project_id: PROJECT.to_owned(),
        })
        .await?;
    let message = timeout(
        PROTOCOL_TIMEOUT,
        mcp2.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;
    anyhow::ensure!(
        message.error.message.contains("unknown project"),
        "close must purge specs, got {:?}",
        message.error.message
    );
    let audit_final = audit_list(&mut mcp2).await?;
    shot_json(work, "step5-audit", "Step 5: human-readable audit trail", &serde_json::json!({
        "events": audit_final.events,
    }))?;
    println!("[smoke]   cleanup: ports {backend_port}/{web_port} free, specs purged on close");
    println!("[smoke] DONE — evidence dir: {}", evidence.display());
    Ok(())
}

/// Kill a whole service process group (the Runtime assigns each service
/// pid as its own process group leader).
#[cfg(unix)]
fn kill_group(pid: u32) {
    let _ = Command::new("kill")
        .args(["-9", &format!("-{pid}")])
        .status();
}

#[cfg(not(unix))]
fn kill_group(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .status();
}

fn chrono_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
