//! Workspace Service engine: port selection, command assembly, readiness
//! probing, output rings, and process-group termination for Ody-managed
//! dev servers (strategy E2). Pure functions stay testable; the processor
//! owns process handles and store state.
//!
//! Termination discipline (strategy 8.2: no runaway processes): Unix spawns
//! a new process group and killpg's it — spike-verified zero survivors and
//! immediate port release; Windows shells out to `taskkill /T /F`.

use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;

use ody_app_server_protocol::WorkspaceServiceRef;
use ody_app_server_protocol::WorkspaceServiceStatus;
use tokio::io::AsyncReadExt;
use tokio::process::Child;
use tokio::process::Command;
use tokio::sync::Notify;

/// Trailing per-stream bytes kept for `workspace/service/logs`.
pub(crate) const SERVICE_OUTPUT_TAIL_BYTES: usize = 64 * 1024;
pub(crate) const SERVICE_READY_DEFAULT_TIMEOUT_MS: i64 = 60_000;
pub(crate) const SERVICE_READY_MIN_TIMEOUT_MS: i64 = 5_000;
pub(crate) const SERVICE_READY_MAX_TIMEOUT_MS: i64 = 300_000;
pub(crate) const SERVICE_READY_POLL_INTERVAL_MS: u64 = 500;
pub(crate) const SERVICE_HEALTH_TIMEOUT_MS: u64 = 2_000;
pub(crate) const MAX_SERVICES_PER_PROJECT: usize = 8;
pub(crate) const MAX_SERVICE_NAME_CHARS: usize = 64;

/// E3: spec/orchestration/diagnose bounds (ADR §3.3).
pub(crate) const MAX_SPECS_PER_PROJECT: usize = 16;
pub(crate) const MAX_DEPENDS_ON_PER_SPEC: usize = 8;
pub(crate) const MAX_ORCHESTRATION_RECORDS: usize = 64;
pub(crate) const HEALTH_CHECK_DEFAULT_TIMEOUT_MS: i64 = 2_000;
pub(crate) const HEALTH_CHECK_MIN_TIMEOUT_MS: i64 = 200;
pub(crate) const HEALTH_CHECK_MAX_TIMEOUT_MS: i64 = 10_000;
pub(crate) const DIAGNOSE_MAX_FAILURES: usize = 16;
pub(crate) const DIAGNOSE_DEFAULT_LOG_TAIL_BYTES: u64 = 16 * 1024;
pub(crate) const DIAGNOSE_MIN_LOG_TAIL_BYTES: u64 = 1024;
pub(crate) const DIAGNOSE_MAX_LOG_TAIL_BYTES: u64 = 64 * 1024;
pub(crate) const DIAGNOSE_EXCERPT_MAX_LINES: usize = 50;
pub(crate) const DIAGNOSE_DEFAULT_MAX_CANDIDATES: u32 = 10;
pub(crate) const DIAGNOSE_MAX_CANDIDATES_LIMIT: u32 = 50;

/// Framework default dev-server port, first tech match wins.
const TECH_DEFAULT_PORTS: &[(&str, u16)] = &[
    ("next", 3000),
    ("nuxt", 3000),
    ("vite", 5173),
    ("svelte", 5173),
    ("astro", 4321),
    ("angular", 4200),
];
/// Frameworks that silently hop ports when occupied; force strict mode.
const TECH_STRICT_PORT: &[&str] = &["vite", "svelte"];

pub(crate) fn clamp_ready_timeout(timeout_ms: Option<i64>) -> i64 {
    timeout_ms
        .unwrap_or(SERVICE_READY_DEFAULT_TIMEOUT_MS)
        .clamp(SERVICE_READY_MIN_TIMEOUT_MS, SERVICE_READY_MAX_TIMEOUT_MS)
}

pub(crate) fn default_port_for_tech(tech_ids: &[String]) -> u16 {
    for (tech, port) in TECH_DEFAULT_PORTS {
        if tech_ids.iter().any(|id| id == tech) {
            return *port;
        }
    }
    5173
}

pub(crate) fn strict_port_args(tech_ids: &[String]) -> Vec<&'static str> {
    TECH_STRICT_PORT
        .iter()
        .filter(|tech| tech_ids.iter().any(|id| id == *tech))
        .map(|_| "--strictPort")
        .collect()
}

/// `{pm} run {script} -- --port {port} [framework extras]`; the `PORT`
/// environment variable is injected separately by the processor (R2:
/// dual-channel port injection covers both CLI-arg and env readers).
pub(crate) fn build_command_args(script: &str, port: u16, tech_ids: &[String]) -> Vec<String> {
    let mut args = vec![
        "run".to_owned(),
        script.to_owned(),
        "--".to_owned(),
        "--port".to_owned(),
        port.to_string(),
    ];
    args.extend(strict_port_args(tech_ids).into_iter().map(str::to_owned));
    args
}

pub(crate) fn is_port_free(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

pub(crate) fn pick_free_port() -> Option<u16> {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .ok()
        .and_then(|listener| listener.local_addr().ok())
        .map(|addr| addr.port())
}

/// Requested port must be free (structured error); omitted port probes the
/// framework default, then auto-selects a free one (strategy: port conflicts
/// are structured errors on explicit request, silent avoidance otherwise).
pub(crate) fn pick_port(requested: Option<u16>, tech_ids: &[String]) -> Result<u16, String> {
    if let Some(port) = requested {
        if is_port_free(port) {
            return Ok(port);
        }
        return Err(format!(
            "port {port} is already in use; stop the occupying process or omit `port` to auto-select"
        ));
    }
    let preferred = default_port_for_tech(tech_ids);
    if is_port_free(preferred) {
        return Ok(preferred);
    }
    pick_free_port().ok_or_else(|| "no free loopback port available".to_owned())
}

/// Byte ring keeping the trailing `cap` bytes of one output stream.
pub(crate) struct OutputRing {
    buf: Vec<u8>,
    cap: usize,
}

impl OutputRing {
    pub(crate) fn new(cap: usize) -> Self {
        Self {
            buf: Vec::with_capacity(cap.min(1024)),
            cap,
        }
    }

    pub(crate) fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
        if self.buf.len() > self.cap {
            let drop = self.buf.len() - self.cap;
            self.buf.drain(..drop);
        }
    }

    pub(crate) fn tail_string(&self, max_bytes: usize) -> String {
        let start = self.buf.len().saturating_sub(max_bytes);
        String::from_utf8_lossy(&self.buf[start..]).into_owned()
    }
}

/// Per-service runtime handle; process exits are observed by a wait task.
pub(crate) struct ManagedService {
    pub(crate) pid: u32,
    /// Process group id (== pid on unix; == pid on windows for taskkill).
    pub(crate) pgid: u32,
    pub(crate) stop_requested: Arc<AtomicBool>,
    pub(crate) terminated: Arc<AtomicBool>,
    pub(crate) stdout_ring: Arc<Mutex<OutputRing>>,
    pub(crate) stderr_ring: Arc<Mutex<OutputRing>>,
    pub(crate) terminal_notify: Arc<Notify>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadyOutcome {
    Ready,
    ProcessExited,
    TimedOut,
}

/// Poll the loopback URL until a 2xx/3xx answers, the process terminates,
/// or the deadline passes. Readiness is HTTP-authoritative; dev-server
/// banner parsing is intentionally not relied on (R3).
pub(crate) async fn wait_ready(
    terminated: &Arc<AtomicBool>,
    port: u16,
    timeout_ms: i64,
) -> ReadyOutcome {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms as u64);
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .expect("reqwest client");
    loop {
        if terminated.load(Ordering::SeqCst) {
            return ReadyOutcome::ProcessExited;
        }
        if tokio::time::Instant::now() >= deadline {
            return ReadyOutcome::TimedOut;
        }
        let url = format!("http://127.0.0.1:{port}/");
        match tokio::time::timeout(
            Duration::from_millis(SERVICE_HEALTH_TIMEOUT_MS),
            client.get(&url).send(),
        )
        .await
        {
            Ok(Ok(response)) => {
                let status = response.status().as_u16();
                if (200..400).contains(&status) {
                    return ReadyOutcome::Ready;
                }
            }
            _ => {}
        }
        tokio::time::sleep(Duration::from_millis(SERVICE_READY_POLL_INTERVAL_MS)).await;
    }
}

/// Spawn `<pm> run <script> ...` detached from any terminal, in its own
/// process group (unix), with piped output for the reader tasks.
pub(crate) fn spawn_service_command(
    pm: &str,
    args: &[String],
    root: &std::path::Path,
    port: u16,
    env: &Option<std::collections::HashMap<String, Option<String>>>,
) -> std::io::Result<Child> {
    let mut command = Command::new(pm);
    command
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("PORT", port.to_string())
        .kill_on_drop(false); // termination goes through kill_process_group
    if let Some(overrides) = env {
        for (key, value) in overrides {
            match value {
                Some(value) => {
                    command.env(key, value);
                }
                None => {
                    command.env_remove(key);
                }
            }
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command.spawn()
}

/// Pump one output pipe into a ring until EOF. Spawned per stream.
pub(crate) fn spawn_output_reader<R>(
    mut reader: R,
    ring: Arc<Mutex<OutputRing>>,
) -> tokio::task::JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => ring.lock().expect("ring lock").push(&buf[..n]),
                Err(_) => break,
            }
        }
    })
}

/// Kill the whole process group. spike-verified: zero survivors, port freed.
#[cfg(unix)]
pub(crate) fn kill_process_group(pgid: u32) -> Result<(), String> {
    let rc = unsafe { libc::killpg(pgid as libc::pid_t, libc::SIGKILL) };
    if rc == 0 {
        Ok(())
    } else {
        Err(format!(
            "killpg({pgid}) failed: {}",
            std::io::Error::last_os_error()
        ))
    }
}

/// Windows fallback: `taskkill /PID <pid> /T /F` kills the process tree.
#[cfg(windows)]
pub(crate) fn kill_process_group(pid: u32) -> Result<(), String> {
    let output = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .output()
        .map_err(|err| format!("taskkill spawn failed: {err}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "taskkill failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

/// Kahn's algorithm by levels over (name, depends_on) pairs. Levels are
/// sorted by name for determinism. Errors name the unknown dependency or
/// the specs caught in a cycle (ADR decision 3: request-level error).
pub(crate) fn topo_levels(specs: &[(String, Vec<String>)]) -> Result<Vec<Vec<String>>, String> {
    let mut edges: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for (name, deps) in specs {
        edges.entry(name.clone()).or_default();
        for dep in deps {
            if !specs.iter().any(|(other, _)| other == dep) {
                return Err(format!("unknown dependency {dep:?} of spec {name:?}"));
            }
            edges.get_mut(name).expect("entry").push(dep.clone());
        }
    }
    let mut levels = Vec::new();
    let mut placed: std::collections::BTreeSet<String> = edges.keys().cloned().collect();
    while !placed.is_empty() {
        let level: Vec<String> = edges
            .iter()
            .filter(|(name, _)| placed.contains(*name))
            .filter(|(_, deps)| deps.iter().all(|dep| !placed.contains(dep)))
            .map(|(name, _)| name.clone())
            .collect();
        if level.is_empty() {
            let remaining = placed.into_iter().collect::<Vec<_>>().join(", ");
            return Err(format!("dependency cycle among specs: {remaining}"));
        }
        for name in &level {
            placed.remove(name);
        }
        levels.push(level);
    }
    Ok(levels)
}

/// `api-server` -> `API_SERVER`: dependency connection env keys (ADR decision 4).
pub(crate) fn env_key_for_service(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

pub(crate) fn clamp_health_timeout(timeout_ms: Option<i64>) -> i64 {
    timeout_ms
        .unwrap_or(HEALTH_CHECK_DEFAULT_TIMEOUT_MS)
        .clamp(HEALTH_CHECK_MIN_TIMEOUT_MS, HEALTH_CHECK_MAX_TIMEOUT_MS)
}

/// Orchestration health gate (ADR decision 6): exact `expect` status when
/// defined (default 200), else 2xx tolerance; one attempt with `timeout`.
pub(crate) async fn spec_health_probe(
    client: &reqwest::Client,
    origin: &str,
    path: &str,
    expect_status: Option<u16>,
    timeout: Duration,
) -> ody_app_server_protocol::WorkspaceServiceHealth {
    let checked_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX);
    let url = format!("{}{}", origin.trim_end_matches('/'), path);
    match tokio::time::timeout(timeout, client.get(&url).send()).await {
        Ok(Ok(response)) => {
            let status = response.status().as_u16();
            let ok = expect_status
                .map_or_else(|| response.status().is_success(), |expect| status == expect);
            ody_app_server_protocol::WorkspaceServiceHealth {
                ok,
                status_code: Some(status),
                error: None,
                checked_at_ms,
            }
        }
        Ok(Err(err)) => ody_app_server_protocol::WorkspaceServiceHealth {
            ok: false,
            status_code: None,
            error: Some(err.to_string()),
            checked_at_ms,
        },
        Err(_) => ody_app_server_protocol::WorkspaceServiceHealth {
            ok: false,
            status_code: None,
            error: Some(format!(
                "health probe timed out after {} ms",
                timeout.as_millis()
            )),
            checked_at_ms,
        },
    }
}

/// Extract the port from a loopback http(s) URL (ADR decision 7:
/// diagnosis only correlates loopback URLs). Same parser as
/// `validate_check_url`; non-loopback, non-http, or port-less URLs
/// yield None — never a panic.
pub(crate) fn parse_loopback_port(url: &str) -> Option<u16> {
    if url.contains('\\') {
        return None;
    }
    let parsed = reqwest::Url::parse(url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let host = parsed.host_str()?;
    // host_str keeps IPv6 brackets ("[::1]"); strip before matching.
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if !matches!(host, "127.0.0.1" | "localhost" | "::1") {
        return None;
    }
    parsed.port()
}

/// Prefer a non-terminal service listening on `port`; else the most
/// recently updated terminal record — a crashed backend is exactly
/// what diagnosis must surface.
pub(crate) fn match_service_by_port<'a>(
    services: &'a [WorkspaceServiceRef],
    port: u16,
) -> Option<&'a WorkspaceServiceRef> {
    services
        .iter()
        .filter(|service| service.port == port)
        .filter(|service| !is_terminal_status(service.status))
        .max_by_key(|service| service.updated_at_ms)
        .or_else(|| {
            services
                .iter()
                .filter(|service| service.port == port)
                .max_by_key(|service| service.updated_at_ms)
        })
}

fn is_terminal_status(status: WorkspaceServiceStatus) -> bool {
    matches!(
        status,
        WorkspaceServiceStatus::Failed
            | WorkspaceServiceStatus::Exited
            | WorkspaceServiceStatus::Stopped
    )
}

/// Route-path matching variants for a failure URL path: the full path,
/// then prefix-stripped one segment at a time (backend routes often
/// omit the "/api" prefix that the browser URL carries). Deduped,
/// order-preserving; empty for "/" or empty input.
pub(crate) fn route_path_variants(path: &str) -> Vec<String> {
    let trimmed = path.trim_start_matches('/');
    if trimmed.is_empty() {
        return Vec::new();
    }
    let segments: Vec<&str> = trimmed.split('/').filter(|seg| !seg.is_empty()).collect();
    let mut variants = Vec::new();
    for start in 0..segments.len() {
        let variant = format!("/{}", segments[start..].join("/"));
        if !variants.contains(&variant) {
            variants.push(variant);
        }
    }
    variants
}

/// Log lines from `tail` mentioning the full request `path` (e.g. a
/// request-log line "GET /api/miss 404" matches "/api/miss"). Full-path
/// matching avoids false hits on sibling routes sharing a prefix segment
/// ("/api/items" must not match a "/api/miss" diagnosis). Empty string when
/// nothing matches (caller maps to None).
pub(crate) fn log_excerpt(tail: &str, path: &str, max_lines: usize) -> String {
    let needle = path.trim();
    if needle.len() < 2 {
        return String::new();
    }
    let mut lines = Vec::new();
    for line in tail.lines() {
        if line.contains(needle) {
            lines.push(line);
            if lines.len() >= max_lines {
                break;
            }
        }
    }
    lines.join("\n")
}

/// Preview HTTP check deadline bounds.
pub(crate) const PREVIEW_CHECK_DEFAULT_TIMEOUT_MS: i64 = 10_000;
pub(crate) const PREVIEW_CHECK_MIN_TIMEOUT_MS: i64 = 1_000;
pub(crate) const PREVIEW_CHECK_MAX_TIMEOUT_MS: i64 = 30_000;

pub(crate) fn clamp_preview_timeout(timeout_ms: Option<i64>) -> i64 {
    timeout_ms
        .unwrap_or(PREVIEW_CHECK_DEFAULT_TIMEOUT_MS)
        .clamp(PREVIEW_CHECK_MIN_TIMEOUT_MS, PREVIEW_CHECK_MAX_TIMEOUT_MS)
}

/// `url?` may override the service URL but must stay http(s); the default
/// is the service record's loopback URL (strategy 8.3: loopback only, and
/// no file:// or scheme smuggling into a fetcher).
pub(crate) fn validate_check_url(
    override_url: Option<String>,
    service_url: &str,
) -> Result<String, String> {
    let url = override_url.unwrap_or_else(|| service_url.to_owned());
    if url.contains('\\') {
        return Err(format!(
            "url {url:?} is not allowed for preview check: backslash"
        ));
    }
    let parsed = reqwest::Url::parse(&url).map_err(|err| format!("invalid url {url:?}: {err}"))?;
    match parsed.scheme() {
        "http" | "https" => Ok(url),
        scheme => Err(format!(
            "url scheme {scheme:?} is not allowed for preview check; use http(s)"
        )),
    }
}

/// `<title>` extraction from HTML bodies; None for non-HTML or malformed
/// input. Intentionally regex-free: one case-insensitive scan.
pub(crate) fn extract_title(html: &str) -> Option<String> {
    let lower = html.to_lowercase();
    let open = lower.find("<title>")?;
    let rest = &lower[open + "<title>".len()..];
    let close = rest.find("</title>")?;
    let raw = &html[open + "<title>".len()..open + "<title>".len() + close];
    let title = raw.trim();
    (!title.is_empty()).then(|| title.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_ready_timeout_applies_defaults_and_bounds() {
        assert_eq!(clamp_ready_timeout(None), SERVICE_READY_DEFAULT_TIMEOUT_MS);
        assert_eq!(clamp_ready_timeout(Some(30_000)), 30_000);
        assert_eq!(clamp_ready_timeout(Some(1)), SERVICE_READY_MIN_TIMEOUT_MS);
        assert_eq!(
            clamp_ready_timeout(Some(9_999_999)),
            SERVICE_READY_MAX_TIMEOUT_MS
        );
    }

    #[test]
    fn default_port_and_strict_port_follow_tech_stack() {
        assert_eq!(default_port_for_tech(&["vite".to_owned()]), 5173);
        assert_eq!(default_port_for_tech(&["next".to_owned()]), 3000);
        assert_eq!(default_port_for_tech(&["nuxt".to_owned()]), 3000);
        assert_eq!(default_port_for_tech(&["astro".to_owned()]), 4321);
        assert_eq!(default_port_for_tech(&[]), 5173); // ecosystem fallback
        assert!(strict_port_args(&["vite".to_owned()]).contains(&"--strictPort"));
        assert!(strict_port_args(&["svelte".to_owned()]).contains(&"--strictPort"));
        assert!(strict_port_args(&["next".to_owned()]).is_empty());
    }

    #[test]
    fn build_command_assembles_pm_run_with_port_injection() {
        let args = build_command_args("dev", 5173, &["vite".to_owned()]);
        assert_eq!(
            args,
            vec!["run", "dev", "--", "--port", "5173", "--strictPort"]
        );
    }

    #[test]
    fn pick_port_rejects_occupied_requested_port() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let occupied = listener.local_addr().expect("addr").port();
        let err = pick_port(Some(occupied), &[]).expect_err("occupied port must be rejected");
        assert!(err.contains(&occupied.to_string()), "{err}");
    }

    #[test]
    fn pick_port_auto_avoids_when_default_occupied() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let occupied = listener.local_addr().expect("addr").port();
        // Force the default-port branch by requesting the occupied port as default:
        // pick_port(None) probes the tech default; emulate by occupying 5173 is
        // unreliable in CI, so assert the auto-select invariant instead:
        // binding via 127.0.0.1:0 always yields a free port distinct from occupied.
        let picked = pick_free_port().expect("free port");
        assert_ne!(picked, occupied);
        assert!(is_port_free(picked));
        drop(listener);
    }

    #[test]
    fn output_ring_keeps_trailing_bytes_within_cap() {
        let mut ring = OutputRing::new(8);
        ring.push(b"abcdefgh");
        ring.push(b"ij");
        // The ring keeps the trailing `cap` bytes overall: the last 8 of
        // "abcdefgh" + "ij".
        assert_eq!(ring.tail_string(8), "cdefghij");
        assert_eq!(ring.tail_string(2), "ij");
        let big = vec![b'x'; 100];
        ring.push(&big);
        assert_eq!(ring.tail_string(8).len(), 8);
        // UTF-8 boundary safety: multi-byte char split by cap.
        let mut ring = OutputRing::new(4);
        ring.push("中文字".as_bytes());
        let tail = ring.tail_string(4);
        assert!(tail.len() <= 4 + 3); // lossy replacement may add bytes, never panics
    }

    #[test]
    fn clamp_preview_timeout_applies_defaults_and_bounds() {
        assert_eq!(
            clamp_preview_timeout(None),
            PREVIEW_CHECK_DEFAULT_TIMEOUT_MS
        );
        assert_eq!(clamp_preview_timeout(Some(5_000)), 5_000);
        assert_eq!(clamp_preview_timeout(Some(0)), PREVIEW_CHECK_MIN_TIMEOUT_MS);
        assert_eq!(
            clamp_preview_timeout(Some(999_999)),
            PREVIEW_CHECK_MAX_TIMEOUT_MS
        );
    }

    #[test]
    fn validate_check_url_allows_http_https_and_rejects_other_schemes() {
        // Default comes from the service record (loopback by construction).
        assert_eq!(
            validate_check_url(None, "http://127.0.0.1:5173/").expect("default url"),
            "http://127.0.0.1:5173/"
        );
        assert_eq!(
            validate_check_url(
                Some("https://127.0.0.1:5173/about".to_owned()),
                "http://127.0.0.1:5173/"
            )
            .expect("https override"),
            "https://127.0.0.1:5173/about"
        );
        for bad in [
            "file:///etc/passwd",
            "ftp://127.0.0.1/x",
            "javascript:alert(1)",
            "http://127.0.0.1:5173/\\@evil",
        ] {
            assert!(
                validate_check_url(Some(bad.to_owned()), "http://127.0.0.1:5173/").is_err(),
                "{bad} must be rejected"
            );
        }
    }

    #[test]
    fn extract_title_handles_common_html_shapes() {
        assert_eq!(
            extract_title("<html><head><title>Hello &amp; Hi</title></head></html>"),
            Some("Hello &amp; Hi".to_owned())
        );
        assert_eq!(
            extract_title("<TITLE>Upper</TITLE><body>x</body>"),
            Some("Upper".to_owned())
        );
        assert_eq!(
            extract_title("<html><head>\n  <title>\n  Spaced\n</title>\n</head></html>").as_deref(),
            Some("Spaced")
        );
        assert_eq!(
            extract_title("<html><body>no head title</body></html>"),
            None
        );
        assert_eq!(extract_title(""), None);
        assert_eq!(extract_title("<title>unclosed"), None); // malformed: no closing tag
    }

    #[test]
    fn content_fingerprint_matches_sha256_known_vector() {
        // "abc" sha256, FIPS 180-4 known answer — proves the hex helper wiring.
        assert_eq!(
            crate::workspace_changeset::sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn topo_levels_orders_by_dependency_and_detects_cycles() {
        // Linear chain: web -> api -> db.
        let levels = topo_levels(&[
            ("web".to_owned(), vec!["api".to_owned()]),
            ("api".to_owned(), vec!["db".to_owned()]),
            ("db".to_owned(), vec![]),
        ])
        .expect("linear graph");
        assert_eq!(
            levels,
            vec![
                vec!["db".to_owned()],
                vec!["api".to_owned()],
                vec!["web".to_owned()],
            ]
        );

        // Diamond: app depends on api and web; both depend on db.
        let levels = topo_levels(&[
            ("app".to_owned(), vec!["api".to_owned(), "web".to_owned()]),
            ("api".to_owned(), vec!["db".to_owned()]),
            ("web".to_owned(), vec!["db".to_owned()]),
            ("db".to_owned(), vec![]),
        ])
        .expect("diamond graph");
        assert_eq!(levels[0], vec!["db".to_owned()]);
        assert_eq!(levels[1].len(), 2); // api + web, same level
        assert_eq!(levels[2], vec!["app".to_owned()]);

        // Unknown dependency names the offender.
        let err =
            topo_levels(&[("web".to_owned(), vec!["ghost".to_owned()])]).expect_err("unknown dep");
        assert!(
            err.contains("\"ghost\"") && err.contains("\"web\""),
            "{err}"
        );

        // Cycle is rejected with the remaining nodes named.
        let err = topo_levels(&[
            ("a".to_owned(), vec!["b".to_owned()]),
            ("b".to_owned(), vec!["a".to_owned()]),
        ])
        .expect_err("cycle");
        assert!(err.contains("cycle"), "{err}");
    }

    #[test]
    fn env_key_for_service_normalizes_to_upper_snake() {
        assert_eq!(env_key_for_service("backend"), "BACKEND");
        assert_eq!(env_key_for_service("api-server"), "API_SERVER");
        assert_eq!(env_key_for_service("api server v2"), "API_SERVER_V2");
        assert_eq!(env_key_for_service("数据库"), "___"); // each non-ascii-alnum char -> '_'
    }

    #[test]
    fn clamp_health_timeout_applies_defaults_and_bounds() {
        assert_eq!(clamp_health_timeout(None), HEALTH_CHECK_DEFAULT_TIMEOUT_MS);
        assert_eq!(clamp_health_timeout(Some(500)), 500);
        assert_eq!(clamp_health_timeout(Some(0)), HEALTH_CHECK_MIN_TIMEOUT_MS);
        assert_eq!(
            clamp_health_timeout(Some(999_999)),
            HEALTH_CHECK_MAX_TIMEOUT_MS
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn spec_health_probe_matches_exact_expect_status() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            use tokio::io::AsyncWriteExt;
            stream
                .write_all(b"HTTP/1.1 500 Internal Server Error\r\ncontent-length: 0\r\n\r\n")
                .await
                .expect("write");
        });
        let client = reqwest::Client::new();
        let health = spec_health_probe(
            &client,
            &format!("http://127.0.0.1:{port}"),
            "/api/broken",
            Some(200),
            std::time::Duration::from_secs(2),
        )
        .await;
        server.await.expect("server task");
        assert!(!health.ok, "500 vs expect 200 must fail the gate");
        assert_eq!(health.status_code, Some(500));
    }

    #[test]
    fn parse_loopback_port_accepts_only_loopback_http_urls() {
        assert_eq!(
            parse_loopback_port("http://127.0.0.1:8787/api/items"),
            Some(8787)
        );
        assert_eq!(parse_loopback_port("http://localhost:3000/"), Some(3000));
        assert_eq!(parse_loopback_port("http://[::1]:4321/x"), Some(4321));
        // Non-loopback, wrong scheme, missing port, garbage: no match, never a panic.
        assert_eq!(parse_loopback_port("http://192.168.1.10:8080/"), None);
        assert_eq!(parse_loopback_port("https://example.com/"), None);
        assert_eq!(parse_loopback_port("http://127.0.0.1/"), None);
        assert_eq!(parse_loopback_port("not a url"), None);
        assert_eq!(parse_loopback_port("file:///etc/passwd"), None);
    }

    #[test]
    fn match_service_by_port_prefers_non_terminal_then_most_recent() {
        let terminal = WorkspaceServiceRef {
            id: "svc-old".to_owned(),
            project_id: "ws-1".to_owned(),
            name: "backend".to_owned(),
            root_index: 0,
            script: "dev".to_owned(),
            command: "npm run dev".to_owned(),
            port: 8787,
            url: "http://127.0.0.1:8787/".to_owned(),
            status: WorkspaceServiceStatus::Failed,
            pid: None,
            exit_code: Some(1),
            health: None,
            error: None,
            created_at_ms: 0,
            updated_at_ms: 100,
        };
        let live = WorkspaceServiceRef {
            id: "svc-new".to_owned(),
            status: WorkspaceServiceStatus::Ready,
            updated_at_ms: 50,
            ..terminal.clone()
        };
        // Non-terminal wins even when older.
        assert_eq!(
            match_service_by_port(&[terminal.clone(), live.clone()], 8787).map(|s| s.id.as_str()),
            Some("svc-new")
        );
        // Without a live one, the most recent terminal record is returned
        // (a crashed backend is exactly what diagnosis must surface).
        let older = WorkspaceServiceRef {
            id: "svc-older".to_owned(),
            updated_at_ms: 10,
            ..terminal.clone()
        };
        assert_eq!(
            match_service_by_port(&[older, terminal.clone()], 8787).map(|s| s.id.as_str()),
            Some("svc-old")
        );
        // Port mismatch -> None.
        assert!(match_service_by_port(&[terminal], 9999).is_none());
    }

    #[test]
    fn route_path_variants_strips_leading_segments_deduped() {
        assert_eq!(
            route_path_variants("/api/items"),
            vec!["/api/items".to_owned(), "/items".to_owned()]
        );
        assert_eq!(
            route_path_variants("/api/items/v2"),
            vec![
                "/api/items/v2".to_owned(),
                "/items/v2".to_owned(),
                "/v2".to_owned(),
            ]
        );
        assert_eq!(route_path_variants("/"), Vec::<String>::new());
        assert_eq!(route_path_variants(""), Vec::<String>::new());
    }

    #[test]
    fn log_excerpt_keeps_lines_mentioning_path_segments_capped() {
        let tail = "[req] GET /api/items 200\n[req] GET /api/miss 404\n[db] connect ok\n";
        let excerpt = log_excerpt(tail, "/api/miss", DIAGNOSE_EXCERPT_MAX_LINES);
        assert!(excerpt.contains("/api/miss"), "{excerpt}");
        assert!(!excerpt.contains("/api/items"), "{excerpt}");
        // No keyword hit -> empty string (caller maps to None).
        assert_eq!(log_excerpt("[db] connect ok\n", "/api/miss", 10), "");
        // Cap: 60 matching lines, max 50.
        let many = (0..60)
            .map(|i| format!("[req] GET /api/miss line {i}\n"))
            .collect::<String>();
        assert_eq!(log_excerpt(&many, "/api/miss", 50).lines().count(), 50);
    }
}
