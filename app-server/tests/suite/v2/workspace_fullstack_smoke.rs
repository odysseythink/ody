//! E3 manual smoke (ADR 2026-09-13-e3 §7): a REAL fullstack sample —
//! `npm create vite@latest` (React) frontend + Express backend — driven
//! through the app-server protocol exactly as a renderer client would, with
//! Google Chrome (headless) as the observing browser.
//!
//! What distinguishes this from `workspace_fullstack.rs`: real npm packages,
//! a real Vite dev server (HMR pipeline, dependency pre-bundling), a real
//! Express app, and browser-side evidence (DOM dumps + screenshots) instead
//! of in-process HTTP checks alone.
//!
//! Run manually (requires network for npm and a local Chrome):
//!   cargo test -p ody-app-server --test all workspace_fullstack_smoke -- --ignored --nocapture
//!
//! Evidence (screenshots + this test's stdout) is written under
//! `.ody-code/spikes/e3-fullstack-smoke/` next to the ody repo root.

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
use ody_app_server_protocol::JSONRPCMessage;
use ody_app_server_protocol::RequestId;
use ody_app_server_protocol::WorkspaceChangeSetApplyParams;
use ody_app_server_protocol::WorkspaceChangeSetCreateParams;
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
use ody_app_server_protocol::WorkspaceServiceSpec;
use ody_app_server_protocol::WorkspaceServiceSpecsParams;
use ody_app_server_protocol::WorkspaceServiceStartAllParams;
use ody_app_server_protocol::WorkspaceServiceStartAllResponse;
use ody_app_server_protocol::WorkspaceServiceStatus;
use ody_app_server_protocol::WorkspaceServiceStopAllParams;
use ody_app_server_protocol::WorkspaceServiceStopAllResponse;
use ody_app_server_protocol::WorkspaceSourceValidateParams;
use ody_app_server_protocol::WorkspaceSourceValidateResponse;
use ody_app_server_protocol::WorkspaceValidationCheck;
use ody_app_server_protocol::WorkspaceValidationKind;
use ody_utils_absolute_path::test_support::PathBufExt;
use sha2::Digest;
use sha2::Sha256;
use tempfile::TempDir;
use tokio::time::timeout;

const PROTOCOL_TIMEOUT: Duration = Duration::from_secs(240); // startAll waits vite ready
const NPM_TIMEOUT: Duration = Duration::from_secs(300);
const CHROME: &str = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";

fn evidence_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../.ody-code/spikes/e3-fullstack-smoke")
}

fn sha256_hex(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn npm_ok() -> bool {
    Command::new("npm")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
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

// --- real fixtures ----------------------------------------------------------

/// Express backend with dev-mode route reloading: route modules are re-read
/// per request, so a landed changeset takes effect without a restart — the
/// fixture's equivalent of real dev tooling (nodemon / HMR). 404s are logged
/// to stdout, which is the diagnose log-excerpt evidence.
async fn backend_fixture(dir: &Path) -> Result<()> {
    fs::write(
        dir.join("package.json"),
        serde_json::json!({
            "name": "smoke-backend",
            "scripts": {
                "dev": "node server.js",
                "test": "node --test tests/routes.test.js",
                "build": "node scripts/build.js"
            }
        })
        .to_string(),
    )?;
    fs::write(
        dir.join("server.js"),
        r#"const express = require('express');
const fs = require('fs');
const path = require('path');

const app = express();

// Real fullstack dev practice: the browser page calls this origin directly
// (the injected BACKEND_URL), so CORS — including the Private Network Access
// preflight Chrome enforces between [::1] pages and 127.0.0.1 APIs — must
// be answered. Loopback-only scope keeps this safe for a dev fixture.
app.use((req, res, next) => {
    res.setHeader('Access-Control-Allow-Origin', '*');
    res.setHeader('Access-Control-Allow-Methods', 'GET, OPTIONS');
    res.setHeader('Access-Control-Allow-Headers', '*');
    res.setHeader('Access-Control-Allow-Private-Network', 'true');
    if (req.method === 'OPTIONS') return res.sendStatus(204);
    next();
});

const routesDir = path.join(__dirname, 'routes');

// Dev-mode route reloading: rebuild a fresh router per request so a
// changeset that adds/edits route files takes effect without a restart.
function buildRouter() {
    const router = express.Router();
    for (const file of fs.readdirSync(routesDir).filter((f) => f.endsWith('.js'))) {
        const full = path.join(routesDir, file);
        delete require.cache[require.resolve(full)];
        require(full)(router);
    }
    return router;
}

// Readiness probe target: the managed-service readiness check polls `/`
// and requires a 2xx/3xx answer.
app.get('/', (req, res) => res.json({ ok: true, service: 'backend' }));

app.use((req, res, next) => buildRouter()(req, res, next));

app.use((req, res) => {
    console.log(`[req] ${req.method} ${req.originalUrl} 404`);
    res.status(404).json({ error: 'not found' });
});

const port = Number(process.env.PORT || 8787);
app.listen(port, '127.0.0.1', () => console.log(`backend listening on ${port}`));
"#,
    )?;
    fs::create_dir_all(dir.join("routes"))?;
    fs::write(
        dir.join("routes/health.js"),
        "module.exports = (router) => {\n  router.get('/api/health', (req, res) => res.json({ ok: true }));\n};\n",
    )?;
    fs::write(
        dir.join("routes/items.js"),
        "module.exports = (router) => {\n  router.get('/api/items', (req, res) => res.json([{ id: 1, name: 'Item Alpha' }, { id: 2, name: 'Item Beta' }]));\n};\n",
    )?;
    fs::create_dir_all(dir.join("tests"))?;
    fs::write(
        dir.join("tests/routes.test.js"),
        r#"const test = require('node:test');
const assert = require('node:assert');
const fs = require('fs');
const path = require('path');

test('every route module registers at least one route', () => {
    const routesDir = path.join(__dirname, '..', 'routes');
    for (const file of fs.readdirSync(routesDir).filter((f) => f.endsWith('.js'))) {
        const register = require(path.join(routesDir, file));
        assert.strictEqual(typeof register, 'function', `${file} must export a register function`);
        const routes = [];
        register({ get: (route) => routes.push(route) });
        assert.ok(routes.length > 0, `${file} must register at least one route`);
    }
});
"#,
    )?;
    fs::create_dir_all(dir.join("scripts"))?;
    fs::write(
        dir.join("scripts/build.js"),
        "require('fs').mkdirSync('dist', { recursive: true });\nrequire('fs').writeFileSync('dist/out.txt', 'backend built');\nconsole.log('backend built');\n",
    )?;
    run_npm(&["install", "express"], dir).await?;
    Ok(())
}

/// Vite React app created by the real `npm create vite` scaffold. The
/// vite config consumes the injected `BACKEND_URL` env exactly as ADR
/// decision 4 prescribes: the dev proxy target AND a `define` global that
/// page code uses to call the backend origin directly (so the browser
/// network log shows a backend-origin request for diagnose to correlate).
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
        r#"import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// E3 smoke: the managed-service env contract (ADR decision 4). The backend
// spec's port/url arrive as BACKEND_URL / BACKEND_PORT in this process env.
const backend = process.env.BACKEND_URL ?? 'http://127.0.0.1:8787'

export default defineConfig({
  plugins: [react()],
  define: {
    __BACKEND_URL__: JSON.stringify(backend),
  },
  server: {
    // HMR off: the smoke asserts via fresh page loads; a live HMR websocket
    // also keeps headless Chrome's virtual-time budget from ever draining.
    hmr: false,
    proxy: {
      '/api': { target: backend, changeOrigin: true },
    },
  },
})
"#,
    )?;
    fs::write(
        dir.join("src/vite-env.d.ts"),
        "/// <reference types=\"vite/client\" />\ndeclare const __BACKEND_URL__: string\n",
    )?;
    fs::write(
        dir.join("src/App.tsx"),
        r#"import { useEffect, useState } from 'react'
import './App.css'

export interface Item {
  id: number
  name: string
}

export type Payload = Item[] | { error: string }

function itemsOf(payload: Payload): Item[] {
  return Array.isArray(payload) ? payload : []
}

function errorOf(payload: Payload): string {
  return Array.isArray(payload) ? '' : payload.error
}

// Direct call against the injected BACKEND_URL origin (vite `define` global).
// The dev-proxy consumption of the same env is asserted over HTTP.
// The injected URL carries a trailing slash (ADR decision 4); trim it.
const backendOrigin = __BACKEND_URL__.replace(/\/$/, '')

async function load(path: string, label: string): Promise<Payload> {
  try {
    const response = await fetch(`${backendOrigin}${path}`)
    if (!response.ok) throw new Error(`${label} ${response.status}`)
    return (await response.json()) as Item[]
  } catch (error) {
    return { error: error instanceof Error ? error.message : String(error) }
  }
}

function App() {
  const [directPayload, setDirectPayload] = useState<Payload>([])

  useEffect(() => {
    load('/api/items', 'direct').then(setDirectPayload)
  }, [])

  const directItems = itemsOf(directPayload)
  const directError = errorOf(directPayload)

  return (
    <div data-backend={__BACKEND_URL__}>
      <h1>Storefront</h1>
      <section id="direct-items">
        {directItems.map((item) => (
          <span key={item.id} className="direct-item">{item.name}</span>
        ))}
        {directError && <em id="direct-error">{directError}</em>}
      </section>
    </div>
  )
}

export default App
"#,
    )?;
    fs::write(
        dir.join("src/main.tsx"),
        r#"import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import App from './App.tsx'

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
"#,
    )?;
    Ok(())
}

// --- browser (headless Chrome) ----------------------------------------------
//
// Chrome-headless's --dump-dom/--screenshot can render the DOM and still
// linger on exit (kept-alive dev-server connections); the helpers below
// therefore collect output on a side thread, give Chrome a bounded window,
// and kill it afterwards. The DOM/PNG is what matters, not the exit code.

fn chrome_spawn(profile: &Path, extra: &[String], url: &str) -> Result<std::process::Child> {
    let child = Command::new(CHROME)
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
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("launch headless Chrome")?;
    Ok(child)
}

fn chrome_dom(profile: &Path, url: &str) -> Result<String> {
    let mut child = chrome_spawn(profile, &["--dump-dom".to_owned()], url)?;
    let mut stdout = child.stdout.take().context("chrome stdout")?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    let bytes = match rx.recv_timeout(Duration::from_secs(45)) {
        Ok(bytes) => bytes,
        Err(_) => {
            let _ = child.kill();
            rx.recv().unwrap_or_default()
        }
    };
    let _ = child.wait();
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn chrome_shot(profile: &Path, url: &str, png: &Path) -> Result<()> {
    let flag = format!("--screenshot={}", png.display());
    let mut child = chrome_spawn(
        profile,
        &[flag, "--window-size=1280,800".to_owned()],
        url,
    )?;
    // The screenshot file appears once the budget drains; poll, then kill.
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

// --- protocol helpers --------------------------------------------------------

fn default_client_info() -> ClientInfo {
    ClientInfo {
        name: "ody-app-server-tests".to_string(),
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

async fn read_error(mcp: &mut TestAppServer, request_id: i64) -> Result<String> {
    let message = timeout(
        PROTOCOL_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;
    Ok(message.error.message)
}

const PROJECT: &str = "smoke-ws-1";

async fn bind(mcp: &mut TestAppServer, roots: Vec<PathBuf>) -> Result<()> {
    let request_id = mcp
        .send_workspace_project_bind_request(WorkspaceProjectBindParams {
            id: PROJECT.to_owned(),
            name: "E3 smoke (vite + express)".to_owned(),
            roots: roots.into_iter().map(|path| path.abs()).collect(),
            idempotency_key: "smoke-bind".to_owned(),
        })
        .await?;
    let _: serde::de::IgnoredAny = read_response(mcp, request_id).await?;
    Ok(())
}

async fn define(
    mcp: &mut TestAppServer,
    name: &str,
    root_index: u32,
    script: &str,
    cwd: Option<&str>,
    depends_on: Vec<&str>,
    health_path: Option<&str>,
    port: u16,
    idem: &str,
) -> Result<WorkspaceServiceSpec> {
    let request_id = mcp
        .send_workspace_service_define_request(WorkspaceServiceDefineParams {
            project_id: PROJECT.to_owned(),
            name: name.to_owned(),
            root_index,
            script: script.to_owned(),
            port: Some(port),
            cwd: cwd.map(str::to_owned),
            depends_on: depends_on.iter().map(|s| s.to_string()).collect(),
            health_check: health_path.map(|path| WorkspaceServiceHealthCheck {
                path: path.to_owned(),
                expect_status: Some(200),
                timeout_ms: None,
            }),
            ready_timeout_ms: Some(120_000), // vite cold start + dep pre-bundle
            env_refs: None,
            idempotency_key: idem.to_owned(),
        })
        .await?;
    let response: ody_app_server_protocol::WorkspaceServiceDefineResponse =
        read_response(mcp, request_id).await?;
    Ok(response.spec)
}

async fn start_all(
    mcp: &mut TestAppServer,
    idem: &str,
) -> Result<WorkspaceServiceStartAllResponse> {
    let request_id = mcp
        .send_workspace_service_start_all_request(WorkspaceServiceStartAllParams {
            project_id: PROJECT.to_owned(),
            names: vec![],
            secret_values: None,
            idempotency_key: idem.to_owned(),
        })
        .await?;
    read_response(mcp, request_id).await
}

async fn stop_all(mcp: &mut TestAppServer) -> Result<WorkspaceServiceStopAllResponse> {
    let request_id = mcp
        .send_workspace_service_stop_all_request(WorkspaceServiceStopAllParams {
            project_id: PROJECT.to_owned(),
            names: vec![],
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
                status: Some(404),
                error: None,
                occurred_at_ms: None,
            }],
            log_tail_bytes: None,
            max_candidates: None,
        })
        .await?;
    read_response(mcp, request_id).await
}

async fn changeset_create_apply(
    mcp: &mut TestAppServer,
    title: &str,
    changes: Vec<WorkspaceFileChange>,
    idem: &str,
) -> Result<()> {
    let request_id = mcp
        .send_workspace_source_changeset_create_request(WorkspaceChangeSetCreateParams {
            project_id: PROJECT.to_owned(),
            title: title.to_owned(),
            changes,
            idempotency_key: idem.to_owned(),
        })
        .await?;
    let create: ody_app_server_protocol::WorkspaceChangeSetCreateResponse =
        read_response(mcp, request_id).await?;
    let request_id = mcp
        .send_workspace_source_changeset_apply_request(WorkspaceChangeSetApplyParams {
            changeset_id: create.changeset.id,
            commit_message: None,
        })
        .await?;
    let _: ody_app_server_protocol::WorkspaceChangeSetApplyResponse =
        read_response(mcp, request_id).await?;
    Ok(())
}

fn add(root_index: u32, path: &str, content: &str) -> WorkspaceFileChange {
    WorkspaceFileChange {
        root_index,
        path: path.to_owned(),
        kind: WorkspaceFileChangeKind::Add,
        base_hash: None,
        content: Some(content.to_owned()),
    }
}

fn update(
    root_index: u32,
    path: &str,
    base_content: &str,
    content: &str,
) -> WorkspaceFileChange {
    WorkspaceFileChange {
        root_index,
        path: path.to_owned(),
        kind: WorkspaceFileChangeKind::Update,
        base_hash: Some(sha256_hex(base_content)),
        content: Some(content.to_owned()),
    }
}

async fn close(mcp: &mut TestAppServer) -> Result<()> {
    let request_id = mcp
        .send_workspace_project_close_request(WorkspaceProjectCloseParams {
            project_id: PROJECT.to_owned(),
        })
        .await?;
    let _: serde::de::IgnoredAny = read_response(mcp, request_id).await?;
    Ok(())
}

async fn expect_specs_unknown_project(mcp: &mut TestAppServer) -> Result<()> {
    let request_id = mcp
        .send_workspace_service_specs_request(WorkspaceServiceSpecsParams {
            project_id: PROJECT.to_owned(),
        })
        .await?;
    let message = read_error(mcp, request_id).await?;
    anyhow::ensure!(message.contains("unknown project id"), "{message}");
    Ok(())
}

fn port_free(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_ok()
}

// --- the smoke ---------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
#[ignore = "manual E3 smoke: requires npm registry access and local Chrome; run with -- --ignored --nocapture"]
async fn smoke_real_vite_express_fullstack() -> Result<()> {
    if !npm_ok() {
        eprintln!("smoke skipped: npm is not available");
        return Ok(());
    }
    let evidence = evidence_dir();
    fs::create_dir_all(&evidence)?;
    let scratch = TempDir::new()?;
    let work = scratch.path();

    println!("[smoke] step 1: scaffold real sample (vite react + express)");
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
    // root 0 = frontend (primary), root 1 = backend — the dual-root layout.
    bind(&mut mcp, vec![frontend_dir.clone(), backend_dir.clone()]).await?;

    println!("[smoke] step 2: define specs -> startAll (backend health gate + env injection)");
    define(
        &mut mcp,
        "backend",
        1,
        "dev",
        None,
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
        None,
        vec!["backend"],
        None,
        web_port,
        "smoke-spec-web",
    )
    .await?;
    let started = start_all(&mut mcp, "smoke-orch-1").await?;
    println!(
        "[smoke]   startAll: started={} reused={} failed={}",
        started.started.len(),
        started.reused.len(),
        started.failed.len()
    );
    anyhow::ensure!(started.failed.is_empty(), "startAll failed: {started:?}");
    let backend_ref = started
        .services
        .iter()
        .find(|s| s.name == "backend")
        .expect("backend service")
        .clone();
    let web_ref = started
        .services
        .iter()
        .find(|s| s.name == "web")
        .expect("web service")
        .clone();
    anyhow::ensure!(backend_ref.status == WorkspaceServiceStatus::Ready);
    anyhow::ensure!(web_ref.status == WorkspaceServiceStatus::Ready);

    // Proxy consumption (vite.config.ts reads process.env.BACKEND_URL):
    // asserted over HTTP. Note: Chrome-headless responses from the Vite 8
    // dev proxy stall at the keep-alive layer (direct backend calls work;
    // curl works) — the proxy path is therefore verified at the transport
    // level, and the browser evidence below covers the injected BACKEND_URL
    // reaching page code via `define`.
    let proxied = reqwest::get(format!("{}api/items", web_ref.url)).await?;
    anyhow::ensure!(
        proxied.status() == reqwest::StatusCode::OK,
        "vite proxy must forward to the injected backend URL: {}",
        proxied.status()
    );
    println!("[smoke]   proxy: GET /api/items via vite proxy -> 200");

    // Browser evidence: the page renders data fetched directly against the
    // injected BACKEND_URL origin (define global in page code).
    let dom = chrome_dom(&work.join("chrome-p2"), &web_ref.url)?;
    anyhow::ensure!(
        dom.contains(&format!("data-backend=\"http://127.0.0.1:{backend_port}/\"")),
        "injected BACKEND_URL must reach page code (vite define); dom tail: {}",
        dom.chars().rev().take(800).collect::<String>().chars().rev().collect::<String>()
    );
    anyhow::ensure!(
        dom.contains("direct-item"),
        "page must render items from the direct backend origin; dom tail: {}",
        dom.chars().rev().take(600).collect::<String>().chars().rev().collect::<String>()
    );
    chrome_shot(
        &work.join("chrome-s2"),
        &web_ref.url,
        &evidence.join("step2-storefront.png"),
    )?;
    println!(
        "[smoke]   browser: storefront renders direct-origin items, backend={}",
        backend_ref.url
    );

    println!("[smoke] step 3: new backend endpoint + new page, cross-root validate");
    let new_route = "module.exports = (router) => {\n  router.get('/api/items/new', (req, res) => res.json([{ id: 3, name: 'New Item Gamma' }]));\n};\n";
    let new_page_html = "<!doctype html>\n<html>\n  <head>\n    <title>New Items</title>\n  </head>\n  <body>\n    <div id=\"root\"></div>\n    <script type=\"module\" src=\"/src/new.tsx\"></script>\n  </body>\n</html>\n";
    let new_page_tsx = r#"import { useEffect, useState } from 'react'
import { createRoot } from 'react-dom/client'
import { type Item, type Payload } from './App'

function NewPage() {
  const [payload, setPayload] = useState<Payload>([])

  useEffect(() => {
    const origin = __BACKEND_URL__.replace(/\/$/, '')
    fetch(`${origin}/api/items/new`)
      .then((r) => {
        if (!r.ok) throw new Error(`direct ${r.status}`)
        return r.json()
      })
      .then((data: Item[]) => setPayload(data))
      .catch((e: unknown) =>
        setPayload({ error: e instanceof Error ? e.message : String(e) }),
      )
  }, [])

  const items = Array.isArray(payload) ? payload : []
  const error = Array.isArray(payload) ? '' : payload.error
  return (
    <div>
      <h1>New Items</h1>
      {items.map((item) => (
        <span key={item.id} className="new-item">{item.name}</span>
      ))}
      {error && <em id="new-error">{error}</em>}
    </div>
  )
}

createRoot(document.getElementById('root')!).render(<NewPage />)

export default NewPage
"#;
    changeset_create_apply(
        &mut mcp,
        "add /api/items/new route and new page",
        vec![
            add(1, "routes/items-new.js", new_route),
            add(0, "new.html", new_page_html),
            add(0, "src/new.tsx", new_page_tsx),
        ],
        "smoke-cs-new-endpoint",
    )
    .await?;
    let request_id = mcp
        .send_workspace_source_validate_request(WorkspaceSourceValidateParams {
            project_id: PROJECT.to_owned(),
            changeset_id: None,
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
            timeout_ms: Some(240_000),
        })
        .await?;
    let validated: WorkspaceSourceValidateResponse = read_response(&mut mcp, request_id).await?;
    println!(
        "[smoke]   validate runs: {:?}",
        validated
            .report
            .runs
            .iter()
            .map(|run| (run.root_index, run.status))
            .collect::<Vec<_>>()
    );
    anyhow::ensure!(
        validated
            .report
            .runs
            .iter()
            .all(|run| run.status == ody_app_server_protocol::WorkspaceValidationStatus::Succeeded),
        "cross-root validation must be green: {:?}",
        validated.report
    );
    let new_page_url = format!("{}new.html", web_ref.url);
    let new_dom = chrome_dom(&work.join("chrome-p3"), &new_page_url)?;
    anyhow::ensure!(
        new_dom.contains("new-item") && !new_dom.contains("new-error"),
        "new page must render /api/items/new from the backend origin"
    );
    chrome_shot(
        &work.join("chrome-s3"),
        &new_page_url,
        &evidence.join("step3-new-page.png"),
    )?;
    println!("[smoke]   browser: /new.html renders new-item from the backend origin");

    println!("[smoke] step 4: contract mismatch -> diagnose -> fix -> restart -> verify");
    let items_original = fs::read_to_string(backend_dir.join("routes/items.js"))?;
    let items_broken =
        "module.exports = (router) => {\n  router.get('/api/itemz', (req, res) => res.json([{ id: 1, name: 'Item Alpha' }]));\n};\n";
    changeset_create_apply(
        &mut mcp,
        "break contract: rename /api/items to /api/itemz",
        vec![update(1, "routes/items.js", &items_original, items_broken)],
        "smoke-cs-break",
    )
    .await?;
    let miss = check(
        &mut mcp,
        &web_ref.id,
        Some(format!("{}api/items", web_ref.url)),
    )
    .await?;
    println!(
        "[smoke]   broken contract: GET /api/items via proxy -> {:?}",
        miss.http_status
    );
    anyhow::ensure!(miss.http_status == Some(404), "expected 404, got {miss:?}");

    // Browser observation: the direct backend-origin fetch fails visibly.
    let broken_dom = chrome_dom(&work.join("chrome-p4a"), &web_ref.url)?;
    anyhow::ensure!(
        broken_dom.contains("direct 404"),
        "browser must surface the failed direct backend call"
    );
    chrome_shot(
        &work.join("chrome-s4a"),
        &web_ref.url,
        &evidence.join("step4-contract-broken.png"),
    )?;

    // The exact URL the browser observed (backend origin, from __BACKEND_URL__).
    let failure_url = format!("{}api/items", backend_ref.url);
    let diagnosis = diagnose(&mut mcp, failure_url.clone()).await?;
    let result = diagnosis.results.first().expect("one diagnosis");
    let matched = result.matched_service.as_ref().expect("matched backend");
    println!(
        "[smoke]   diagnose: matched={} status={:?} health_ok={:?} candidates={} excerpt_lines={}",
        matched.name,
        matched.status,
        result.health.as_ref().map(|h| h.ok),
        result.source_candidates.len(),
        result.log_excerpt.as_deref().unwrap_or("").lines().count(),
    );
    anyhow::ensure!(matched.name == "backend", "must match the backend service");
    anyhow::ensure!(matched.port == backend_port);
    anyhow::ensure!(result.health.as_ref().expect("health").ok);
    anyhow::ensure!(
        result
            .log_excerpt
            .as_deref()
            .unwrap_or_default()
            .contains("/api/items"),
        "backend log excerpt must name the failing path"
    );
    anyhow::ensure!(
        !result.source_candidates.is_empty(),
        "source candidates must point at route files: {:?}",
        result.notes
    );

    let items_fixed = items_original.clone();
    changeset_create_apply(
        &mut mcp,
        "fix contract: restore /api/items",
        vec![update(1, "routes/items.js", items_broken, &items_fixed)],
        "smoke-cs-fix",
    )
    .await?;
    let stopped = stop_all(&mut mcp).await?;
    println!(
        "[smoke]   restart: stopAll stopped {} services",
        stopped.stopped.len()
    );
    let restarted = start_all(&mut mcp, "smoke-orch-2").await?;
    println!(
        "[smoke]   restart: startAll started={} reused={} failed={}",
        restarted.started.len(),
        restarted.reused.len(),
        restarted.failed.len()
    );
    anyhow::ensure!(restarted.failed.is_empty(), "restart failed: {restarted:?}");
    anyhow::ensure!(
        restarted.started.len() == 2,
        "restart must respawn both services"
    );
    let web_after = restarted
        .services
        .iter()
        .find(|s| s.name == "web")
        .expect("web after restart")
        .clone();
    let fixed = check(
        &mut mcp,
        &web_after.id,
        Some(format!("{}api/items", web_after.url)),
    )
    .await?;
    println!(
        "[smoke]   fixed: GET /api/items via proxy -> {:?}",
        fixed.http_status
    );
    anyhow::ensure!(
        fixed.http_status == Some(200),
        "fix must verify 200: {fixed:?}"
    );
    let fixed_dom = chrome_dom(&work.join("chrome-p4b"), &web_after.url)?;
    anyhow::ensure!(
        fixed_dom.contains("direct-item"),
        "page must render again after fix"
    );
    chrome_shot(
        &work.join("chrome-s4b"),
        &web_after.url,
        &evidence.join("step4-fixed.png"),
    )?;

    println!("[smoke] step 5: cleanup (stopAll, port release, close purges specs)");
    stop_all(&mut mcp).await?;
    anyhow::ensure!(port_free(backend_port), "backend port {backend_port} still bound");
    anyhow::ensure!(port_free(web_port), "web port {web_port} still bound");
    close(&mut mcp).await?;
    expect_specs_unknown_project(&mut mcp).await?;
    println!("[smoke]   cleanup: ports {backend_port}/{web_port} free, specs purged on close");
    println!("[smoke] DONE — evidence dir: {}", evidence.display());
    Ok(())
}
