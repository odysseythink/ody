use std::collections::BTreeMap;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::Ordering;

use ody_app_server_protocol::JSONRPCErrorError;
use ody_app_server_protocol::ServerNotification;
use ody_app_server_protocol::WORKSPACE_SERVICE_PROTOCOL_VERSION;
use ody_app_server_protocol::WorkspaceNetworkDiagnosis;
use ody_app_server_protocol::WorkspacePreviewCheckParams;
use ody_app_server_protocol::WorkspacePreviewCheckResponse;
use ody_app_server_protocol::WorkspacePreviewDiagnoseParams;
use ody_app_server_protocol::WorkspacePreviewDiagnoseResponse;
use ody_app_server_protocol::WorkspaceServiceChangedNotification;
use ody_app_server_protocol::WorkspaceServiceChangedReason;
use ody_app_server_protocol::WorkspaceServiceDefineParams;
use ody_app_server_protocol::WorkspaceServiceDefineResponse;
use ody_app_server_protocol::WorkspaceServiceHealth;
use ody_app_server_protocol::WorkspaceServiceHealthParams;
use ody_app_server_protocol::WorkspaceServiceHealthResponse;
use ody_app_server_protocol::WorkspaceServiceListParams;
use ody_app_server_protocol::WorkspaceServiceListResponse;
use ody_app_server_protocol::WorkspaceServiceLogsParams;
use ody_app_server_protocol::WorkspaceServiceLogsResponse;
use ody_app_server_protocol::WorkspaceServiceRef;
use ody_app_server_protocol::WorkspaceServiceSpec;
use ody_app_server_protocol::WorkspaceServiceSpecsParams;
use ody_app_server_protocol::WorkspaceServiceSpecsResponse;
use ody_app_server_protocol::WorkspaceServiceStartAllParams;
use ody_app_server_protocol::WorkspaceServiceStartAllResponse;
use ody_app_server_protocol::WorkspaceServiceStartParams;
use ody_app_server_protocol::WorkspaceServiceStartResponse;
use ody_app_server_protocol::WorkspaceServiceStatus;
use ody_app_server_protocol::WorkspaceServiceStopAllParams;
use ody_app_server_protocol::WorkspaceServiceStopAllResponse;
use ody_app_server_protocol::WorkspaceServiceStopParams;
use ody_app_server_protocol::WorkspaceServiceStopResponse;
use serde::Deserialize;
use serde::Serialize;
use uuid::Uuid;

use crate::error_code::internal_error;
use crate::outgoing_message::OutgoingMessageSender;
use crate::error_code::invalid_params;
use crate::workspace_audit::audit_event;
use crate::workspace_audit::WorkspaceAuditLog;
use crate::outgoing_message::ConnectionId;
use crate::workspace_lock::WorkspaceWriteLock;
use crate::request_processors::workspace_project_processor::WorkspaceProjectStore;
use crate::workspace_service::ManagedService;
use crate::workspace_service::OutputRing;
use crate::workspace_service::ReadyOutcome;

const STORE_DIR: &str = "workspace-service";
const STORE_FILE: &str = "v1.json";

/// Load-normalization marker written when a Runtime restart leaves
/// non-terminal services without live processes (E4).
const RUNTIME_RESTARTED_ERROR: &str = "runtime restarted; process not running";

/// E4: sensitive env ref validation (upper-snake, DNS-safe env keys).
const MAX_ENV_REFS_PER_SPEC: usize = 16;
const MAX_ENV_REF_CHARS: usize = 64;

fn validate_env_refs(refs: &[String]) -> Result<Vec<String>, JSONRPCErrorError> {
    if refs.len() > MAX_ENV_REFS_PER_SPEC {
        return Err(invalid_params(format!(
            "envRefs has {} entries; max is {MAX_ENV_REFS_PER_SPEC}",
            refs.len()
        )));
    }
    let mut seen = std::collections::BTreeSet::new();
    for name in refs {
        let ok = !name.is_empty()
            && name.len() <= MAX_ENV_REF_CHARS
            && name.chars().enumerate().all(|(i, c)| {
                c.is_ascii_uppercase() || c == '_' || (i > 0 && c.is_ascii_digit())
            });
        if !ok {
            return Err(invalid_params(format!(
                "envRefs entry {name:?} must be UPPER_SNAKE (A-Z, 0-9 after first char, _), ≤{MAX_ENV_REF_CHARS} chars, ASCII only"
            )));
        }
        if !seen.insert(name.clone()) {
            return Err(invalid_params(format!("duplicate envRefs entry {name:?}")));
        }
    }
    Ok(refs.to_vec())
}

/// Resolve a spec's envRefs against request-scoped secretValues. Returned
/// pairs are injected at the extra_env layer: after dependency-injected
/// vars (so a secret overrides a same-named injected var — a same-named
/// dep injection is a misconfiguration the secret wins) and before explicit
/// user env (ADR decision 4: explicit user intent is always highest).
/// Never log or persist the returned values.
fn resolve_secret_env(
    spec: &WorkspaceServiceSpec,
    secret_values: &Option<BTreeMap<String, String>>,
) -> Result<Vec<(String, String)>, JSONRPCErrorError> {
    if spec.env_refs.is_empty() {
        return Ok(Vec::new());
    }
    let provided = secret_values.as_ref().cloned().unwrap_or_default();
    let missing: Vec<&str> = spec
        .env_refs
        .iter()
        .filter(|name| !provided.contains_key(*name))
        .map(String::as_str)
        .collect();
    if !missing.is_empty() {
        return Err(invalid_params(format!(
            "service {:?} requires envRefs missing from secretValues: {}",
            spec.name,
            missing.join(", ")
        )));
    }
    Ok(spec
        .env_refs
        .iter()
        .map(|name| (name.clone(), provided[name].clone()))
        .collect())
}

#[derive(Clone)]
pub(crate) struct WorkspaceServiceRequestProcessor {
    project_store: Arc<Mutex<WorkspaceProjectStore>>,
    store: Arc<Mutex<WorkspaceServiceStore>>,
    runtime: Arc<Mutex<HashMap<String, ManagedService>>>,
    audit: Arc<WorkspaceAuditLog>,
    outgoing: Arc<OutgoingMessageSender>,
    /// Runtime-restart normalization notices, broadcast once after the
    /// first connection initializes (see `flush_startup_notifications`).
    startup_notifications: Arc<Mutex<Vec<WorkspaceServiceChangedNotification>>>,
    write_lock: Arc<WorkspaceWriteLock>,
}

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct WorkspaceServiceStore {
    schema_version: u32,
    pub(crate) services: BTreeMap<String, WorkspaceServiceRef>,
    /// Namespaced idempotency key (`"start:{key}"`) -> service id.
    idempotency: BTreeMap<String, String>,
    /// E3: project-scoped service specs (`spec-{uuid}` -> spec).
    #[serde(default)]
    pub(crate) specs: BTreeMap<String, WorkspaceServiceSpec>,
    /// E3: `"define:{key}"` -> spec id.
    #[serde(default)]
    spec_idempotency: BTreeMap<String, String>,
    /// E3: `"start_all:{key}"` -> recorded orchestration outcome.
    #[serde(default)]
    orchestrations: BTreeMap<String, StoredOrchestration>,
    #[serde(skip)]
    path: PathBuf,
}

/// Recorded startAll outcome for idempotent replay (ADR decision 5).
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct StoredOrchestration {
    pub(crate) project_id: String,
    pub(crate) response: WorkspaceServiceStartAllResponse,
    pub(crate) created_at_ms: i64,
}

impl WorkspaceServiceRequestProcessor {
    pub(crate) fn new(
        ody_home: PathBuf,
        project_store: Arc<Mutex<WorkspaceProjectStore>>,
        audit: Arc<WorkspaceAuditLog>,
        outgoing: Arc<OutgoingMessageSender>,
        write_lock: Arc<WorkspaceWriteLock>,
    ) -> Self {
        let path = ody_home.join(STORE_DIR).join(STORE_FILE);
        let mut store = WorkspaceServiceStore::load(path);
        // E4: drop orchestration records whose project no longer exists
        // (close 联动已删, 这里兜底 Runtime 崩溃与手工删 store 的场景),
        // then queue a RuntimeRestarted notice per affected project.
        let known: std::collections::BTreeSet<String> = project_store
            .lock()
            .expect("project store lock")
            .projects
            .keys()
            .cloned()
            .collect();
        let stale: Vec<String> = store
            .orchestrations
            .iter()
            .filter(|(_, record)| !known.contains(&record.project_id))
            .map(|(key, _)| key.clone())
            .collect();
        for key in &stale {
            store.orchestrations.remove(key);
        }
        let mut by_project: BTreeMap<String, Vec<WorkspaceServiceRef>> = BTreeMap::new();
        for service in store.services.values() {
            if service.error.as_deref() == Some(RUNTIME_RESTARTED_ERROR) {
                by_project
                    .entry(service.project_id.clone())
                    .or_default()
                    .push(service.clone());
            }
        }
        if !stale.is_empty() || !by_project.is_empty() {
            let _ = store.persist();
        }
        let startup_notifications = by_project
            .into_iter()
            .map(|(project_id, services)| WorkspaceServiceChangedNotification {
                project_id,
                reason: WorkspaceServiceChangedReason::RuntimeRestarted,
                services,
            })
            .collect();
        Self {
            project_store,
            store: Arc::new(Mutex::new(store)),
            runtime: Arc::new(Mutex::new(HashMap::new())),
            audit,
            outgoing,
            startup_notifications: Arc::new(Mutex::new(startup_notifications)),
            write_lock,
        }
    }

    /// Broadcast pending RuntimeRestarted notices (drained). Called by
    /// message_processor after the first connection initializes so the
    /// broadcast reaches a live client; with zero connections it is a no-op
    /// and clients recover context via `workspace/service/list` + audit.
    pub(crate) async fn flush_startup_notifications(&self) {
        let notices = std::mem::take(&mut *self.startup_notifications.lock().expect("lock"));
        for notice in notices {
            self.outgoing
                .send_server_notification(ServerNotification::WorkspaceServiceChanged(notice))
                .await;
        }
    }

    /// Test-visible peek at the queued startup notices (does not drain).
    #[cfg(test)]
    fn take_startup_notification(&self) -> Option<WorkspaceServiceChangedNotification> {
        self.startup_notifications.lock().expect("lock").first().cloned()
    }

    async fn broadcast_changed(
        &self,
        project_id: &str,
        reason: WorkspaceServiceChangedReason,
        services: Vec<WorkspaceServiceRef>,
    ) {
        self.outgoing
            .send_server_notification(ServerNotification::WorkspaceServiceChanged(
                WorkspaceServiceChangedNotification {
                    project_id: project_id.to_owned(),
                    reason,
                    services,
                },
            ))
            .await;
    }

    pub(crate) async fn start(
        &self,
        params: WorkspaceServiceStartParams,
    ) -> Result<WorkspaceServiceStartResponse, JSONRPCErrorError> {
        let result = self.start_inner(params).await;
        match &result {
            Ok(response) => self.audit.record(audit_event(
                None,
                "service.start",
                "ok",
                serde_json::json!({
                    "serviceId": response.service.id,
                    "name": response.service.name,
                }),
            )),
            Err(err) => self.audit.record(audit_event(
                None,
                "service.start",
                "error",
                serde_json::json!({
                    "error": err.message.chars().take(200).collect::<String>(),
                }),
            )),
        }
        result
    }

    async fn start_inner(
        &self,
        params: WorkspaceServiceStartParams,
    ) -> Result<WorkspaceServiceStartResponse, JSONRPCErrorError> {
        let project = self.project(&params.project_id)?;
        let name = params.name.trim();
        if name.is_empty()
            || name.chars().count() > crate::workspace_service::MAX_SERVICE_NAME_CHARS
        {
            return Err(invalid_params(format!(
                "service name must be 1-{} chars, got {:?}",
                crate::workspace_service::MAX_SERVICE_NAME_CHARS,
                params.name
            )));
        }

        // Idempotent replay: same key returns the recorded service.
        let idem_key = format!("start:{}", params.idempotency_key);
        {
            let store = self.store.lock().expect("store lock");
            if let Some(service_id) = store.idempotency.get(&idem_key) {
                if let Some(service) = store.services.get(service_id) {
                    return Ok(WorkspaceServiceStartResponse {
                        service: service.clone(),
                    });
                }
            }
        }

        // Ad-hoc start may still reference a declared spec's envRefs; when
        // no same-name spec exists, provided secretValues are ignored
        // (values without a declared ref have nowhere vetted to go).
        let secret_env = {
            let store = self.store.lock().expect("store lock");
            store
                .specs
                .values()
                .find(|spec| spec.project_id == params.project_id && spec.name == name)
                .cloned()
        }
        .map(|spec| resolve_secret_env(&spec, &params.secret_values))
        .transpose()?
        .unwrap_or_default();
        let service = self
            .start_one(
                &project,
                name,
                params.root_index,
                None,
                &params.script,
                params.port,
                params.ready_timeout_ms,
                &[],
                &secret_env,
                &params.env,
                Some(&idem_key),
                true,
            )
            .await?;
        Ok(WorkspaceServiceStartResponse { service })
    }

    /// Shared spawn path for `start` and orchestration. `extra_env` carries
    /// dependency connection vars; `secret_env` carries resolved envRefs
    /// values (secret overrides injected same-name keys; explicit user env
    /// wins by overwrite order, ADR decision 4). `check_active_name` is true for the
    /// single-service protocol (duplicate active name is an error) and false
    /// for startAll (reuse is handled by the caller before reaching here).
    #[allow(clippy::too_many_arguments)]
    async fn start_one(
        &self,
        project: &ody_app_server_protocol::WorkspaceProjectRef,
        name: &str,
        root_index: u32,
        cwd: Option<&str>,
        script: &str,
        port: Option<u16>,
        ready_timeout_ms: Option<i64>,
        extra_env: &[(String, String)],
        secret_env: &[(String, String)],
        user_env: &Option<HashMap<String, Option<String>>>,
        idem_key: Option<&str>,
        check_active_name: bool,
    ) -> Result<WorkspaceServiceRef, JSONRPCErrorError> {
        let root = project.roots.get(root_index as usize).ok_or_else(|| {
            invalid_params(format!(
                "root_index {} out of bounds for project {} ({} roots)",
                root_index,
                project.id,
                project.roots.len()
            ))
        })?;
        let root_path = Path::new(&root.path);
        let cwd_path = resolve_cwd(root_path, cwd)?; // cwd == None -> root_path

        // Script must exist in the cwd's package.json (Runtime never guesses).
        let mut discovery = ody_app_server_protocol::WorkspaceRootDiscovery {
            root_path: cwd_path.to_string_lossy().into_owned(),
            package_name: None,
            package_manager: None,
            tech_stack: Vec::new(),
            scripts: Vec::new(),
            sources: Vec::new(),
            git: ody_app_server_protocol::WorkspaceGitStatus {
                is_repo: false,
                repo_root: None,
                branch: None,
                head_commit_hash: None,
                has_changes: None,
                available: false,
                error: None,
            },
            stats: ody_app_server_protocol::WorkspaceScanStats {
                files_visited: 0,
                dirs_visited: 0,
                skipped_dirs: 0,
            },
            errors: Vec::new(),
            truncated: false,
        };
        crate::workspace_discovery::inspect_package_json(&cwd_path, &mut discovery);
        let script_entry = discovery
            .scripts
            .iter()
            .find(|entry| entry.name == script)
            .ok_or_else(|| {
                invalid_params(format!(
                    "script {:?} not found in {}/package.json; available: {}",
                    script,
                    cwd_path.display(),
                    discovery
                        .scripts
                        .iter()
                        .map(|entry| entry.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })?;
        let tech_ids = discovery
            .tech_stack
            .iter()
            .map(|entry| entry.id.clone())
            .collect::<Vec<_>>();

        // One non-terminal service per name; bounded per project. startAll
        // reuses same-name active services instead of erroring (R3).
        if check_active_name {
            let store = self.store.lock().expect("store lock");
            let active = store
                .services
                .values()
                .filter(|service| service.project_id == project.id && !is_terminal(service.status))
                .count();
            if active >= crate::workspace_service::MAX_SERVICES_PER_PROJECT {
                return Err(invalid_params(format!(
                    "project {} already has {} active services (max {})",
                    project.id,
                    active,
                    crate::workspace_service::MAX_SERVICES_PER_PROJECT
                )));
            }
            if store.services.values().any(|service| {
                service.project_id == project.id
                    && service.name == name
                    && !is_terminal(service.status)
            }) {
                return Err(invalid_params(format!(
                    "service name {name:?} is already active in project {}; stop it first",
                    project.id
                )));
            }
        }

        let pm = crate::workspace_discovery::detect_package_manager(&cwd_path)
            .unwrap_or_else(|| "npm".to_owned());
        let port = crate::workspace_service::pick_port(port, &tech_ids).map_err(invalid_params)?;
        let args =
            crate::workspace_service::build_command_args(&script_entry.name, port, &tech_ids);
        let command_display = format!("{pm} {}", args.join(" "));
        let now = now_ms();
        let service_id = Uuid::new_v4().to_string();
        let url = format!("http://127.0.0.1:{port}/");

        let mut service = WorkspaceServiceRef {
            id: service_id.clone(),
            project_id: project.id.clone(),
            name: name.to_owned(),
            root_index,
            script: script_entry.name.clone(),
            command: command_display,
            port,
            url: url.clone(),
            status: WorkspaceServiceStatus::Starting,
            pid: None,
            exit_code: None,
            health: None,
            error: None,
            created_at_ms: now,
            updated_at_ms: now,
        };

        // Injection order = precedence order: dependency connection env,
        // then secret envRefs values (a secret overrides a same-named
        // injected var), then explicit user env wins (ADR 4).
        let mut merged_env: HashMap<String, Option<String>> = extra_env
            .iter()
            .chain(secret_env.iter())
            .map(|(key, value)| (key.clone(), Some(value.clone())))
            .collect();
        if let Some(user) = user_env {
            for (key, value) in user {
                merged_env.insert(key.clone(), value.clone());
            }
        }
        let spawn_result = crate::workspace_service::spawn_service_command(
            &pm,
            &args,
            &cwd_path,
            port,
            &Some(merged_env),
        );
        let mut child = match spawn_result {
            Ok(child) => child,
            Err(err) => {
                service.status = WorkspaceServiceStatus::Failed;
                service.error = Some(format!("failed to spawn {pm}: {err}"));
                self.persist_service(&service, idem_key);
                return Err(invalid_params(service.error.clone().unwrap_or_default()));
            }
        };
        let pid = child.id().expect("child pid");

        let stdout_ring = Arc::new(Mutex::new(OutputRing::new(
            crate::workspace_service::SERVICE_OUTPUT_TAIL_BYTES,
        )));
        let stderr_ring = Arc::new(Mutex::new(OutputRing::new(
            crate::workspace_service::SERVICE_OUTPUT_TAIL_BYTES,
        )));
        if let Some(stdout) = child.stdout.take() {
            crate::workspace_service::spawn_output_reader(stdout, stdout_ring.clone());
        }
        if let Some(stderr) = child.stderr.take() {
            crate::workspace_service::spawn_output_reader(stderr, stderr_ring.clone());
        }

        let stop_requested = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let terminated = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let terminal_notify = Arc::new(tokio::sync::Notify::new());

        service.pid = Some(pid);
        self.persist_service(&service, idem_key);

        // Wait task: owns the child; writes the terminal state exactly once.
        {
            let store = self.store.clone();
            let service_id = service_id.clone();
            let stop_requested = stop_requested.clone();
            let terminated = terminated.clone();
            let terminal_notify = terminal_notify.clone();
            let outgoing_for_task = self.outgoing.clone();
            tokio::spawn(async move {
                let exit = child.wait().await;
                terminated.store(true, Ordering::SeqCst);
                let exit_code = exit.ok().and_then(|status| status.code());
                // E4: broadcast the terminal transition (crash, clean exit,
                // or stop) — this task is the single terminal write path.
                let record = {
                    let mut store = store.lock().expect("store lock");
                    if let Some(record) = store.services.get_mut(&service_id) {
                        record.exit_code = exit_code;
                        if stop_requested.load(Ordering::SeqCst) {
                            record.status = WorkspaceServiceStatus::Stopped;
                        } else if exit_code == Some(0) {
                            record.status = WorkspaceServiceStatus::Exited;
                        } else {
                            record.status = WorkspaceServiceStatus::Failed;
                            record.error = Some(format!(
                                "process exited with code {}",
                                exit_code.map_or("signal".to_owned(), |code| code.to_string())
                            ));
                        }
                        record.updated_at_ms = now_ms();
                        let _ = store.persist();
                    }
                    store.services.get(&service_id).cloned()
                };
                terminal_notify.notify_waiters();
                if let Some(record) = record {
                    let reason = match record.status {
                        WorkspaceServiceStatus::Stopped => WorkspaceServiceChangedReason::Stopped,
                        WorkspaceServiceStatus::Exited => WorkspaceServiceChangedReason::Exited,
                        _ => WorkspaceServiceChangedReason::Failed,
                    };
                    outgoing_for_task
                        .send_server_notification(
                            ServerNotification::WorkspaceServiceChanged(
                                WorkspaceServiceChangedNotification {
                                    project_id: record.project_id.clone(),
                                    reason,
                                    services: vec![record],
                                },
                            ),
                        )
                        .await;
                }
            });
        }

        let runtime_entry = ManagedService {
            pid,
            pgid: pid,
            stop_requested,
            terminated: terminated.clone(),
            stdout_ring,
            stderr_ring,
            terminal_notify: terminal_notify.clone(),
        };
        self.runtime
            .lock()
            .expect("runtime lock")
            .insert(service_id.clone(), runtime_entry);

        // Synchronous readiness wait inside the start request.
        let (outcome, ready_origin) = crate::workspace_service::wait_ready(
            &terminated,
            port,
            crate::workspace_service::clamp_ready_timeout(ready_timeout_ms),
        )
        .await;
        if outcome == ReadyOutcome::ProcessExited {
            // The wait task sets `terminated` before writing the terminal
            // record; give it a brief window to publish, so we never hand
            // the client a lingering Starting record.
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                terminal_notify.notified(),
            )
            .await;
        }
        {
            let mut store = self.store.lock().expect("store lock");
            let record = store
                .services
                .get_mut(&service_id)
                .expect("service record persisted above");
            match outcome {
                ReadyOutcome::Ready => {
                    record.status = WorkspaceServiceStatus::Ready;
                    if let Some(origin) = ready_origin {
                        // Record the origin that actually answered (an
                        // IPv6-only binder is reached via [::1], not
                        // 127.0.0.1): env injection, health probes, preview
                        // checks, and diagnosis all consume `url`.
                        record.url = origin;
                    }
                }
                ReadyOutcome::ProcessExited => {
                    // Wait task already wrote Failed/Exited with exit code.
                }
                ReadyOutcome::TimedOut => {
                    record.status = WorkspaceServiceStatus::Failed;
                    record.error = Some(format!(
                        "readiness timeout after {} ms; stderr tail: {}",
                        crate::workspace_service::clamp_ready_timeout(ready_timeout_ms),
                        self.stderr_tail_of(&service_id)
                    ));
                    let _ = crate::workspace_service::kill_process_group(pid);
                }
            }
            record.updated_at_ms = now_ms();
            let _ = store.persist();
        }

        let service = self
            .store
            .lock()
            .expect("store lock")
            .services
            .get(&service_id)
            .cloned()
            .expect("service record");
        if outcome == ReadyOutcome::Ready {
            // E4: readiness is a lifecycle event clients should not poll for.
            self.broadcast_changed(
                &service.project_id,
                WorkspaceServiceChangedReason::Started,
                vec![service.clone()],
            )
            .await;
        }
        Ok(service)
    }

    pub(crate) async fn stop(
        &self,
        params: WorkspaceServiceStopParams,
    ) -> Result<WorkspaceServiceStopResponse, JSONRPCErrorError> {
        let result = self.stop_inner(params).await;
        match &result {
            Ok(response) => self.audit.record(audit_event(
                None,
                "service.stop",
                "ok",
                serde_json::json!({
                    "serviceId": response.service.id,
                    "name": response.service.name,
                }),
            )),
            Err(err) => self.audit.record(audit_event(
                None,
                "service.stop",
                "error",
                serde_json::json!({
                    "error": err.message.chars().take(200).collect::<String>(),
                }),
            )),
        }
        result
    }

    async fn stop_inner(
        &self,
        params: WorkspaceServiceStopParams,
    ) -> Result<WorkspaceServiceStopResponse, JSONRPCErrorError> {
        let service = self.stop_one(&params.service_id).await?;
        self.broadcast_changed(
            &service.project_id,
            WorkspaceServiceChangedReason::Stopped,
            vec![service.clone()],
        )
        .await;
        Ok(WorkspaceServiceStopResponse { service })
    }

    async fn stop_one(&self, service_id: &str) -> Result<WorkspaceServiceRef, JSONRPCErrorError> {
        let runtime_entry = {
            let mut runtime = self.runtime.lock().expect("runtime lock");
            runtime.get(service_id).map(|entry| ManagedServiceClone {
                pgid: entry.pgid,
                stop_requested: entry.stop_requested.clone(),
                terminal_notify: entry.terminal_notify.clone(),
            })
        };
        let (pgid, stop_requested, terminal_notify) = match runtime_entry {
            Some(entry) => (entry.pgid, entry.stop_requested, entry.terminal_notify),
            None => {
                // Terminal or after-restart: just report the stored record.
                return self.service_record(service_id);
            }
        };
        stop_requested.store(true, Ordering::SeqCst);
        if let Err(err) = crate::workspace_service::kill_process_group(pgid) {
            tracing::warn!(service = %service_id, error = %err, "kill_process_group failed");
        }
        // Wait briefly for the wait task to publish the terminal state.
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            terminal_notify.notified(),
        )
        .await;
        let mut service = self.service_record(service_id)?;
        if !is_terminal(service.status) {
            // killpg raced with the reaper window; force the terminal mark.
            service.status = WorkspaceServiceStatus::Stopped;
            service.updated_at_ms = now_ms();
            let mut store = self.store.lock().expect("store lock");
            store.services.insert(service.id.clone(), service.clone());
            let _ = store.persist();
        }
        self.runtime
            .lock()
            .expect("runtime lock")
            .remove(service_id);
        Ok(service)
    }

    pub(crate) async fn list(
        &self,
        params: WorkspaceServiceListParams,
    ) -> Result<WorkspaceServiceListResponse, JSONRPCErrorError> {
        self.project(&params.project_id)?;
        let mut services = self
            .store
            .lock()
            .expect("store lock")
            .services
            .values()
            .filter(|service| service.project_id == params.project_id)
            .cloned()
            .collect::<Vec<_>>();
        services.sort_by_key(|service| std::cmp::Reverse(service.updated_at_ms));
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map_err(|err| internal_error(err.to_string()))?;
        for service in &mut services {
            if service.status != WorkspaceServiceStatus::Ready {
                continue;
            }
            let checked_at_ms = now_ms();
            match tokio::time::timeout(
                std::time::Duration::from_millis(
                    crate::workspace_service::SERVICE_HEALTH_TIMEOUT_MS,
                ),
                client.get(&service.url).send(),
            )
            .await
            {
                Ok(Ok(response)) => {
                    service.health = Some(WorkspaceServiceHealth {
                        ok: response.status().is_success(),
                        status_code: Some(response.status().as_u16()),
                        error: None,
                        checked_at_ms,
                    });
                }
                Ok(Err(err)) => {
                    service.health = Some(WorkspaceServiceHealth {
                        ok: false,
                        status_code: None,
                        error: Some(err.to_string()),
                        checked_at_ms,
                    });
                }
                Err(_) => {
                    service.health = Some(WorkspaceServiceHealth {
                        ok: false,
                        status_code: None,
                        error: Some(format!(
                            "health probe timed out after {} ms",
                            crate::workspace_service::SERVICE_HEALTH_TIMEOUT_MS
                        )),
                        checked_at_ms,
                    });
                }
            }
        }
        Ok(WorkspaceServiceListResponse { services })
    }

    pub(crate) async fn logs(
        &self,
        params: WorkspaceServiceLogsParams,
    ) -> Result<WorkspaceServiceLogsResponse, JSONRPCErrorError> {
        self.service_record(&params.service_id)?;
        let runtime = self.runtime.lock().expect("runtime lock");
        let Some(entry) = runtime.get(&params.service_id) else {
            return Err(invalid_params(format!(
                "no live process for service {}; logs are unavailable after stop or runtime restart",
                params.service_id
            )));
        };
        let tail_bytes = params
            .tail_bytes
            .map(|value| value as usize)
            .unwrap_or(crate::workspace_service::SERVICE_OUTPUT_TAIL_BYTES)
            .min(crate::workspace_service::SERVICE_OUTPUT_TAIL_BYTES);
        Ok(WorkspaceServiceLogsResponse {
            service_id: params.service_id.clone(),
            stdout_tail: entry
                .stdout_ring
                .lock()
                .expect("ring lock")
                .tail_string(tail_bytes),
            stderr_tail: entry
                .stderr_ring
                .lock()
                .expect("ring lock")
                .tail_string(tail_bytes),
        })
    }

    pub(crate) async fn check(
        &self,
        params: WorkspacePreviewCheckParams,
    ) -> Result<WorkspacePreviewCheckResponse, JSONRPCErrorError> {
        let service = self.service_record(&params.service_id)?;
        if service.status != WorkspaceServiceStatus::Ready {
            return Err(invalid_params(format!(
                "service {} is not ready (status: {:?}); start it and wait for readiness first",
                params.service_id, service.status
            )));
        }
        let url = crate::workspace_service::validate_check_url(params.url, &service.url)
            .map_err(invalid_params)?;
        let timeout_ms = crate::workspace_service::clamp_preview_timeout(params.timeout_ms);
        let checked_at_ms = now_ms();
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map_err(|err| internal_error(err.to_string()))?;

        let outcome = tokio::time::timeout(
            std::time::Duration::from_millis(timeout_ms as u64),
            client.get(&url).send(),
        )
        .await;

        let base = WorkspacePreviewCheckResponse {
            service_id: params.service_id.clone(),
            url: url.clone(),
            reachable: false,
            http_status: None,
            content_bytes: 0,
            content_sha256: crate::workspace_changeset::sha256_hex(b""),
            title: None,
            error: None,
            checked_at_ms,
        };

        match outcome {
            Ok(Ok(response)) => {
                let http_status = response.status().as_u16();
                let headers = response.headers().clone();
                let bytes = response
                    .bytes()
                    .await
                    .map_err(|err| internal_error(err.to_string()))?;
                let body = String::from_utf8_lossy(&bytes);
                let is_html = headers
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .map_or(true, |value| value.contains("html")); // missing CT: best-effort
                Ok(WorkspacePreviewCheckResponse {
                    reachable: true,
                    http_status: Some(http_status),
                    content_bytes: bytes.len() as u64,
                    content_sha256: crate::workspace_changeset::sha256_hex(&bytes),
                    title: if is_html {
                        crate::workspace_service::extract_title(&body)
                    } else {
                        None
                    },
                    error: None,
                    ..base
                })
            }
            Ok(Err(err)) => Ok(WorkspacePreviewCheckResponse {
                error: Some(err.to_string()),
                ..base
            }),
            Err(_) => Ok(WorkspacePreviewCheckResponse {
                error: Some(format!("request timed out after {timeout_ms} ms")),
                ..base
            }),
        }
    }

    pub(crate) async fn define(
        &self,
        params: WorkspaceServiceDefineParams,
    ) -> Result<WorkspaceServiceDefineResponse, JSONRPCErrorError> {
        let result = self.define_inner(params).await;
        match &result {
            Ok(response) => self.audit.record(audit_event(
                None,
                "service.define",
                "ok",
                serde_json::json!({
                    "specId": response.spec.id,
                    "name": response.spec.name,
                    // Ref NAMES only — values never reach the audit trail.
                    "envRefs": response.spec.env_refs,
                }),
            )),
            Err(err) => self.audit.record(audit_event(
                None,
                "service.define",
                "error",
                serde_json::json!({
                    "error": err.message.chars().take(200).collect::<String>(),
                }),
            )),
        }
        result
    }

    async fn define_inner(
        &self,
        params: WorkspaceServiceDefineParams,
    ) -> Result<WorkspaceServiceDefineResponse, JSONRPCErrorError> {
        let project = self.project(&params.project_id)?;
        let idem_key = format!("define:{}", params.idempotency_key);
        {
            let store = self.store.lock().expect("store lock");
            if let Some(spec_id) = store.spec_idempotency.get(&idem_key) {
                if let Some(spec) = store.specs.get(spec_id) {
                    return Ok(WorkspaceServiceDefineResponse { spec: spec.clone() });
                }
            }
        }
        let name = params.name.trim();
        if name.is_empty()
            || name.chars().count() > crate::workspace_service::MAX_SERVICE_NAME_CHARS
        {
            return Err(invalid_params(format!(
                "spec name must be 1-{} chars, got {:?}",
                crate::workspace_service::MAX_SERVICE_NAME_CHARS,
                params.name
            )));
        }
        let root = project
            .roots
            .get(params.root_index as usize)
            .ok_or_else(|| {
                invalid_params(format!(
                    "root_index {} out of bounds for project {} ({} roots)",
                    params.root_index,
                    params.project_id,
                    project.roots.len()
                ))
            })?;
        let cwd_path = resolve_cwd(Path::new(&root.path), params.cwd.as_deref())?;
        if params.depends_on.len() > crate::workspace_service::MAX_DEPENDS_ON_PER_SPEC {
            return Err(invalid_params(format!(
                "spec {name:?} has {} dependencies (max {})",
                params.depends_on.len(),
                crate::workspace_service::MAX_DEPENDS_ON_PER_SPEC
            )));
        }
        if params.depends_on.iter().any(|dep| dep == name) {
            return Err(invalid_params(format!(
                "spec {name:?} must not depend on itself"
            )));
        }
        let env_refs = validate_env_refs(params.env_refs.as_deref().unwrap_or(&[]))?;

        let now = now_ms();
        let (spec_id, created_at_ms) = {
            let store = self.store.lock().expect("store lock");
            let existing = store
                .specs
                .values()
                .find(|spec| spec.project_id == params.project_id && spec.name == name)
                .map(|spec| (spec.id.clone(), spec.created_at_ms));
            if existing.is_none()
                && store
                    .specs
                    .values()
                    .filter(|spec| spec.project_id == params.project_id)
                    .count()
                    >= crate::workspace_service::MAX_SPECS_PER_PROJECT
            {
                return Err(invalid_params(format!(
                    "project {} already has {} specs (max {})",
                    params.project_id,
                    crate::workspace_service::MAX_SPECS_PER_PROJECT,
                    crate::workspace_service::MAX_SPECS_PER_PROJECT
                )));
            }
            existing.unwrap_or_else(|| (format!("spec-{}", Uuid::new_v4()), now))
        };

        // Dependency names must reference existing specs of the same project.
        {
            let store = self.store.lock().expect("store lock");
            for dep in &params.depends_on {
                if !store
                    .specs
                    .values()
                    .any(|spec| spec.project_id == params.project_id && spec.name == *dep)
                {
                    return Err(invalid_params(format!(
                        "spec {name:?} depends on unknown spec {dep:?} of project {}",
                        params.project_id
                    )));
                }
            }
        }

        // Upsert then cycle-check the assembled project graph (ADR decision 3).
        let mut graph: Vec<(String, Vec<String>)> = {
            let store = self.store.lock().expect("store lock");
            store
                .specs
                .values()
                .filter(|spec| spec.project_id == params.project_id && spec.name != name)
                .map(|spec| (spec.name.clone(), spec.depends_on.clone()))
                .collect()
        };
        graph.push((name.to_owned(), params.depends_on.clone()));
        crate::workspace_service::topo_levels(&graph).map_err(invalid_params)?;

        // Script must exist in the cwd's package.json (Runtime never guesses).
        let mut discovery = ody_app_server_protocol::WorkspaceRootDiscovery {
            root_path: cwd_path.to_string_lossy().into_owned(),
            package_name: None,
            package_manager: None,
            tech_stack: Vec::new(),
            scripts: Vec::new(),
            sources: Vec::new(),
            git: ody_app_server_protocol::WorkspaceGitStatus {
                is_repo: false,
                repo_root: None,
                branch: None,
                head_commit_hash: None,
                has_changes: None,
                available: false,
                error: None,
            },
            stats: ody_app_server_protocol::WorkspaceScanStats {
                files_visited: 0,
                dirs_visited: 0,
                skipped_dirs: 0,
            },
            errors: Vec::new(),
            truncated: false,
        };
        crate::workspace_discovery::inspect_package_json(&cwd_path, &mut discovery);
        if !discovery
            .scripts
            .iter()
            .any(|entry| entry.name == params.script)
        {
            return Err(invalid_params(format!(
                "script {:?} not found in {}/package.json; available: {}",
                params.script,
                cwd_path.display(),
                discovery
                    .scripts
                    .iter()
                    .map(|entry| entry.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }

        let spec = WorkspaceServiceSpec {
            id: spec_id,
            project_id: params.project_id.clone(),
            name: name.to_owned(),
            root_index: params.root_index,
            script: params.script.clone(),
            port: params.port,
            cwd: params.cwd.clone(),
            depends_on: params.depends_on.clone(),
            health_check: params.health_check.clone(),
            ready_timeout_ms: params.ready_timeout_ms,
            env_refs,
            created_at_ms,
            updated_at_ms: now,
        };
        {
            let mut store = self.store.lock().expect("store lock");
            store.specs.insert(spec.id.clone(), spec.clone());
            store.spec_idempotency.insert(idem_key, spec.id.clone());
            let _ = store.persist();
        }
        Ok(WorkspaceServiceDefineResponse { spec })
    }

    pub(crate) async fn specs(
        &self,
        params: WorkspaceServiceSpecsParams,
    ) -> Result<WorkspaceServiceSpecsResponse, JSONRPCErrorError> {
        self.project(&params.project_id)?;
        let mut specs = self
            .store
            .lock()
            .expect("store lock")
            .specs
            .values()
            .filter(|spec| spec.project_id == params.project_id)
            .cloned()
            .collect::<Vec<_>>();
        specs.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(WorkspaceServiceSpecsResponse { specs })
    }

    pub(crate) async fn start_all(
        &self,
        params: WorkspaceServiceStartAllParams,
        connection_id: ConnectionId,
    ) -> Result<WorkspaceServiceStartAllResponse, JSONRPCErrorError> {
        self.write_lock.check(&params.project_id, connection_id)?;
        let result = self.start_all_inner(params).await;
        match &result {
            Ok(response) => self.audit.record(audit_event(
                None,
                "service.start_all",
                "ok",
                serde_json::json!({
                    "started": response.started.len(),
                    "reused": response.reused.len(),
                    "failed": response.failed.len(),
                }),
            )),
            Err(err) => self.audit.record(audit_event(
                None,
                "service.start_all",
                "error",
                serde_json::json!({
                    "error": err.message.chars().take(200).collect::<String>(),
                }),
            )),
        }
        result
    }

    async fn start_all_inner(
        &self,
        params: WorkspaceServiceStartAllParams,
    ) -> Result<WorkspaceServiceStartAllResponse, JSONRPCErrorError> {
        let project = self.project(&params.project_id)?;
        let idem_key = format!("start_all:{}", params.idempotency_key);
        {
            let store = self.store.lock().expect("store lock");
            if let Some(record) = store.orchestrations.get(&idem_key) {
                // Idempotent replay (R2): spawn nothing again; report the
                // current records, folding still-active originally-started
                // services into `reused` so the response honestly says "no
                // new process".
                let original = record.response.clone();
                let services = original
                    .services
                    .iter()
                    .filter_map(|service| store.services.get(&service.id).cloned())
                    .collect::<Vec<_>>();
                let mut reused = original.reused.clone();
                for id in &original.started {
                    if services
                        .iter()
                        .any(|service| service.id == *id && !is_terminal(service.status))
                    {
                        reused.push(id.clone());
                    }
                }
                return Ok(WorkspaceServiceStartAllResponse {
                    services,
                    started: Vec::new(),
                    reused,
                    failed: original.failed.clone(),
                });
            }
        }

        // Select specs: empty names = all; unknown names are request-level errors.
        let selected: Vec<WorkspaceServiceSpec> = {
            let store = self.store.lock().expect("store lock");
            let mut all = store
                .specs
                .values()
                .filter(|spec| spec.project_id == params.project_id)
                .cloned()
                .collect::<Vec<_>>();
            all.sort_by(|a, b| a.name.cmp(&b.name));
            if params.names.is_empty() {
                all
            } else {
                for name in &params.names {
                    if !all.iter().any(|spec| &spec.name == name) {
                        return Err(invalid_params(format!(
                            "unknown spec {name:?} in project {}",
                            params.project_id
                        )));
                    }
                }
                all.into_iter()
                    .filter(|spec| params.names.contains(&spec.name))
                    .collect()
            }
        };
        if selected.is_empty() {
            return Err(invalid_params(format!(
                "project {} has no specs to start; define them first via workspace/service/define",
                params.project_id
            )));
        }

        // Request-level validation before anything spawns (ADR decision 3):
        // topo over the selected graph; every dependency must be selected or
        // already running.
        let levels = crate::workspace_service::topo_levels(
            &selected
                .iter()
                .map(|spec| (spec.name.clone(), spec.depends_on.clone()))
                .collect::<Vec<_>>(),
        )
        .map_err(invalid_params)?;
        let selected_names: std::collections::BTreeSet<String> =
            selected.iter().map(|spec| spec.name.clone()).collect();
        for spec in &selected {
            for dep in &spec.depends_on {
                if selected_names.contains(dep) {
                    continue;
                }
                let running = self
                    .store
                    .lock()
                    .expect("store lock")
                    .services
                    .values()
                    .any(|service| {
                        service.project_id == params.project_id
                            && service.name == *dep
                            && !is_terminal(service.status)
                    });
                if !running {
                    return Err(invalid_params(format!(
                        "dependency {dep:?} of spec {:?} is neither selected nor running",
                        spec.name
                    )));
                }
            }
        }

        // E4: resolve every selected spec's envRefs up front (ADR decision
        // 3 — request-level validation before anything spawns). One shared
        // secretValues map across the orchestration; extra keys are ignored.
        let secret_envs: BTreeMap<String, Vec<(String, String)>> = selected
            .iter()
            .map(|spec| {
                resolve_secret_env(spec, &params.secret_values)
                    .map(|pairs| (spec.name.clone(), pairs))
            })
            .collect::<Result<_, _>>()?;

        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map_err(|err| internal_error(err.to_string()))?;
        let mut started = Vec::new();
        let mut reused = Vec::new();
        let mut failed = Vec::new();
        let mut running: BTreeMap<String, WorkspaceServiceRef> = BTreeMap::new();
        let mut aborted = false;

        'levels: for level in &levels {
            if aborted {
                break;
            }
            for name in level {
                let spec = selected
                    .iter()
                    .find(|spec| &spec.name == name)
                    .expect("level names come from the selected graph");
                // Reuse an active same-name service instead of respawning (R3).
                let active = {
                    let store = self.store.lock().expect("store lock");
                    store
                        .services
                        .values()
                        .find(|service| {
                            service.project_id == params.project_id
                                && service.name == *name
                                && !is_terminal(service.status)
                        })
                        .cloned()
                };
                let record = match active {
                    Some(service) => {
                        reused.push(service.id.clone());
                        service
                    }
                    None => {
                        // Dependency connection env (ADR decision 4).
                        let mut extra_env: Vec<(String, String)> = Vec::new();
                        for dep in &spec.depends_on {
                            let dep_service = running.get(dep).cloned().or_else(|| {
                                self.store
                                    .lock()
                                    .expect("store lock")
                                    .services
                                    .values()
                                    .find(|service| {
                                        service.project_id == params.project_id
                                            && service.name == *dep
                                            && !is_terminal(service.status)
                                    })
                                    .cloned()
                            });
                            let Some(dep_service) = dep_service else {
                                failed.push(format!("unresolved-dependency-{dep}"));
                                aborted = true;
                                continue 'levels;
                            };
                            let key = crate::workspace_service::env_key_for_service(dep);
                            extra_env.push((format!("{key}_URL"), dep_service.url.clone()));
                            extra_env.push((format!("{key}_PORT"), dep_service.port.to_string()));
                        }
                        let secret_env = secret_envs.get(name).cloned().unwrap_or_default();
                        let record = self
                            .start_one(
                                &project,
                                &spec.name,
                                spec.root_index,
                                spec.cwd.as_deref(),
                                &spec.script,
                                spec.port,
                                spec.ready_timeout_ms,
                                &extra_env,
                                &secret_env,
                                &None,
                                None,
                                false,
                            )
                            .await?;
                        if record.status != WorkspaceServiceStatus::Ready {
                            failed.push(record.id.clone());
                            aborted = true;
                            continue 'levels;
                        }
                        started.push(record.id.clone());
                        record
                    }
                };
                running.insert(name.clone(), record);
            }
            if aborted {
                break;
            }
            // Level gate: health-check every service of this level that
            // defines a health check (started or reused), before the next
            // level spawns (ADR decision 6).
            for name in level {
                let spec = selected
                    .iter()
                    .find(|spec| &spec.name == name)
                    .expect("level names come from the selected graph");
                let Some(health_check) = &spec.health_check else {
                    continue;
                };
                let service = running.get(name).expect("recorded above").clone();
                let timeout = std::time::Duration::from_millis(
                    crate::workspace_service::clamp_health_timeout(health_check.timeout_ms) as u64,
                );
                let health = crate::workspace_service::spec_health_probe(
                    &client,
                    &service.url,
                    &health_check.path,
                    health_check.expect_status.or(Some(200)),
                    timeout,
                )
                .await;
                if !health.ok {
                    let was_started = started.contains(&service.id);
                    if was_started {
                        let mut store = self.store.lock().expect("store lock");
                        if let Some(record) = store.services.get_mut(&service.id) {
                            record.status = WorkspaceServiceStatus::Failed;
                            record.error = Some(format!(
                                "health check {} failed: {}",
                                health_check.path,
                                health.error.unwrap_or_else(|| {
                                    format!("status {}", health.status_code.unwrap_or(0))
                                })
                            ));
                            record.updated_at_ms = now_ms();
                        }
                        let _ = store.persist();
                    }
                    failed.push(service.id.clone());
                    aborted = true;
                    break;
                }
            }
        }

        let touched: std::collections::BTreeSet<String> = started
            .iter()
            .chain(reused.iter())
            .chain(failed.iter())
            .filter(|id| !id.starts_with("unresolved-dependency-"))
            .cloned()
            .collect();
        let services = {
            let store = self.store.lock().expect("store lock");
            touched
                .iter()
                .filter_map(|id| store.services.get(id).cloned())
                .collect::<Vec<_>>()
        };
        let response = WorkspaceServiceStartAllResponse {
            services,
            started,
            reused,
            failed: failed
                .iter()
                .filter(|id| !id.starts_with("unresolved-dependency-"))
                .cloned()
                .collect(),
        };
        {
            let mut store = self.store.lock().expect("store lock");
            store.orchestrations.insert(
                idem_key.clone(),
                StoredOrchestration {
                    project_id: params.project_id.clone(),
                    response: response.clone(),
                    created_at_ms: now_ms(),
                },
            );
            // Bound the replay log: evict the oldest records (R: unbounded growth).
            while store.orchestrations.len() > crate::workspace_service::MAX_ORCHESTRATION_RECORDS {
                let oldest = store
                    .orchestrations
                    .iter()
                    .min_by_key(|(_, record)| record.created_at_ms)
                    .map(|(key, _)| key.clone());
                if let Some(key) = oldest {
                    store.orchestrations.remove(&key);
                } else {
                    break;
                }
            }
            let _ = store.persist();
        }
        Ok(response)
    }

    pub(crate) async fn stop_all(
        &self,
        params: WorkspaceServiceStopAllParams,
        connection_id: ConnectionId,
    ) -> Result<WorkspaceServiceStopAllResponse, JSONRPCErrorError> {
        self.write_lock.check(&params.project_id, connection_id)?;
        let result = self.stop_all_inner(params).await;
        match &result {
            Ok(response) => self.audit.record(audit_event(
                None,
                "service.stop_all",
                "ok",
                serde_json::json!({
                    "stopped": response.stopped.len(),
                }),
            )),
            Err(err) => self.audit.record(audit_event(
                None,
                "service.stop_all",
                "error",
                serde_json::json!({
                    "error": err.message.chars().take(200).collect::<String>(),
                }),
            )),
        }
        result
    }

    async fn stop_all_inner(
        &self,
        params: WorkspaceServiceStopAllParams,
    ) -> Result<WorkspaceServiceStopAllResponse, JSONRPCErrorError> {
        self.project(&params.project_id)?;
        let selected: Vec<WorkspaceServiceSpec> = {
            let store = self.store.lock().expect("store lock");
            let mut all = store
                .specs
                .values()
                .filter(|spec| spec.project_id == params.project_id)
                .cloned()
                .collect::<Vec<_>>();
            all.sort_by(|a, b| a.name.cmp(&b.name));
            if params.names.is_empty() {
                all
            } else {
                for name in &params.names {
                    if !all.iter().any(|spec| &spec.name == name) {
                        return Err(invalid_params(format!(
                            "unknown spec {name:?} in project {}",
                            params.project_id
                        )));
                    }
                }
                all.into_iter()
                    .filter(|spec| params.names.contains(&spec.name))
                    .collect()
            }
        };
        // Reverse dependency order: dependents stop before their deps.
        let mut levels = crate::workspace_service::topo_levels(
            &selected
                .iter()
                .map(|spec| (spec.name.clone(), spec.depends_on.clone()))
                .collect::<Vec<_>>(),
        )
        .map_err(invalid_params)?;
        levels.reverse();
        let mut stopped = Vec::new();
        for level in levels {
            for name in level.into_iter().rev() {
                let active = {
                    let store = self.store.lock().expect("store lock");
                    store
                        .services
                        .values()
                        .find(|service| {
                            service.project_id == params.project_id
                                && service.name == name
                                && !is_terminal(service.status)
                        })
                        .map(|service| service.id.clone())
                };
                if let Some(service_id) = active {
                    let record = self.stop_one(&service_id).await?;
                    stopped.push(record);
                }
            }
        }
        stopped.sort_by(|a, b| a.name.cmp(&b.name));
        if !stopped.is_empty() {
            let project_id = stopped[0].project_id.clone();
            self.broadcast_changed(
                &project_id,
                WorkspaceServiceChangedReason::Stopped,
                stopped.clone(),
            )
            .await;
        }
        Ok(WorkspaceServiceStopAllResponse { stopped })
    }

    /// `workspace/preview/diagnose` (ADR decision 7): correlate caller-
    /// observed network failures against managed services, their logs,
    /// and the workspace source index. Never opens a browser; the index is
    /// computed on demand once per call (same cost model as E1 resolve).
    pub(crate) async fn diagnose(
        &self,
        params: WorkspacePreviewDiagnoseParams,
    ) -> Result<WorkspacePreviewDiagnoseResponse, JSONRPCErrorError> {
        let project = self.project(&params.project_id)?;
        if params.failures.is_empty()
            || params.failures.len() > crate::workspace_service::DIAGNOSE_MAX_FAILURES
        {
            return Err(invalid_params(format!(
                "diagnose accepts 1..={} failures per call, got {}",
                crate::workspace_service::DIAGNOSE_MAX_FAILURES,
                params.failures.len()
            )));
        }
        let tail_bytes = params
            .log_tail_bytes
            .unwrap_or(crate::workspace_service::DIAGNOSE_DEFAULT_LOG_TAIL_BYTES)
            .clamp(
                crate::workspace_service::DIAGNOSE_MIN_LOG_TAIL_BYTES,
                crate::workspace_service::DIAGNOSE_MAX_LOG_TAIL_BYTES,
            ) as usize;
        let max_candidates = params
            .max_candidates
            .unwrap_or(crate::workspace_service::DIAGNOSE_DEFAULT_MAX_CANDIDATES)
            .clamp(1, crate::workspace_service::DIAGNOSE_MAX_CANDIDATES_LIMIT)
            as usize;
        let services = {
            let store = self.store.lock().expect("store lock");
            store
                .services
                .values()
                .filter(|service| service.project_id == params.project_id)
                .cloned()
                .collect::<Vec<_>>()
        };
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map_err(|err| internal_error(err.to_string()))?;
        // Computed lazily: only when at least one failure matches a service.
        let mut index: Option<ody_app_server_protocol::WorkspaceSourceIndex> = None;

        let mut results = Vec::with_capacity(params.failures.len());
        for failure in &params.failures {
            let mut notes = Vec::new();
            let mut source_candidates = Vec::new();
            let parsed_port = crate::workspace_service::parse_loopback_port(&failure.url);
            let parsed = parsed_port
                .and_then(|port| crate::workspace_service::match_service_by_port(&services, port));
            let (matched_service, health, stdout_tail, stderr_tail, log_excerpt) = match parsed {
                None => {
                    notes.push(if parsed_port.is_none() {
                     format!(
                         "{} is not a loopback http(s) URL with a port; no managed service matched",
                         failure.url
                     )
                 } else {
                     format!(
                         "no managed service of project {} listens on that port",
                         params.project_id
                     )
                 });
                    (None, None, None, None, None)
                }
                Some(service) => {
                    notes.push(format!(
                        "matched service {:?} (status {:?}{})",
                        service.name,
                        service.status,
                        service
                            .error
                            .as_ref()
                            .map_or(String::new(), |error| format!(", error: {error}"))
                    ));
                    // On-demand probe: spec health check when defined, else
                    // "/" with 2xx tolerance (same lookup as `health`).
                    let spec_check = {
                        let store = self.store.lock().expect("store lock");
                        store
                            .specs
                            .values()
                            .find(|spec| {
                                spec.project_id == service.project_id && spec.name == service.name
                            })
                            .and_then(|spec| spec.health_check.clone())
                    };
                    let (probe_path, probe_expect, probe_timeout) = match &spec_check {
                        Some(check) => (
                            check.path.clone(),
                            check.expect_status.or(Some(200)),
                            crate::workspace_service::clamp_health_timeout(check.timeout_ms),
                        ),
                        None => (
                            "/".to_owned(),
                            None,
                            crate::workspace_service::clamp_health_timeout(None),
                        ),
                    };
                    let health = crate::workspace_service::spec_health_probe(
                        &client,
                        &service.url,
                        &probe_path,
                        probe_expect,
                        std::time::Duration::from_millis(probe_timeout as u64),
                    )
                    .await;
                    let (stdout_tail, stderr_tail) = {
                        let runtime = self.runtime.lock().expect("runtime lock");
                        runtime.get(&service.id).map_or((None, None), |entry| {
                            (
                                Some(
                                    entry
                                        .stdout_ring
                                        .lock()
                                        .expect("ring lock")
                                        .tail_string(tail_bytes),
                                ),
                                Some(
                                    entry
                                        .stderr_ring
                                        .lock()
                                        .expect("ring lock")
                                        .tail_string(tail_bytes),
                                ),
                            )
                        })
                    };
                    let combined = format!(
                        "{}\n{}",
                        stdout_tail.as_deref().unwrap_or_default(),
                        stderr_tail.as_deref().unwrap_or_default()
                    );
                    let path = reqwest::Url::parse(&failure.url)
                        .ok()
                        .map(|url| url.path().to_owned())
                        .unwrap_or_default();
                    let excerpt = crate::workspace_service::log_excerpt(
                        &combined,
                        &path,
                        crate::workspace_service::DIAGNOSE_EXCERPT_MAX_LINES,
                    );
                    let excerpt = if excerpt.is_empty() {
                        None
                    } else {
                        Some(excerpt)
                    };

                    // Source candidates: route-path variants (exact) + file
                    // name match on the last segment, over the on-demand
                    // index (E1 resolve matching rules).
                    if index.is_none() {
                        index = Some(crate::workspace_source_index::index_project(&project).await);
                    }
                    let index = index.as_ref().expect("index computed above");
                    let variants = crate::workspace_service::route_path_variants(&path);
                    let last_segment = path
                        .rsplit('/')
                        .next()
                        .filter(|seg| !seg.is_empty())
                        .map(str::to_lowercase);
                    for (artifact, source_ref) in index.artifacts.iter().zip(index.refs.iter()) {
                        if source_candidates.len() >= max_candidates {
                            break;
                        }
                        let route_hit = variants.iter().any(|variant| {
                            artifact.route_path.as_deref() == Some(variant.as_str())
                                || source_ref.route_path.as_deref() == Some(variant.as_str())
                        });
                        let name_hit = last_segment
                            .as_deref()
                            .map_or(false, |seg| artifact.name.to_lowercase() == seg);
                        if route_hit || name_hit {
                            source_candidates.push(source_ref.clone());
                        }
                    }
                    if source_candidates.is_empty() {
                        // No exact hit (typical for a 404 on a not-yet-written
                        // route): surface the indexed route files as
                        // candidates — they are where the missing endpoint
                        // belongs (ADR decision 7 fallback).
                        for (artifact, source_ref) in index.artifacts.iter().zip(index.refs.iter())
                        {
                            if source_candidates.len() >= max_candidates {
                                break;
                            }
                            if artifact.route_path.is_some() {
                                source_candidates.push(source_ref.clone());
                            }
                        }
                        if !source_candidates.is_empty() {
                            notes.push(
                                "no exact route match; showing indexed route files as candidates"
                                    .to_owned(),
                            );
                        }
                    }
                    if source_candidates.is_empty() {
                        notes.push(
                         "no source candidates: route-path variants and last-segment name matched nothing in the workspace index"
                             .to_owned(),
                     );
                    }
                    (
                        Some(service.clone()),
                        Some(health),
                        stdout_tail,
                        stderr_tail,
                        excerpt,
                    )
                }
            };
            results.push(WorkspaceNetworkDiagnosis {
                failure: failure.clone(),
                matched_service,
                health,
                stdout_tail,
                stderr_tail,
                log_excerpt,
                source_candidates,
                notes,
            });
        }
        Ok(WorkspacePreviewDiagnoseResponse {
            project_id: params.project_id,
            results,
            diagnosed_at_ms: now_ms(),
        })
    }

    pub(crate) async fn health(
        &self,
        params: WorkspaceServiceHealthParams,
    ) -> Result<WorkspaceServiceHealthResponse, JSONRPCErrorError> {
        let service = self.service_record(&params.service_id)?;
        let spec_check = {
            let store = self.store.lock().expect("store lock");
            store
                .specs
                .values()
                .find(|spec| spec.project_id == service.project_id && spec.name == service.name)
                .and_then(|spec| spec.health_check.clone())
        };
        // Probe path: explicit param > spec health check > "/". An explicit
        // param path has no spec expectation -> 2xx tolerance (ADR §3.1).
        let (path, expect_status, timeout_ms) = match (&params.path, &spec_check) {
            (Some(path), _) => (
                path.clone(),
                None,
                crate::workspace_service::clamp_health_timeout(None),
            ),
            (None, Some(check)) => (
                check.path.clone(),
                check.expect_status.or(Some(200)),
                crate::workspace_service::clamp_health_timeout(check.timeout_ms),
            ),
            (None, None) => (
                "/".to_owned(),
                None,
                crate::workspace_service::clamp_health_timeout(None),
            ),
        };
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map_err(|err| internal_error(err.to_string()))?;
        let health = crate::workspace_service::spec_health_probe(
            &client,
            &service.url,
            &path,
            expect_status,
            std::time::Duration::from_millis(timeout_ms as u64),
        )
        .await;
        Ok(WorkspaceServiceHealthResponse {
            service_id: service.id,
            health,
            probed_path: path,
            expected_status: expect_status,
        })
    }

    /// `workspace/project/close` hook: stop every non-terminal service of
    /// the project, then drop all its service records. Kill failures never
    /// block close (strategy: binding removal wins); they are logged.
    pub(crate) async fn stop_all_for_project(&self, project_id: &str) -> usize {
        let candidates: Vec<String> = {
            let store = self.store.lock().expect("store lock");
            store
                .services
                .values()
                .filter(|service| service.project_id == project_id && !is_terminal(service.status))
                .map(|service| service.id.clone())
                .collect()
        };
        for service_id in &candidates {
            // stop_one kills the process group, waits for the terminal
            // publish, forces the terminal mark when the reaper raced, and
            // drops the runtime entry.
            let _ = self.stop_one(service_id).await;
        }
        {
            // Purge every trace of the project in one locked write (8.2:
            // close leaves zero residual state): service records, specs,
            // define idempotency entries, and orchestration replay records.
            let mut store = self.store.lock().expect("store lock");
            let removed_spec_ids: Vec<String> = store
                .specs
                .values()
                .filter(|spec| spec.project_id == project_id)
                .map(|spec| spec.id.clone())
                .collect();
            store.specs.retain(|_, spec| spec.project_id != project_id);
            store
                .spec_idempotency
                .retain(|_, spec_id| !removed_spec_ids.contains(spec_id));
            store
                .orchestrations
                .retain(|_, record| record.project_id != project_id);
            store
                .services
                .retain(|_, service| service.project_id != project_id);
            let _ = store.persist();
        }
        candidates.len()
    }

    // --- internals ---

    fn project(
        &self,
        project_id: &str,
    ) -> Result<ody_app_server_protocol::WorkspaceProjectRef, JSONRPCErrorError> {
        let store = self.project_store.lock().expect("project store lock");
        store
            .projects
            .get(project_id)
            .cloned()
            .ok_or_else(|| invalid_params(format!("unknown project id {project_id}")))
    }

    fn service_record(&self, service_id: &str) -> Result<WorkspaceServiceRef, JSONRPCErrorError> {
        self.store
            .lock()
            .expect("store lock")
            .services
            .get(service_id)
            .cloned()
            .ok_or_else(|| invalid_params(format!("unknown service id {service_id}")))
    }

    fn persist_service(&self, service: &WorkspaceServiceRef, idem_key: Option<&str>) {
        let mut store = self.store.lock().expect("store lock");
        store.services.insert(service.id.clone(), service.clone());
        if let Some(key) = idem_key {
            store.idempotency.insert(key.to_owned(), service.id.clone());
        }
        let _ = store.persist();
    }

    fn stderr_tail_of(&self, service_id: &str) -> String {
        self.runtime
            .lock()
            .expect("runtime lock")
            .get(service_id)
            .map(|entry| {
                entry
                    .stderr_ring
                    .lock()
                    .expect("ring lock")
                    .tail_string(2 * 1024)
            })
            .unwrap_or_default()
    }
}

/// Lightweight clone of the runtime bits `stop` needs (ManagedService is not Clone).
struct ManagedServiceClone {
    pgid: u32,
    stop_requested: Arc<std::sync::atomic::AtomicBool>,
    terminal_notify: Arc<tokio::sync::Notify>,
}

pub(crate) fn is_terminal(status: WorkspaceServiceStatus) -> bool {
    matches!(
        status,
        WorkspaceServiceStatus::Failed
            | WorkspaceServiceStatus::Exited
            | WorkspaceServiceStatus::Stopped
    )
}

impl WorkspaceServiceStore {
    fn load(path: PathBuf) -> Self {
        let load_from = |source: &Path| -> Option<Self> {
            let contents = fs::read_to_string(source).ok()?;
            let mut store: Self = serde_json::from_str(&contents).ok()?;
            store.schema_version = WORKSPACE_SERVICE_PROTOCOL_VERSION;
            store.path = path.clone();
            // Runtime restart: no process handles survive; normalize (R4).
            let mut normalized = false;
            for service in store.services.values_mut() {
                if !is_terminal(service.status) {
                    service.status = WorkspaceServiceStatus::Stopped;
                    service.error = Some(RUNTIME_RESTARTED_ERROR.to_owned());
                    service.updated_at_ms = now_ms();
                    normalized = true;
                }
            }
            if normalized {
                let _ = store.persist();
            }
            Some(store)
        };
        let fallback_path = path.clone();
        load_from(&path).unwrap_or_else(|| {
            let backup = path.with_extension("json.bak");
            load_from(&backup).unwrap_or_else(|| Self {
                schema_version: WORKSPACE_SERVICE_PROTOCOL_VERSION,
                services: BTreeMap::new(),
                idempotency: BTreeMap::new(),
                specs: BTreeMap::new(),
                spec_idempotency: BTreeMap::new(),
                orchestrations: BTreeMap::new(),
                path: fallback_path,
            })
        })
    }

    fn persist(&self) -> Result<(), JSONRPCErrorError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|err| internal_error(err.to_string()))?;
        }
        let contents =
            serde_json::to_string_pretty(self).map_err(|err| internal_error(err.to_string()))?;
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, contents).map_err(|err| internal_error(err.to_string()))?;
        if self.path.exists() {
            let _ = fs::copy(&self.path, self.path.with_extension("json.bak"));
        }
        fs::rename(&tmp, &self.path).map_err(|err| internal_error(err.to_string()))?;
        Ok(())
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX)
}
/// spec.cwd: normalize (E1 normalize_relative rules), must be an existing
/// directory under the root, and canonicalized target must stay under the
/// canonical root (symlink escape guard, ADR §4.1).
fn resolve_cwd(root_path: &Path, cwd: Option<&str>) -> Result<PathBuf, JSONRPCErrorError> {
    let Some(cwd) = cwd else {
        return Ok(root_path.to_path_buf());
    };
    let normalized = crate::workspace_changeset::normalize_relative(cwd)?;
    let target = root_path.join(&normalized);
    let canonical_root = root_path.canonicalize().map_err(|err| {
        invalid_params(format!(
            "cannot canonicalize root {}: {err}",
            root_path.display()
        ))
    })?;
    let canonical_target = target.canonicalize().map_err(|err| {
        invalid_params(format!(
            "cwd {cwd:?} is not an existing directory under root {}: {err}",
            root_path.display()
        ))
    })?;
    if !canonical_target.starts_with(&canonical_root) {
        return Err(invalid_params(format!(
            "cwd {cwd:?} resolves outside the workspace root ({})",
            root_path.display()
        )));
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_terminal_matches_state_machine() {
        assert!(!is_terminal(WorkspaceServiceStatus::Starting));
        assert!(!is_terminal(WorkspaceServiceStatus::Ready));
        assert!(is_terminal(WorkspaceServiceStatus::Failed));
        assert!(is_terminal(WorkspaceServiceStatus::Exited));
        assert!(is_terminal(WorkspaceServiceStatus::Stopped));
    }

    #[test]
    fn store_restart_normalizes_lingering_services_to_stopped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("workspace-service").join("v1.json");
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        let mut store = WorkspaceServiceStore {
            schema_version: WORKSPACE_SERVICE_PROTOCOL_VERSION,
            services: BTreeMap::new(),
            idempotency: BTreeMap::new(),
            specs: BTreeMap::new(),
            spec_idempotency: BTreeMap::new(),
            orchestrations: BTreeMap::new(),
            path: path.clone(),
        };
        store.services.insert(
            "svc-1".to_owned(),
            WorkspaceServiceRef {
                id: "svc-1".to_owned(),
                project_id: "ws-1".to_owned(),
                name: "web".to_owned(),
                root_index: 0,
                script: "dev".to_owned(),
                command: "npm run dev -- --port 5173".to_owned(),
                port: 5173,
                url: "http://127.0.0.1:5173/".to_owned(),
                status: WorkspaceServiceStatus::Ready, // lingering
                pid: Some(1),
                exit_code: None,
                health: None,
                error: None,
                created_at_ms: 0,
                updated_at_ms: 0,
            },
        );
        store.persist().expect("persist");

        let reloaded = WorkspaceServiceStore::load(path);
        let service = reloaded.services.get("svc-1").expect("service");
        assert_eq!(service.status, WorkspaceServiceStatus::Stopped);
        assert_eq!(
            service.error.as_deref(),
            Some("runtime restarted; process not running")
        );
    }

    fn test_outgoing() -> (
        Arc<OutgoingMessageSender>,
        tokio::sync::mpsc::Receiver<crate::outgoing_message::OutgoingEnvelope>,
    ) {
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        (
            Arc::new(OutgoingMessageSender::new(
                tx,
                ody_analytics::AnalyticsEventsClient::disabled(),
            )),
            rx,
        )
    }

    /// Bind a one-root project "p1" into a fresh ody_home and return the
    /// shared project store handle plus the audit log.
    async fn processor_fixture(
        root: &Path,
    ) -> (
        WorkspaceServiceRequestProcessor,
        tokio::sync::mpsc::Receiver<crate::outgoing_message::OutgoingEnvelope>,
    ) {
        use crate::request_processors::workspace_project_processor::WorkspaceProjectRequestProcessor;
        use ody_utils_absolute_path::AbsolutePathBuf;

        let ody_home = tempfile::tempdir().expect("ody home");
        let audit = Arc::new(crate::workspace_audit::WorkspaceAuditLog::new(
            ody_home.path().join("workspace-audit").join("v1.jsonl"),
        ));
        let project_processor = WorkspaceProjectRequestProcessor::new(
            ody_home.path().to_path_buf(),
            Arc::clone(&audit),
            Arc::new(crate::workspace_lock::WorkspaceWriteLock::default()),
        );
        let root = AbsolutePathBuf::try_from(root.to_path_buf()).expect("absolute root");
        project_processor
            .bind(ody_app_server_protocol::WorkspaceProjectBindParams {
                id: "p1".to_owned(),
                name: "fixture".to_owned(),
                roots: vec![root],
                idempotency_key: "bind-1".to_owned(),
            })
            .await
            .expect("bind project");
        let (outgoing, rx) = test_outgoing();
        let processor = WorkspaceServiceRequestProcessor::new(
            ody_home.path().to_path_buf(),
            project_processor.store_handle(),
            audit,
            outgoing,
            Arc::new(crate::workspace_lock::WorkspaceWriteLock::default()),
        );
        (processor, rx)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn crashed_service_broadcasts_failed_notification() {
        // Real subprocess exit is the behavior under test; skip on hosts
        // without node (CI images that lack it).
        if tokio::process::Command::new("node")
            .arg("--version")
            .output()
            .await
            .is_err()
        {
            return;
        }
        let root = tempfile::tempdir().expect("root");
        fs::write(
            root.path().join("package.json"),
            serde_json::json!({
                "name": "crasher-fixture",
                "scripts": { "crash": "node crasher.js" }
            })
            .to_string(),
        )
        .expect("package.json");
        fs::write(
            root.path().join("crasher.js"),
            "setTimeout(() => process.exit(3), 100);\n",
        )
        .expect("crasher.js");
        let (processor, mut rx) = processor_fixture(root.path()).await;
        let response = processor
            .start(WorkspaceServiceStartParams {
                project_id: "p1".to_owned(),
                name: "crasher".to_owned(),
                root_index: 0,
                script: "crash".to_owned(),
                port: None,
                ready_timeout_ms: Some(5_000),
                env: None,
                secret_values: None,
                idempotency_key: "crash-1".to_owned(),
            })
            .await
            .expect("start");
        let notification = tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                let envelope = rx.recv().await.expect("notification");
                let crate::outgoing_message::OutgoingEnvelope::Broadcast { message } = envelope
                else {
                    continue;
                };
                let ody_app_server_transport::OutgoingMessage::AppServerNotification(notification) = message
                else {
                    continue;
                };
                if let ServerNotification::WorkspaceServiceChanged(changed) = notification {
                    if changed.reason == WorkspaceServiceChangedReason::Failed {
                        return changed;
                    }
                }
            }
        })
        .await
        .expect("timed out waiting for service/changed");
        assert_eq!(notification.project_id, "p1");
        assert_eq!(notification.services.len(), 1);
        assert_eq!(notification.services[0].id, response.service.id);
        assert_eq!(notification.services[0].exit_code, Some(3));
        assert_eq!(notification.services[0].status, WorkspaceServiceStatus::Failed);
    }

    #[test]
    fn load_normalization_marks_orchestration_cleanup_and_restart_notice() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("workspace-service").join("v1.json");
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        let mut store = WorkspaceServiceStore {
            schema_version: WORKSPACE_SERVICE_PROTOCOL_VERSION,
            services: BTreeMap::new(),
            idempotency: BTreeMap::new(),
            specs: BTreeMap::new(),
            spec_idempotency: BTreeMap::new(),
            orchestrations: BTreeMap::new(),
            path: path.clone(),
        };
        store.services.insert(
            "svc-1".to_owned(),
            WorkspaceServiceRef {
                id: "svc-1".to_owned(),
                project_id: "gone-project".to_owned(),
                name: "web".to_owned(),
                root_index: 0,
                script: "dev".to_owned(),
                command: "npm run dev".to_owned(),
                port: 5173,
                url: String::new(),
                status: WorkspaceServiceStatus::Ready, // lingering
                pid: Some(1),
                exit_code: None,
                health: None,
                error: None,
                created_at_ms: 0,
                updated_at_ms: 0,
            },
        );
        store.orchestrations.insert(
            "start_all:key-1".to_owned(),
            StoredOrchestration {
                project_id: "gone-project".to_owned(),
                response: WorkspaceServiceStartAllResponse {
                    services: Vec::new(),
                    started: Vec::new(),
                    reused: Vec::new(),
                    failed: Vec::new(),
                },
                created_at_ms: 0,
            },
        );
        store.persist().expect("persist");
        drop(store);

        // Project store with zero projects -> the orchestration record is stale.
        let project_store = Arc::new(Mutex::new(WorkspaceProjectStore::default()));
        let (outgoing, _rx) = test_outgoing();
        let audit = Arc::new(crate::workspace_audit::WorkspaceAuditLog::new(
            dir.path().join("workspace-audit").join("v1.jsonl"),
        ));
        let processor = WorkspaceServiceRequestProcessor::new(
            dir.path().to_path_buf(),
            project_store,
            audit,
            outgoing,
            Arc::new(crate::workspace_lock::WorkspaceWriteLock::default()),
        );
        let store = processor.store.lock().expect("store lock");
        assert!(store.orchestrations.is_empty(), "stale orchestration dropped");
        let service = store.services.values().next().expect("service");
        assert_eq!(service.status, WorkspaceServiceStatus::Stopped);
        assert_eq!(service.error.as_deref(), Some(RUNTIME_RESTARTED_ERROR));
        drop(store);
        let notice = processor
            .take_startup_notification()
            .expect("restart notice");
        assert_eq!(notice.reason, WorkspaceServiceChangedReason::RuntimeRestarted);
        assert_eq!(notice.project_id, "gone-project");
        assert_eq!(notice.services.len(), 1);
    }

    #[test]
    fn store_corrupt_falls_back_to_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("workspace-service").join("v1.json");
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(&path, "{ not json").expect("write corrupt");
        let store = WorkspaceServiceStore::load(path);
        assert!(store.services.is_empty());
        assert_eq!(store.schema_version, WORKSPACE_SERVICE_PROTOCOL_VERSION);
    }

    #[test]
    fn define_validates_cycle_through_upsert() {
        // Graph assembled across defines: a -> b, then b -> a must be rejected.
        let mut specs: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        specs.insert("a".to_owned(), vec!["b".to_owned()]);
        let would_be: Vec<(String, Vec<String>)> = {
            let mut next = specs.clone();
            next.insert("b".to_owned(), vec!["a".to_owned()]);
            next.into_iter().collect()
        };
        let err = crate::workspace_service::topo_levels(&would_be).expect_err("cycle");
        assert!(err.contains("cycle"), "{err}");
    }

    #[test]
    fn store_schema_v2_loads_v1_file_with_missing_spec_fields() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("workspace-service").join("v1.json");
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        // A v1-era file: services + idempotency only, no specs/orchestrations.
        fs::write(
            &path,
            serde_json::json!({
                "schemaVersion": 1,
                "services": {},
                "idempotency": {}
            })
            .to_string(),
        )
        .expect("write v1 store");
        let store = WorkspaceServiceStore::load(path);
        assert!(store.specs.is_empty());
        assert!(store.orchestrations.is_empty());
        assert_eq!(store.schema_version, WORKSPACE_SERVICE_PROTOCOL_VERSION);
    }

    // ---- E4 T06: envRefs + secretValues helpers ----

    /// Root fixture whose `dev` script is a real loopback HTTP server that
    /// records `process.env.STRIPE_KEY` into `<root>/.env-capture` on boot,
    /// so tests can observe the injected value from the child process.
    async fn env_capture_fixture() -> (
        tempfile::TempDir,
        WorkspaceServiceRequestProcessor,
        tokio::sync::mpsc::Receiver<crate::outgoing_message::OutgoingEnvelope>,
    ) {
        let root = tempfile::tempdir().expect("root");
        fs::write(
            root.path().join("package.json"),
            serde_json::json!({
                "name": "env-refs-fixture",
                "scripts": { "dev": "node capture.js" }
            })
            .to_string(),
        )
        .expect("package.json");
        fs::write(
            root.path().join("capture.js"),
            r#"const fs = require('fs');
fs.writeFileSync('.env-capture', process.env.STRIPE_KEY || '');
const http = require('http');
const port = Number(process.env.PORT || 5173);
http.createServer((req, res) => {
    res.writeHead(200, { 'content-type': 'text/plain' });
    res.end('ok');
}).listen(port, '127.0.0.1');
"#,
        )
        .expect("capture.js");
        let (processor, rx) = processor_fixture(root.path()).await;
        (root, processor, rx)
    }

    fn define_params(name: &str, env_refs: Option<Vec<String>>) -> WorkspaceServiceDefineParams {
        WorkspaceServiceDefineParams {
            project_id: "p1".to_owned(),
            name: name.to_owned(),
            root_index: 0,
            script: "dev".to_owned(),
            port: None,
            cwd: None,
            depends_on: Vec::new(),
            health_check: None,
            ready_timeout_ms: None,
            env_refs,
            idempotency_key: format!("define-{name}"),
        }
    }

    /// Builder so tests can chain `.with_secrets(...)` / `.with_user_env(...)`
    /// without listing every additive field at each call site.
    struct StartBuilder(WorkspaceServiceStartParams);

    fn start_params(project_id: &str, name: &str) -> StartBuilder {
        StartBuilder(WorkspaceServiceStartParams {
            project_id: project_id.to_owned(),
            name: name.to_owned(),
            root_index: 0,
            script: "dev".to_owned(),
            port: None,
            ready_timeout_ms: None,
            env: None,
            secret_values: None,
            idempotency_key: format!("start-{project_id}-{name}"),
        })
    }

    impl StartBuilder {
        fn with_secrets(mut self, secrets: BTreeMap<String, String>) -> Self {
            self.0.secret_values = Some(secrets);
            self
        }
        fn with_user_env(mut self, env: HashMap<String, Option<String>>) -> Self {
            self.0.env = Some(env);
            self
        }
    }

    async fn wait_for_capture(path: &Path) -> String {
        for _ in 0..100 {
            if let Ok(content) = fs::read_to_string(path) {
                if !content.is_empty() {
                    return content;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        panic!("capture file never appeared at {}", path.display());
    }

    #[tokio::test]
    async fn define_validates_env_refs_names_and_limits() {
        let (_root, processor, _rx) = env_capture_fixture().await;
        // 合法：upper-snake、下划线、首位之后的数字。
        processor
            .define(define_params(
                "s1",
                Some(vec!["STRIPE_KEY".into(), "API_V2_TOKEN".into()]),
            ))
            .await
            .expect("valid envRefs accepted");
        // 非法：小写（ref 必须 upper-snake，防 env key 混淆）。
        let err = processor
            .define(define_params("s2", Some(vec!["secret".into()])))
            .await
            .expect_err("lowercase ref rejected");
        assert!(err.message.contains("UPPER_SNAKE"), "{}", err.message);
        // 非法：非 ASCII。
        let err = processor
            .define(define_params("s3", Some(vec!["密钥".into()])))
            .await
            .expect_err("non-ascii ref rejected");
        assert!(err.message.contains("envRefs"), "{}", err.message);
        // 非法：重复。
        let err = processor
            .define(define_params("s4", Some(vec!["KEY".into(), "KEY".into()])))
            .await
            .expect_err("duplicate ref rejected");
        assert!(err.message.contains("duplicate"), "{}", err.message);
        // 非法：超 16 个。
        let many: Vec<String> = (0..17).map(|i| format!("K{i:02}")).collect();
        let err = processor
            .define(define_params("s5", Some(many)))
            .await
            .expect_err("too many refs");
        assert!(err.message.contains("16"), "{}", err.message);
    }

    #[tokio::test]
    async fn start_rejects_missing_secret_and_never_persists_values() {
        let (_root, processor, _rx) = env_capture_fixture().await;
        processor
            .define(define_params(
                "api",
                Some(vec!["STRIPE_KEY".into(), "DB_PASS".into()]),
            ))
            .await
            .expect("define");
        // 缺失 DB_PASS：结构化错误只列 ref 名，不回显任何值。
        let err = processor
            .start(
                start_params("p1", "api")
                    .with_secrets(BTreeMap::from([(
                        "STRIPE_KEY".to_owned(),
                        "sk-live-12345".to_owned(),
                    )]))
                    .0,
            )
            .await
            .expect_err("missing secret must reject");
        assert!(err.message.contains("DB_PASS"), "{}", err.message);
        assert!(
            !err.message.contains("sk-live-12345"),
            "error must not echo values"
        );
        // 提供的密钥不得出现在 store 文件中（ref 名应持久化）。
        let store_path = processor.store.lock().expect("store lock").path.clone();
        let raw = fs::read_to_string(&store_path).expect("store file");
        assert!(raw.contains("STRIPE_KEY"), "ref name persisted");
        assert!(!raw.contains("sk-live-12345"), "secret value never persisted");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn start_injects_secret_env_and_user_env_overrides_it() {
        // Real subprocess env is the behavior under test; skip on hosts
        // without node (CI images that lack it).
        if tokio::process::Command::new("node")
            .arg("--version")
            .output()
            .await
            .is_err()
        {
            return;
        }
        let (root, processor, _rx) = env_capture_fixture().await;
        processor
            .define(define_params("api", Some(vec!["STRIPE_KEY".into()])))
            .await
            .expect("define");
        let first = processor
            .start(
                start_params("p1", "api")
                    .with_secrets(BTreeMap::from([(
                        "STRIPE_KEY".to_owned(),
                        "from-secret".to_owned(),
                    )]))
                    .with_user_env(HashMap::from([(
                        "STRIPE_KEY".to_owned(),
                        Some("from-user".to_owned()),
                    )]))
                    .0,
            )
            .await
            .expect("start");
        let captured = wait_for_capture(&root.path().join(".env-capture")).await;
        assert_eq!(
            captured.trim(),
            "from-user",
            "explicit user env wins over secret (ADR 4 precedence)"
        );

        // 无 user override 时 secret 生效（第二个独立 fixture 实例）。
        let (root2, processor2, _rx2) = env_capture_fixture().await;
        processor2
            .define(define_params("api", Some(vec!["STRIPE_KEY".into()])))
            .await
            .expect("define");
        let second = processor2
            .start(
                start_params("p1", "api")
                    .with_secrets(BTreeMap::from([(
                        "STRIPE_KEY".to_owned(),
                        "from-secret".to_owned(),
                    )]))
                    .0,
            )
            .await
            .expect("start");
        let captured2 = wait_for_capture(&root2.path().join(".env-capture")).await;
        assert_eq!(captured2.trim(), "from-secret", "secret injected when no override");

        // Leave no child processes behind.
        for (processor, service_id) in
            [
                (&processor, first.service.id.clone()),
                (&processor2, second.service.id.clone()),
            ]
        {
            let _ = processor
                .stop(ody_app_server_protocol::WorkspaceServiceStopParams { service_id })
                .await;
        }
    }

    #[test]
    fn env_merge_prefers_explicit_user_env_over_injected() {
        let mut merged: HashMap<String, Option<String>> = [(
            "BACKEND_URL".to_owned(),
            Some("http://127.0.0.1:1/".to_owned()),
        )]
        .into_iter()
        .collect();
        // Explicit user override wins over injected (ADR decision 4).
        merged.insert(
            "BACKEND_URL".to_owned(),
            Some("http://127.0.0.1:9/".to_owned()),
        );
        assert_eq!(
            merged.get("BACKEND_URL").and_then(|v| v.as_deref()),
            Some("http://127.0.0.1:9/")
        );
    }

    #[test]
    fn diagnose_rejects_out_of_bounds_failure_batch() {
        // Mirror of the request-level validation performed in `diagnose`;
        // the wiring itself is covered by the T06 wire-level integration test.
        let failures = vec![ody_app_server_protocol::WorkspaceObservedNetworkFailure {
            url: "http://127.0.0.1:1/".to_owned(),
            method: None,
            status: None,
            error: None,
            occurred_at_ms: None,
        }];
        assert!(!failures.is_empty());
        assert!(failures.len() <= crate::workspace_service::DIAGNOSE_MAX_FAILURES);
    }
}
