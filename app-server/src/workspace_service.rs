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

/// 单个回环地址族的占用探测。`AddrNotAvailable`/`Unsupported` 表示本机没有
/// 该地址族（例如没有 IPv6 的机器），这不叫「被占用」。
fn loopback_family_free(host: &str, port: u16) -> bool {
    match std::net::TcpListener::bind((host, port)) {
        Ok(_) => true,
        Err(err) => matches!(
            err.kind(),
            std::io::ErrorKind::AddrNotAvailable | std::io::ErrorKind::Unsupported
        ),
    }
}

/// E0 验收缺陷 D1：探测必须覆盖两个回环地址族。Vite 在 Node 的 DNS 解析下
/// 绑 `[::1]`（IPv6-only，`list` 返回的 url 也是 `http://[::1]:port/`），只看
/// 127.0.0.1 会把被占用的端口判成空闲 → `--strictPort` 启动立刻
/// `EADDRINUSE` 退出。
pub(crate) fn is_port_free(port: u16) -> bool {
    loopback_family_free("127.0.0.1", port) && loopback_family_free("::1", port)
}

pub(crate) fn pick_free_port() -> Option<u16> {
    // 系统分配的空闲端口只保证单地址族；两个族都能绑才算真正可用。
    for _ in 0..32 {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).ok()?;
        let port = listener.local_addr().ok()?.port();
        drop(listener);
        if is_port_free(port) {
            return Some(port);
        }
    }
    None
}

/// 重启恢复的孤儿回收结果。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum OrphanReapOutcome {
    /// 身份核对通过，整个进程组已回收。
    Reaped,
    /// pid 上已无进程（正常退出或已被回收）。
    NotRunning,
    /// pid 被复用或命令行不匹配：不动它，只记录原因。
    Skipped(String),
}

/// E0 验收缺陷 D2：app-server 被硬杀（崩溃 / SIGKILL）时 `ManagedService`
/// 的 Drop 来不及执行，dev server 进程组会变成孤儿继续占用端口，而 store
/// 只把记录标成 stopped——用户以为没有服务在跑，实际后台跑着旧代码。实测：
/// 孤儿占着 `[::1]:5173`，下一次 `--strictPort` 启动 244ms 就 exit 1。
///
/// 回收前必须核对进程身份：pid 会被复用，只按 pid 杀可能误伤无关进程。判据
/// 是记录里的 `--port {port}` 与脚本名同时出现在命令行里（npm 会改写自己的
/// 进程标题，因此无法逐字比对记录的命令行）。启动用 `process_group(0)` 让
/// 子进程自成进程组，所以记录的 pid 就是 pgid，`kill_process_group(pid)`
/// 正好覆盖 `npm → node(vite)` 整棵树。
///
/// Windows 由 Job Object 的 KILL_ON_JOB_CLOSE 在父进程死亡时兜底，不存在
/// 同类孤儿，因此只实现 unix 侧。
#[cfg(unix)]
pub(crate) fn reap_orphan_service_process(pid: u32, port: u16, script: &str) -> OrphanReapOutcome {
    let output = std::process::Command::new("ps")
        .args(["-o", "command=", "-p", &pid.to_string()])
        .output();
    let cmdline = match output {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_owned()
        }
        _ => return OrphanReapOutcome::NotRunning,
    };
    if cmdline.is_empty() {
        return OrphanReapOutcome::NotRunning;
    }
    if !cmdline.contains(&format!("--port {port}")) || !cmdline.contains(script) {
        return OrphanReapOutcome::Skipped(format!(
            "pid {pid} does not match the recorded dev server (--port {port}, script {script}): {cmdline}"
        ));
    }
    match kill_process_group(pid) {
        Ok(()) => OrphanReapOutcome::Reaped,
        Err(err) => OrphanReapOutcome::Skipped(err),
    }
}

#[cfg(not(unix))]
pub(crate) fn reap_orphan_service_process(
    _pid: u32,
    _port: u16,
    _script: &str,
) -> OrphanReapOutcome {
    OrphanReapOutcome::Skipped(
        "orphan reaping is unix-only; Windows relies on the Job Object".to_owned(),
    )
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
    /// Windows: Job Object binding the spawned tree (E4). None when job
    /// creation failed at start (taskkill /T fallback still active).
    #[cfg(windows)]
    pub(crate) job: Option<crate::workspace_service_windows::ServiceJob>,
}

impl ManagedService {
    /// Kill the whole service tree. Unix: killpg. Windows: Job Object
    /// TerminateJobObject, falling back to taskkill /T when the job is
    /// unavailable.
    pub(crate) fn kill_tree(&self) -> Result<(), String> {
        #[cfg(windows)]
        if let Some(job) = &self.job {
            if job.kill().is_ok() {
                return Ok(());
            }
            // fall through to taskkill
        }
        kill_process_group(self.pgid)
    }
}

impl Drop for ManagedService {
    fn drop(&mut self) {
        // Strategy 8.2 (no runaway processes): the Runtime exiting — or the
        // runtime handle map otherwise being torn down — must not leave the
        // spawned dev-server group behind. `terminated` only records that
        // the direct child exited: grandchildren in the group can survive
        // (package-manager wrappers killed by signal, OOM killer, ...), so
        // the drop kill is unconditional. Already-dead groups only cost one
        // ESRCH; errors never panic inside drop.
        // Windows: self.job drops after this (CloseHandle →
        // KILL_ON_JOB_CLOSE double insurance); kill_tree prefers the job's
        // TerminateJobObject so both paths converge on the Job Object.
        if let Err(err) = self.kill_tree() {
            tracing::debug!(
                pgid = self.pgid,
                error = %err,
                "managed service process group kill on handle drop"
            );
        }
    }
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
///
/// Both loopback families are probed every round: dev servers that bind
/// `localhost` verbatim (Vite's default under Node's DNS resolution) end up
/// IPv6-only on `[::1]`, and a 127.0.0.1-only probe would never see them.
/// reqwest client for loopback dev-service probes (readiness, health,
/// preview check, diagnose). Never honors system/env HTTP proxies: the
/// targets are the user's own dev servers on 127.0.0.1/[::1], and a system
/// proxy that answers for loopback breaks every probe — observed on macOS
/// with a system proxy at 127.0.0.1:12001 returning 502 for both loopback
/// origins (curl, which ignores system proxy settings, worked fine against
/// the same server; the macOS exceptions list is stored as one comma-joined
/// string that the proxy matcher never splits into entries).
pub(crate) fn loopback_probe_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(5))
        .no_proxy()
        .build()
        .expect("reqwest client")
}

/// On success the origin that actually answers is returned, so the service
/// URL handed to downstream env injection and preview checks is reachable.
pub(crate) async fn wait_ready(
    terminated: &Arc<AtomicBool>,
    port: u16,
    timeout_ms: i64,
) -> (ReadyOutcome, Option<String>) {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms as u64);
    let client = loopback_probe_client();
    let origins = [
        format!("http://127.0.0.1:{port}"),
        format!("http://[::1]:{port}"),
    ];
    loop {
        if terminated.load(Ordering::SeqCst) {
            return (ReadyOutcome::ProcessExited, None);
        }
        if tokio::time::Instant::now() >= deadline {
            return (ReadyOutcome::TimedOut, None);
        }
        for origin in &origins {
            let url = format!("{origin}/");
            match tokio::time::timeout(
                Duration::from_millis(SERVICE_HEALTH_TIMEOUT_MS),
                client.get(&url).send(),
            )
            .await
            {
                Ok(Ok(response)) => {
                    let status = response.status().as_u16();
                    if (200..400).contains(&status) {
                        return (ReadyOutcome::Ready, Some(format!("{origin}/")));
                    }
                }
                _ => {}
            }
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
    #[test]
    fn pick_port_auto_avoids_when_default_occupied() {
        // 占用端口保持存活，`pick_port(None)` 必须绕过它。断言只落在「不等于
        // 被占端口」这个确定性性质上：临时端口在并行测试间会被重新分配，
        // 任何「返回值此刻仍空闲」的断言都是 TOCTOU 竞态。
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let occupied = listener.local_addr().expect("addr").port();
        let picked = pick_port(None, &[]).expect("auto-selected port");
        assert_ne!(picked, occupied);
        drop(listener);
    }

    fn is_port_free_sees_ipv6_only_loopback_listeners() {
        // E0 验收缺陷 D1：runtime 自己起的 vite 是 IPv6-only（[::1]），而
        // 探测只看 127.0.0.1 → 探测判「空闲」、--strictPort 启动立刻撞车。
        let listener = match std::net::TcpListener::bind("[::1]:0") {
            Ok(listener) => listener,
            Err(_) => return, // 本机无 IPv6：跳过
        };
        let occupied = listener.local_addr().expect("addr").port();
        assert!(
            !is_port_free(occupied),
            "IPv6-only listener on [::1]:{occupied} must count as occupied"
        );
        drop(listener);
        // 不再断言「drop 后立刻空闲」：临时端口可能被并行测试或系统重新占住，
        // 与被测行为无关。
    }

    #[test]
    fn pick_port_rejects_explicit_port_held_by_ipv6_only_listener() {
        let listener = match std::net::TcpListener::bind("[::1]:0") {
            Ok(listener) => listener,
            Err(_) => return,
        };
        let occupied = listener.local_addr().expect("addr").port();
        let err = pick_port(Some(occupied), &[]).expect_err("port held on ::1 must be rejected");
        assert!(err.contains(&occupied.to_string()), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn reap_orphan_service_process_kills_matching_group() {
        use std::os::unix::process::CommandExt;
        // E0 验收缺陷 D2：硬杀 app-server 后 ManagedService 的 Drop 不执行，
        // dev server 进程组成为孤儿并占着端口；重启恢复要按记录回收。
        // 命令行里带上记录过的 `--port` 与脚本名（npm 会改写进程标题，
        // 无法逐字比对记录的命令行）。
        let mut child = std::process::Command::new("sh")
            .args(["-c", "while true; do sleep 1; done", "--port", "5199", "dev"])
            .process_group(0)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sh");
        let pgid = child.id();
        assert_eq!(
            reap_orphan_service_process(pgid, 5199, "dev"),
            OrphanReapOutcome::Reaped
        );
        let _ = child.wait();
        // 组已消失：killpg 探测返回 ESRCH。
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let gone = unsafe { libc::killpg(pgid as libc::pid_t, 0) } != 0;
            if gone {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "orphan process group {pgid} survived reaping"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(unix)]
    #[test]
    fn reap_orphan_service_process_never_kills_a_reused_pid() {
        use std::os::unix::process::CommandExt;
        // pid 复用防线：命令行与记录不符时只记录、不动手。
        let mut child = std::process::Command::new("sh")
            .args(["-c", "while true; do sleep 1; done"])
            .process_group(0)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sh");
        let pgid = child.id();
        assert!(matches!(
            reap_orphan_service_process(pgid, 5199, "dev"),
            OrphanReapOutcome::Skipped(_)
        ));
        let alive = unsafe { libc::kill(pgid as libc::pid_t, 0) } == 0;
        assert!(alive, "unrelated process must survive the orphan reaper");
        unsafe { libc::killpg(pgid as libc::pid_t, libc::SIGKILL) };
        let _ = child.wait();
    }

    #[cfg(unix)]
    #[test]
    fn reap_orphan_service_process_reports_missing_process() {
        use std::os::unix::process::CommandExt;
        // 已退出的 pid：不 panic、不误报 Reaped。
        let mut child = std::process::Command::new("sh")
            .args(["-c", "exit 0", "--port", "5199", "dev"])
            .process_group(0)
            .spawn()
            .expect("spawn sh");
        let pgid = child.id();
        let _ = child.wait();
        assert_eq!(
            reap_orphan_service_process(pgid, 5199, "dev"),
            OrphanReapOutcome::NotRunning
        );
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

    #[tokio::test(flavor = "current_thread")]
    async fn wait_ready_reaches_ipv6_only_binder_and_reports_its_origin() {
        // Vite's default `localhost` bind is IPv6-only under Node's DNS
        // resolution; a 127.0.0.1-only readiness probe would time out.
        let listener = match tokio::net::TcpListener::bind("[::1]:0").await {
            Ok(listener) => listener,
            Err(_) => {
                eprintln!("skipping: IPv6 loopback is not available");
                return;
            }
        };
        let port = listener.local_addr().expect("addr").port();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            use tokio::io::AsyncWriteExt;
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok")
                .await
                .expect("write");
        });
        let terminated = Arc::new(AtomicBool::new(false));
        let (outcome, origin) = wait_ready(&terminated, port, 10_000).await;
        server.await.expect("server task");
        assert_eq!(outcome, ReadyOutcome::Ready);
        assert_eq!(
            origin.as_deref(),
            Some(format!("http://[::1]:{port}/").as_str()),
            "the origin that answered must be reported"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn wait_ready_prefers_ipv4_origin_when_it_answers() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            use tokio::io::AsyncWriteExt;
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok")
                .await
                .expect("write");
        });
        let terminated = Arc::new(AtomicBool::new(false));
        let (outcome, origin) = wait_ready(&terminated, port, 10_000).await;
        server.await.expect("server task");
        assert_eq!(outcome, ReadyOutcome::Ready);
        assert_eq!(
            origin.as_deref(),
            Some(format!("http://127.0.0.1:{port}/").as_str())
        );
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

    #[cfg(unix)]
    #[test]
    fn managed_service_drop_kills_unterminated_process_group() {
        use std::os::unix::process::CommandExt;
        // A long-lived child in its own process group, as spawn_service_command
        // produces; the handle is dropped without any explicit stop — the
        // Runtime-exit path. Strategy 8.2: no survivor.
        let mut child = std::process::Command::new("sleep")
            .arg("60")
            .process_group(0)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sleep");
        let pgid = child.id();
        let entry = ManagedService {
            pid: pgid,
            pgid,
            stop_requested: Arc::new(AtomicBool::new(false)),
            terminated: Arc::new(AtomicBool::new(false)),
            stdout_ring: Arc::new(Mutex::new(OutputRing::new(SERVICE_OUTPUT_TAIL_BYTES))),
            stderr_ring: Arc::new(Mutex::new(OutputRing::new(SERVICE_OUTPUT_TAIL_BYTES))),
            terminal_notify: Arc::new(Notify::new()),
        };
        drop(entry);
        // killpg(pgid, 0) probes for group existence: ESRCH once reaped.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let gone = unsafe { libc::killpg(pgid as libc::pid_t, 0) } != 0;
            if gone {
                child.wait().expect("reap probe child");
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "process group {pgid} survived ManagedService drop"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
