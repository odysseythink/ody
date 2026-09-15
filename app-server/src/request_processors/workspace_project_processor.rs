use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use ody_app_server_protocol::JSONRPCErrorError;
use ody_app_server_protocol::WORKSPACE_PROJECT_PROTOCOL_VERSION;
use ody_app_server_protocol::WorkspaceDiscovery;
use ody_app_server_protocol::WorkspaceProjectBindParams;
use ody_app_server_protocol::WorkspaceProjectBindResponse;
use ody_app_server_protocol::WorkspaceProjectCloseParams;
use ody_app_server_protocol::WorkspaceProjectCloseResponse;
use ody_app_server_protocol::WorkspaceProjectGetParams;
use ody_app_server_protocol::WorkspaceProjectGetResponse;
use ody_app_server_protocol::WorkspaceProjectListParams;
use ody_app_server_protocol::WorkspaceProjectListResponse;
use ody_app_server_protocol::WorkspaceProjectLockParams;
use ody_app_server_protocol::WorkspaceProjectLockResponse;
use ody_app_server_protocol::WorkspaceProjectUnlockParams;
use ody_app_server_protocol::WorkspaceProjectUnlockResponse;
use ody_app_server_protocol::WorkspaceProjectRef;
use ody_app_server_protocol::WorkspaceProjectScanParams;
use ody_app_server_protocol::WorkspaceProjectScanResponse;
use ody_app_server_protocol::WorkspaceRoot;
use ody_app_server_protocol::WorkspaceRootRole;
use ody_utils_absolute_path::AbsolutePathBuf;
use serde::Deserialize;
use serde::Serialize;
use uuid::Uuid;

use crate::error_code::internal_error;
use crate::outgoing_message::ConnectionId;
use crate::workspace_lock::WorkspaceWriteLock;
use crate::error_code::invalid_params;
use crate::workspace_audit::audit_event;
use crate::workspace_audit::error_detail;
use crate::workspace_audit::WorkspaceAuditLog;

const STORE_DIR: &str = "workspace-project";
const STORE_FILE: &str = "v1.json";
const MAX_ROOTS: usize = 8;
const AUTH_SOURCE_USER_SELECTED: &str = "user_selected";

#[derive(Clone)]
pub(crate) struct WorkspaceProjectRequestProcessor {
    ody_home: PathBuf,
    store: Arc<Mutex<WorkspaceProjectStore>>,
    audit: Arc<WorkspaceAuditLog>,
    write_lock: Arc<WorkspaceWriteLock>,
}

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct WorkspaceProjectStore {
    schema_version: u32,
    pub(crate) projects: BTreeMap<String, WorkspaceProjectRef>,
    /// Namespaced idempotency key (`"bind:{key}"`) -> project id.
    idempotency: BTreeMap<String, String>,
    #[serde(skip)]
    path: PathBuf,
}

impl WorkspaceProjectRequestProcessor {
    pub(crate) fn new(
        ody_home: PathBuf,
        audit: Arc<WorkspaceAuditLog>,
        write_lock: Arc<WorkspaceWriteLock>,
    ) -> Self {
        let path = ody_home.join(STORE_DIR).join(STORE_FILE);
        let store = WorkspaceProjectStore::load(path);
        Self {
            ody_home,
            store: Arc::new(Mutex::new(store)),
            audit,
            write_lock,
        }
    }

    /// Shared handle so sibling processors (workspace/source) can read binding
    /// records without a second store instance on the same file.
    pub(crate) fn store_handle(&self) -> Arc<Mutex<WorkspaceProjectStore>> {
        self.store.clone()
    }

    /// Shared write-intent lock table (see `workspace_lock`).
    pub(crate) fn write_lock(&self) -> Arc<WorkspaceWriteLock> {
        Arc::clone(&self.write_lock)
    }

    /// Explicit write-intent declaration. Idempotent for the current holder;
    /// other connections are rejected with a diagnosable error.
    pub(crate) async fn lock(
        &self,
        params: WorkspaceProjectLockParams,
        connection_id: ConnectionId,
    ) -> Result<WorkspaceProjectLockResponse, JSONRPCErrorError> {
        self.write_lock.acquire(&params.project_id, connection_id)?;
        Ok(WorkspaceProjectLockResponse {
            project_id: params.project_id,
        })
    }

    /// Owner-only release; a non-owner call is a no-op, never an error.
    pub(crate) async fn unlock(
        &self,
        params: WorkspaceProjectUnlockParams,
        connection_id: ConnectionId,
    ) -> Result<WorkspaceProjectUnlockResponse, JSONRPCErrorError> {
        self.write_lock.release(&params.project_id, connection_id);
        Ok(WorkspaceProjectUnlockResponse {
            project_id: params.project_id,
        })
    }

    pub(crate) async fn bind(
        &self,
        params: WorkspaceProjectBindParams,
    ) -> Result<WorkspaceProjectBindResponse, JSONRPCErrorError> {
        let project_id = params.id.clone();
        let result = self.with_store(|store| store.bind(&self.ody_home, params));
        match &result {
            Ok(response) => self.audit.record(audit_event(
                Some(&response.project.id),
                "project.bind",
                "ok",
                serde_json::json!({
                    "projectId": response.project.id,
                    "roots": response.project.roots.len(),
                }),
            )),
            Err(err) => self.audit.record(audit_event(
                Some(&project_id),
                "project.bind",
                "error",
                error_detail("project.bind", err),
            )),
        }
        result
    }

    pub(crate) async fn get(
        &self,
        params: WorkspaceProjectGetParams,
        connection_id: ConnectionId,
    ) -> Result<WorkspaceProjectGetResponse, JSONRPCErrorError> {
        self.with_store(|store| {
            store
                .projects
                .get(&params.project_id)
                .cloned()
                .map(|mut project| {
                    project.locked_by = self.write_lock.holder(&project.id).map(|c| c.0);
                    WorkspaceProjectGetResponse {
                        project,
                        caller_connection_id: connection_id.0,
                    }
                })
                .ok_or_else(|| unknown_project(&params.project_id))
        })
    }

    pub(crate) async fn list(
        &self,
        _params: WorkspaceProjectListParams,
        connection_id: ConnectionId,
    ) -> Result<WorkspaceProjectListResponse, JSONRPCErrorError> {
        self.with_store(|store| {
            let mut projects = store.projects.values().cloned().collect::<Vec<_>>();
            for project in &mut projects {
                project.locked_by = self.write_lock.holder(&project.id).map(|c| c.0);
            }
            projects.sort_by_key(|project| std::cmp::Reverse(project.updated_at_ms));
            Ok(WorkspaceProjectListResponse {
                projects,
                caller_connection_id: connection_id.0,
            })
        })
    }

    pub(crate) async fn close(
        &self,
        params: WorkspaceProjectCloseParams,
        connection_id: ConnectionId,
    ) -> Result<WorkspaceProjectCloseResponse, JSONRPCErrorError> {
        // E4 T05: a foreign holder must not lose the project underneath its
        // in-flight writes; the holder itself may close.
        self.write_lock.check(&params.project_id, connection_id)?;
        let project_id = params.project_id.clone();
        let result = self.with_store(|store| {
            let project = store
                .projects
                .remove(&params.project_id)
                .ok_or_else(|| unknown_project(&params.project_id))?;
            store.idempotency.retain(|_, id| id != &project.id);
            store.persist()?;
            Ok(WorkspaceProjectCloseResponse { project })
        });
        if result.is_ok() {
            // Closing releases the caller's own hold (if any) along with the
            // project record.
            self.write_lock.release(&project_id, connection_id);
        }
        match &result {
            Ok(_) => self.audit.record(audit_event(
                Some(&project_id),
                "project.close",
                "ok",
                serde_json::json!({"projectId": project_id}),
            )),
            Err(err) => self.audit.record(audit_event(
                Some(&project_id),
                "project.close",
                "error",
                error_detail("project.close", err),
            )),
        }
        result
    }

    pub(crate) async fn scan(
        &self,
        params: WorkspaceProjectScanParams,
    ) -> Result<WorkspaceProjectScanResponse, JSONRPCErrorError> {
        let project = self.with_store(|store| {
            store
                .projects
                .get(&params.project_id)
                .cloned()
                .ok_or_else(|| unknown_project(&params.project_id))
        })?;
        let mut roots = Vec::with_capacity(project.roots.len());
        let mut truncated = false;
        for root in &project.roots {
            let discovery = crate::workspace_discovery::scan_root(&root.path).await;
            truncated |= discovery.truncated;
            roots.push(discovery);
        }
        Ok(WorkspaceProjectScanResponse {
            discovery: WorkspaceDiscovery {
                project_id: project.id,
                roots,
                truncated,
                scanned_at_ms: now_ms(),
            },
        })
    }

    fn with_store<T>(
        &self,
        operation: impl FnOnce(&mut WorkspaceProjectStore) -> Result<T, JSONRPCErrorError>,
    ) -> Result<T, JSONRPCErrorError> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| internal_error("workspace project store lock poisoned"))?;
        operation(&mut store)
    }
}

impl WorkspaceProjectStore {
    fn load(path: PathBuf) -> Self {
        let backup = path.with_extension("json.bak");
        for candidate in [&path, &backup] {
            if let Ok(raw) = fs::read(candidate)
                && let Ok(mut state) = serde_json::from_slice::<Self>(&raw)
                && state.schema_version == WORKSPACE_PROJECT_PROTOCOL_VERSION
            {
                state.path = path.clone();
                return state;
            }
        }
        Self {
            schema_version: WORKSPACE_PROJECT_PROTOCOL_VERSION,
            path,
            ..Default::default()
        }
    }

    fn bind(
        &mut self,
        ody_home: &Path,
        params: WorkspaceProjectBindParams,
    ) -> Result<WorkspaceProjectBindResponse, JSONRPCErrorError> {
        validate_id(&params.id, "project id")?;
        validate_id(&params.idempotency_key, "idempotency key")?;
        if params.name.trim().is_empty() || params.name.len() > 200 {
            return Err(invalid_params(
                "workspace project name must be 1..=200 characters",
            ));
        }
        let idempotency_key = idempotency_key("bind", &params.idempotency_key);
        if let Some(existing_id) = self.idempotency.get(&idempotency_key) {
            let project = self.projects.get(existing_id).cloned().ok_or_else(|| {
                internal_error("workspace project idempotency record references a missing project")
            })?;
            if project.id != params.id {
                return Err(invalid_params(format!(
                    "idempotency key {} was already used for project {}",
                    params.idempotency_key, project.id
                )));
            }
            return Ok(WorkspaceProjectBindResponse { project });
        }
        if self.projects.contains_key(&params.id) {
            return Err(invalid_params(format!(
                "workspace project id {} is already bound",
                params.id
            )));
        }
        let roots = validate_roots(ody_home, &params.roots)?;
        let now = now_ms();
        let project = WorkspaceProjectRef {
            id: params.id.clone(),
            name: params.name,
            schema_version: WORKSPACE_PROJECT_PROTOCOL_VERSION,
            roots,
            created_at_ms: now,
            updated_at_ms: now,
            locked_by: None,
        };
        self.projects.insert(project.id.clone(), project.clone());
        self.idempotency.insert(idempotency_key, project.id.clone());
        self.persist()?;
        Ok(WorkspaceProjectBindResponse { project })
    }

    fn persist(&self) -> Result<(), JSONRPCErrorError> {
        let directory = self
            .path
            .parent()
            .ok_or_else(|| internal_error("workspace project store path has no parent"))?;
        fs::create_dir_all(directory).map_err(|error| {
            internal_error(format!("failed to create workspace project store: {error}"))
        })?;
        let serialized = serde_json::to_vec_pretty(self).map_err(|error| {
            internal_error(format!(
                "failed to serialize workspace project store: {error}"
            ))
        })?;
        let temporary = self
            .path
            .with_extension(format!("json.{}.tmp", Uuid::now_v7()));
        fs::write(&temporary, serialized).map_err(|error| {
            internal_error(format!("failed to write workspace project store: {error}"))
        })?;
        if self.path.exists() {
            let _ = fs::copy(&self.path, self.path.with_extension("json.bak"));
        }
        fs::rename(&temporary, &self.path).map_err(|error| {
            internal_error(format!("failed to commit workspace project store: {error}"))
        })
    }
}

fn validate_roots(
    ody_home: &Path,
    roots: &[AbsolutePathBuf],
) -> Result<Vec<WorkspaceRoot>, JSONRPCErrorError> {
    if roots.is_empty() {
        return Err(invalid_params(
            "workspace project requires at least one root",
        ));
    }
    if roots.len() > MAX_ROOTS {
        return Err(invalid_params(format!(
            "workspace project supports at most {MAX_ROOTS} roots, got {}",
            roots.len()
        )));
    }
    let ody_home_canonical = ody_home
        .canonicalize()
        .map_err(|error| internal_error(format!("failed to canonicalize ody home: {error}")))?;
    let mut canonical_roots: Vec<PathBuf> = Vec::with_capacity(roots.len());
    for root in roots {
        let canonical = root.canonicalize().map_err(|error| {
            invalid_params(format!(
                "workspace root {} is not accessible: {error}",
                root.display()
            ))
        })?;
        let canonical_path = canonical.as_path();
        if !canonical_path.is_dir() {
            return Err(invalid_params(format!(
                "workspace root {} is not a directory",
                canonical.display()
            )));
        }
        if canonical_path.parent().is_none() {
            return Err(invalid_params(format!(
                "workspace root {} is a filesystem root and cannot be bound",
                canonical.display()
            )));
        }
        if canonical_path == ody_home_canonical.as_path()
            || canonical_path.starts_with(ody_home_canonical.as_path())
        {
            return Err(invalid_params(format!(
                "workspace root {} is inside the Ody application data directory",
                canonical.display()
            )));
        }
        for previous in &canonical_roots {
            if canonical_path.starts_with(previous) || previous.starts_with(canonical_path) {
                return Err(invalid_params(format!(
                    "workspace roots {} and {} overlap",
                    previous.display(),
                    canonical.display()
                )));
            }
        }
        canonical_roots.push(canonical_path.to_path_buf());
    }
    Ok(canonical_roots
        .into_iter()
        .enumerate()
        .map(|(index, path)| WorkspaceRoot {
            path: path.to_string_lossy().into_owned(),
            role: if index == 0 {
                WorkspaceRootRole::Primary
            } else {
                WorkspaceRootRole::Secondary
            },
            auth_source: AUTH_SOURCE_USER_SELECTED.to_owned(),
        })
        .collect())
}

fn unknown_project(project_id: &str) -> JSONRPCErrorError {
    invalid_params(format!("unknown workspace project id: {project_id}"))
}

fn validate_id(value: &str, label: &str) -> Result<(), JSONRPCErrorError> {
    if value.is_empty()
        || value.len() > 200
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
    {
        return Err(invalid_params(format!(
            "{label} must be 1..=200 URL-safe characters"
        )));
    }
    Ok(())
}

fn idempotency_key(kind: &str, key: &str) -> String {
    format!("{kind}:{key}")
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> AbsolutePathBuf {
        let path = tempfile::tempdir()
            .expect("create root")
            .keep()
            .canonicalize()
            .expect("canonicalize root");
        ody_utils_absolute_path::test_support::PathBufExt::abs(&path)
    }

    #[test]
    fn store_corruption_falls_back_to_backup_then_empty_store() {
        let ody_home = tempfile::tempdir().expect("create ody home");
        let store_path = ody_home.path().join(STORE_FILE);
        fs::write(&store_path, b"{ not json").expect("write corrupt store");
        fs::write(store_path.with_extension("json.bak"), b"{ also not json")
            .expect("write corrupt backup");
        let store = WorkspaceProjectStore::load(store_path);
        assert!(store.projects.is_empty());
        assert_eq!(store.schema_version, WORKSPACE_PROJECT_PROTOCOL_VERSION);
    }

    #[test]
    fn bind_persists_and_load_recovers_project() {
        let ody_home = tempfile::tempdir().expect("create ody home");
        let root = temp_root();
        let store_path = ody_home.path().join(STORE_FILE);
        let mut store = WorkspaceProjectStore::load(store_path.clone());
        store
            .bind(
                ody_home.path(),
                WorkspaceProjectBindParams {
                    id: "ws-1".to_owned(),
                    name: "fixture".to_owned(),
                    roots: vec![root.clone()],
                    idempotency_key: "key-1".to_owned(),
                },
            )
            .expect("bind project");
        drop(store);

        let reloaded = WorkspaceProjectStore::load(store_path);
        let project = reloaded
            .projects
            .get("ws-1")
            .expect("project survives reload");
        assert_eq!(project.roots[0].path, root.to_string_lossy());
    }

    #[tokio::test]
    async fn get_and_list_report_caller_connection_id() {
        let ody_home = tempfile::tempdir().expect("create ody home");
        let audit = Arc::new(crate::workspace_audit::WorkspaceAuditLog::new(
            ody_home.path().join("workspace-audit").join("v1.jsonl"),
        ));
        let write_lock = Arc::new(crate::workspace_lock::WorkspaceWriteLock::default());
        let processor = WorkspaceProjectRequestProcessor::new(
            ody_home.path().to_path_buf(),
            audit,
            write_lock.clone(),
        );
        processor
            .bind(WorkspaceProjectBindParams {
                id: "ws-1".to_owned(),
                name: "fixture".to_owned(),
                roots: vec![temp_root()],
                idempotency_key: "key-1".to_owned(),
            })
            .await
            .expect("bind project");

        let caller = ConnectionId(7);
        let got = processor
            .get(
                WorkspaceProjectGetParams {
                    project_id: "ws-1".to_owned(),
                },
                caller,
            )
            .await
            .expect("get project");
        assert_eq!(got.caller_connection_id, 7);
        assert_eq!(got.project.locked_by, None);

        // Same caller takes the write lock: it must recognize itself as the
        // holder; a foreign caller must see the same `locked_by` but can
        // still compare it against its own `caller_connection_id`.
        processor
            .lock(
                WorkspaceProjectLockParams {
                    project_id: "ws-1".to_owned(),
                },
                caller,
            )
            .await
            .expect("lock project");
        let own = processor
            .get(
                WorkspaceProjectGetParams {
                    project_id: "ws-1".to_owned(),
                },
                caller,
            )
            .await
            .expect("get project");
        assert_eq!(own.project.locked_by, Some(caller.0));
        let foreign = processor
            .list(WorkspaceProjectListParams {}, ConnectionId(99))
            .await
            .expect("list projects");
        assert_eq!(foreign.caller_connection_id, 99);
        let listed = &foreign.projects[0];
        assert_eq!(listed.locked_by, Some(caller.0));
        assert_ne!(listed.locked_by.unwrap(), foreign.caller_connection_id);
        drop(write_lock);
    }

    #[test]
    fn bind_rejects_overlapping_roots() {
        let ody_home = tempfile::tempdir().expect("create ody home");
        let parent = temp_root();
        let child = parent.join("child");
        fs::create_dir_all(&child).expect("create child root");
        let mut store = WorkspaceProjectStore::load(ody_home.path().join(STORE_FILE));
        let error = store
            .bind(
                ody_home.path(),
                WorkspaceProjectBindParams {
                    id: "ws-1".to_owned(),
                    name: "fixture".to_owned(),
                    roots: vec![parent, child],
                    idempotency_key: "key-1".to_owned(),
                },
            )
            .expect_err("overlapping roots must be rejected");
        assert!(error.message.contains("overlap"), "{}", error.message);
    }

    #[test]
    fn bind_rejects_ody_home_and_filesystem_root() {
        let ody_home = tempfile::tempdir().expect("create ody home");
        let mut store = WorkspaceProjectStore::load(ody_home.path().join(STORE_FILE));
        let error = store
            .bind(
                ody_home.path(),
                WorkspaceProjectBindParams {
                    id: "ws-1".to_owned(),
                    name: "fixture".to_owned(),
                    roots: vec![ody_utils_absolute_path::test_support::PathBufExt::abs(
                        &ody_home.path().to_path_buf(),
                    )],
                    idempotency_key: "key-1".to_owned(),
                },
            )
            .expect_err("ody home must be rejected");
        assert!(
            error.message.contains("application data directory"),
            "{}",
            error.message
        );

        #[cfg(unix)]
        {
            let error = store
                .bind(
                    ody_home.path(),
                    WorkspaceProjectBindParams {
                        id: "ws-2".to_owned(),
                        name: "fixture".to_owned(),
                        roots: vec![ody_utils_absolute_path::test_support::PathBufExt::abs(
                            &PathBuf::from("/"),
                        )],
                        idempotency_key: "key-2".to_owned(),
                    },
                )
                .expect_err("filesystem root must be rejected");
            assert!(
                error.message.contains("filesystem root"),
                "{}",
                error.message
            );
        }
    }
}
