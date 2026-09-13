use std::collections::BTreeMap;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::Ordering;

use ody_app_server_protocol::JSONRPCErrorError;
use ody_app_server_protocol::WORKSPACE_SERVICE_PROTOCOL_VERSION;
use ody_app_server_protocol::WorkspaceServiceHealth;
use ody_app_server_protocol::WorkspacePreviewCheckParams;
use ody_app_server_protocol::WorkspacePreviewCheckResponse;
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
use serde::Deserialize;
use serde::Serialize;
use uuid::Uuid;

use crate::error_code::internal_error;
use crate::error_code::invalid_params;
use crate::request_processors::workspace_project_processor::WorkspaceProjectStore;
use crate::workspace_service::ManagedService;
use crate::workspace_service::OutputRing;
use crate::workspace_service::ReadyOutcome;

const STORE_DIR: &str = "workspace-service";
const STORE_FILE: &str = "v1.json";

#[derive(Clone)]
pub(crate) struct WorkspaceServiceRequestProcessor {
    project_store: Arc<Mutex<WorkspaceProjectStore>>,
    store: Arc<Mutex<WorkspaceServiceStore>>,
    runtime: Arc<Mutex<HashMap<String, ManagedService>>>,
}

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct WorkspaceServiceStore {
    schema_version: u32,
    pub(crate) services: BTreeMap<String, WorkspaceServiceRef>,
    /// Namespaced idempotency key (`"start:{key}"`) -> service id.
    idempotency: BTreeMap<String, String>,
    #[serde(skip)]
    path: PathBuf,
}

impl WorkspaceServiceRequestProcessor {
    pub(crate) fn new(ody_home: PathBuf, project_store: Arc<Mutex<WorkspaceProjectStore>>) -> Self {
        let path = ody_home.join(STORE_DIR).join(STORE_FILE);
        let store = WorkspaceServiceStore::load(path);
        Self {
            project_store,
            store: Arc::new(Mutex::new(store)),
            runtime: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(crate) async fn start(
        &self,
        params: WorkspaceServiceStartParams,
    ) -> Result<WorkspaceServiceStartResponse, JSONRPCErrorError> {
        let project = self.project(&params.project_id)?;
        let name = params.name.trim();
        if name.is_empty() || name.chars().count() > crate::workspace_service::MAX_SERVICE_NAME_CHARS {
            return Err(invalid_params(format!(
                "service name must be 1-{} chars, got {:?}",
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
        let root_path = Path::new(&root.path);

        // Idempotent replay: same key returns the recorded service.
        let idem_key = format!("start:{}", params.idempotency_key);
        {
            let store = self.store.lock().expect("store lock");
            if let Some(service_id) = store.idempotency.get(&idem_key) {
                if let Some(service) = store.services.get(service_id) {
                    return Ok(WorkspaceServiceStartResponse { service: service.clone() });
                }
            }
        }

        // Script must exist in the root's package.json (Runtime never guesses).
        let mut discovery = ody_app_server_protocol::WorkspaceRootDiscovery {
            root_path: root.path.clone(),
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
        crate::workspace_discovery::inspect_package_json(root_path, &mut discovery);
        let script = discovery
            .scripts
            .iter()
            .find(|entry| entry.name == params.script)
            .ok_or_else(|| {
                invalid_params(format!(
                    "script {:?} not found in {}/package.json; available: {}",
                    params.script,
                    root.path,
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

        // One non-terminal service per name; bounded per project.
        {
            let store = self.store.lock().expect("store lock");
            let active = store
                .services
                .values()
                .filter(|service| {
                    service.project_id == params.project_id && !is_terminal(service.status)
                })
                .count();
            if active >= crate::workspace_service::MAX_SERVICES_PER_PROJECT {
                return Err(invalid_params(format!(
                    "project {} already has {} active services (max {})",
                    params.project_id,
                    active,
                    crate::workspace_service::MAX_SERVICES_PER_PROJECT
                )));
            }
            if store.services.values().any(|service| {
                service.project_id == params.project_id
                    && service.name == name
                    && !is_terminal(service.status)
            }) {
                return Err(invalid_params(format!(
                    "service name {name:?} is already active in project {}; stop it first",
                    params.project_id
                )));
            }
        }

        let pm = crate::workspace_discovery::detect_package_manager(root_path)
            .unwrap_or_else(|| "npm".to_owned());
        let port = crate::workspace_service::pick_port(params.port, &tech_ids)
            .map_err(invalid_params)?;
        let args = crate::workspace_service::build_command_args(&script.name, port, &tech_ids);
        let command_display = format!("{pm} {}", args.join(" "));
        let now = now_ms();
        let service_id = Uuid::new_v4().to_string();
        let url = format!("http://127.0.0.1:{port}/");

        let mut service = WorkspaceServiceRef {
            id: service_id.clone(),
            project_id: params.project_id.clone(),
            name: name.to_owned(),
            root_index: params.root_index,
            script: script.name.clone(),
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

        let spawn_result = crate::workspace_service::spawn_service_command(
            &pm,
            &args,
            root_path,
            port,
            &params.env,
        );
        let mut child = match spawn_result {
            Ok(child) => child,
            Err(err) => {
                service.status = WorkspaceServiceStatus::Failed;
                service.error = Some(format!("failed to spawn {pm}: {err}"));
                self.persist_service(&service, Some(&idem_key));
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
        self.persist_service(&service, Some(&idem_key));

        // Wait task: owns the child; writes the terminal state exactly once.
        {
            let store = self.store.clone();
            let service_id = service_id.clone();
            let stop_requested = stop_requested.clone();
            let terminated = terminated.clone();
            let terminal_notify = terminal_notify.clone();
            tokio::spawn(async move {
                let exit = child.wait().await;
                terminated.store(true, Ordering::SeqCst);
                let exit_code = exit.ok().and_then(|status| status.code());
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
                terminal_notify.notify_waiters();
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
        let outcome = crate::workspace_service::wait_ready(
            &terminated,
            port,
            crate::workspace_service::clamp_ready_timeout(params.ready_timeout_ms),
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
                }
                ReadyOutcome::ProcessExited => {
                    // Wait task already wrote Failed/Exited with exit code.
                }
                ReadyOutcome::TimedOut => {
                    record.status = WorkspaceServiceStatus::Failed;
                    record.error = Some(format!(
                        "readiness timeout after {} ms; stderr tail: {}",
                        crate::workspace_service::clamp_ready_timeout(params.ready_timeout_ms),
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
        Ok(WorkspaceServiceStartResponse { service })
    }

    pub(crate) async fn stop(
        &self,
        params: WorkspaceServiceStopParams,
    ) -> Result<WorkspaceServiceStopResponse, JSONRPCErrorError> {
        let runtime_entry = {
            let mut runtime = self.runtime.lock().expect("runtime lock");
            runtime.get(&params.service_id).map(|entry| ManagedServiceClone {
                pgid: entry.pgid,
                stop_requested: entry.stop_requested.clone(),
                terminal_notify: entry.terminal_notify.clone(),
            })
        };
        let (pgid, stop_requested, terminal_notify) = match runtime_entry {
            Some(entry) => (entry.pgid, entry.stop_requested, entry.terminal_notify),
            None => {
                // Terminal or after-restart: just report the stored record.
                let service = self.service_record(&params.service_id)?;
                return Ok(WorkspaceServiceStopResponse { service });
            }
        };
        stop_requested.store(true, Ordering::SeqCst);
        if let Err(err) = crate::workspace_service::kill_process_group(pgid) {
            tracing::warn!(service = %params.service_id, error = %err, "kill_process_group failed");
        }
        // Wait briefly for the wait task to publish the terminal state.
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            terminal_notify.notified(),
        )
        .await;
        let mut service = self.service_record(&params.service_id)?;
        if !is_terminal(service.status) {
            // killpg raced with the reaper window; force the terminal mark.
            service.status = WorkspaceServiceStatus::Stopped;
            service.updated_at_ms = now_ms();
            let mut store = self.store.lock().expect("store lock");
            store.services.insert(service.id.clone(), service.clone());
            let _ = store.persist();
        }
        self.runtime.lock().expect("runtime lock").remove(&params.service_id);
        Ok(WorkspaceServiceStopResponse { service })
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
                std::time::Duration::from_millis(crate::workspace_service::SERVICE_HEALTH_TIMEOUT_MS),
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
                        error: Some(format!("health probe timed out after {} ms", crate::workspace_service::SERVICE_HEALTH_TIMEOUT_MS)),
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
            stdout_tail: entry.stdout_ring.lock().expect("ring lock").tail_string(tail_bytes),
            stderr_tail: entry.stderr_ring.lock().expect("ring lock").tail_string(tail_bytes),
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

    /// `workspace/project/close` hook: stop every non-terminal service of
    /// the project, then drop all its service records. Kill failures never
    /// block close (strategy: binding removal wins); they are logged.
    pub(crate) async fn stop_all_for_project(&self, project_id: &str) -> usize {
        let candidates: Vec<String> = {
            let store = self.store.lock().expect("store lock");
            store
                .services
                .values()
                .filter(|service| {
                    service.project_id == project_id && !is_terminal(service.status)
                })
                .map(|service| service.id.clone())
                .collect()
        };
        for service_id in &candidates {
            let entry = {
                let runtime = self.runtime.lock().expect("runtime lock");
                runtime.get(service_id).map(|entry| {
                    (
                        entry.pgid,
                        entry.stop_requested.clone(),
                        entry.terminal_notify.clone(),
                    )
                })
            };
            if let Some((pgid, stop_requested, terminal_notify)) = entry {
                stop_requested.store(true, Ordering::SeqCst);
                if let Err(err) = crate::workspace_service::kill_process_group(pgid) {
                    tracing::warn!(service = %service_id, error = %err, "close cleanup kill failed");
                }
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    terminal_notify.notified(),
                )
                .await;
                self.runtime
                    .lock()
                    .expect("runtime lock")
                    .remove(service_id);
            }
            // Terminal entries have no live process; normalize the record.
            let mut store = self.store.lock().expect("store lock");
            if let Some(record) = store.services.get_mut(service_id) {
                if !is_terminal(record.status) {
                    record.status = WorkspaceServiceStatus::Stopped;
                    record.error = Some("stopped during project close".to_owned());
                    record.updated_at_ms = now_ms();
                }
            }
        }
        if !candidates.is_empty() {
            let mut store = self.store.lock().expect("store lock");
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

    fn service_record(
        &self,
        service_id: &str,
    ) -> Result<WorkspaceServiceRef, JSONRPCErrorError> {
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
        store
            .services
            .insert(service.id.clone(), service.clone());
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
                    service.error = Some("runtime restarted; process not running".to_owned());
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
}
