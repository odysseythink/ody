use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use ody_app_server_protocol::JSONRPCErrorError;
use ody_app_server_protocol::WORKSPACE_SOURCE_PROTOCOL_VERSION;
use ody_app_server_protocol::WorkspaceChangeSet;
use ody_app_server_protocol::WorkspaceChangeSetApplyParams;
use ody_app_server_protocol::WorkspaceChangeSetApplyResponse;
use ody_app_server_protocol::WorkspaceChangeSetCheckpoint;
use ody_app_server_protocol::WorkspaceChangeSetCreateParams;
use ody_app_server_protocol::WorkspaceChangeSetCreateResponse;
use ody_app_server_protocol::WorkspaceChangeSetDiffEntry;
use ody_app_server_protocol::WorkspaceChangeSetGetParams;
use ody_app_server_protocol::WorkspaceChangeSetGetResponse;
use ody_app_server_protocol::WorkspaceChangeSetListParams;
use ody_app_server_protocol::WorkspaceChangeSetListResponse;
use ody_app_server_protocol::WorkspaceChangeSetRejectParams;
use ody_app_server_protocol::WorkspaceChangeSetRejectResponse;
use ody_app_server_protocol::WorkspaceChangeSetRestoreParams;
use ody_app_server_protocol::WorkspaceChangeSetRestoreResponse;
use ody_app_server_protocol::WorkspaceChangeSetStatus;
use ody_app_server_protocol::WorkspaceFileChangeKind;
use ody_app_server_protocol::WorkspaceGitDiff;
use ody_app_server_protocol::WorkspaceProjectRef;
use ody_app_server_protocol::WorkspaceSourceDiffParams;
use ody_app_server_protocol::WorkspaceSourceDiffResponse;
use ody_app_server_protocol::WorkspaceSourceIndexParams;
use ody_app_server_protocol::WorkspaceSourceIndexResponse;
use ody_app_server_protocol::WorkspaceSourceQueryKind;
use ody_app_server_protocol::WorkspaceSourceResolveMatch;
use ody_app_server_protocol::WorkspaceSourceResolveParams;
use ody_app_server_protocol::WorkspaceSourceResolveResponse;
use serde::Deserialize;
use serde::Serialize;

use crate::error_code::internal_error;
use crate::error_code::invalid_params;
use crate::request_processors::workspace_project_processor::WorkspaceProjectStore;
use crate::workspace_changeset::PreparedChange;

const RESOLVE_DEFAULT_LIMIT: usize = 20;
const RESOLVE_MAX_LIMIT: usize = 100;

const STORE_DIR: &str = "workspace-source";
const STORE_FILE: &str = "v1.json";

/// `git diff HEAD` timeout for the whole-repo diff channel.
const GIT_DIFF_TIMEOUT_MS: u64 = 5_000;

#[derive(Clone)]
pub(crate) struct WorkspaceSourceRequestProcessor {
    project_store: Arc<Mutex<WorkspaceProjectStore>>,
    store: Arc<Mutex<WorkspaceSourceStore>>,
}

impl WorkspaceSourceRequestProcessor {
    pub(crate) fn new(ody_home: PathBuf, project_store: Arc<Mutex<WorkspaceProjectStore>>) -> Self {
        let path = ody_home.join(STORE_DIR).join(STORE_FILE);
        let store = WorkspaceSourceStore::load(path);
        Self {
            project_store,
            store: Arc::new(Mutex::new(store)),
        }
    }

    pub(crate) async fn index(
        &self,
        params: WorkspaceSourceIndexParams,
    ) -> Result<WorkspaceSourceIndexResponse, JSONRPCErrorError> {
        let project = self.project(&params.project_id)?;
        let index = crate::workspace_source_index::index_project(&project).await;
        Ok(WorkspaceSourceIndexResponse { index })
    }

    pub(crate) async fn resolve(
        &self,
        params: WorkspaceSourceResolveParams,
    ) -> Result<WorkspaceSourceResolveResponse, JSONRPCErrorError> {
        let project = self.project(&params.project_id)?;
        let value = params.value.trim();
        if value.is_empty() {
            return Err(invalid_params("resolve value must not be empty"));
        }
        let limit = params
            .limit
            .map(|v| v as usize)
            .unwrap_or(RESOLVE_DEFAULT_LIMIT)
            .clamp(1, RESOLVE_MAX_LIMIT);
        let index = crate::workspace_source_index::index_project(&project).await;
        let needle = value.to_lowercase();
        let mut matches = Vec::new();
        for (artifact, source_ref) in index.artifacts.iter().zip(index.refs.iter()) {
            let hit = match params.kind {
                WorkspaceSourceQueryKind::Name => artifact.name.to_lowercase() == needle,
                WorkspaceSourceQueryKind::Symbol => source_ref.symbol.to_lowercase() == needle,
                WorkspaceSourceQueryKind::RoutePath => {
                    artifact.route_path.as_deref() == Some(value)
                        || source_ref.route_path.as_deref() == Some(value)
                }
            };
            if hit {
                matches.push(WorkspaceSourceResolveMatch {
                    artifact: artifact.clone(),
                    source_ref: source_ref.clone(),
                });
                if matches.len() >= limit {
                    break;
                }
            }
        }
        Ok(WorkspaceSourceResolveResponse { matches })
    }

    pub(crate) async fn changeset_create(
        &self,
        params: WorkspaceChangeSetCreateParams,
    ) -> Result<WorkspaceChangeSetCreateResponse, JSONRPCErrorError> {
        let idem_key = format!("changeset:{}", params.idempotency_key);
        // Idempotent retry: return the existing record before any validation.
        if let Some(existing) = self.with_store(|store| {
            Ok(store
                .idempotency
                .get(&idem_key)
                .and_then(|id| store.changesets.get(id))
                .map(|stored| stored.change_set.clone()))
        })? {
            return Ok(WorkspaceChangeSetCreateResponse { changeset: existing });
        }
        let project = self.project(&params.project_id)?;
        let title = params.title.trim();
        if title.is_empty() || title.len() > 200 {
            return Err(invalid_params("title must be 1..=200 characters"));
        }
        let prepared = crate::workspace_changeset::prepare_changes(&project, &params.changes)?;
        let unified_diff = crate::workspace_changeset::render_changeset_diff(&prepared);
        let id = format!("cs-{}", uuid::Uuid::new_v4());
        let now = now_ms();
        let stored = StoredChangeSet {
            change_set: WorkspaceChangeSet {
                id: id.clone(),
                project_id: project.id.clone(),
                title: title.to_owned(),
                schema_version: WORKSPACE_SOURCE_PROTOCOL_VERSION,
                changes: params.changes,
                status: WorkspaceChangeSetStatus::Pending,
                checkpoint: WorkspaceChangeSetCheckpoint::None,
                unified_diff,
                created_at_ms: now,
                updated_at_ms: now,
                applied_at_ms: None,
            },
            base_contents: prepared
                .iter()
                .filter_map(|p| p.base_content.clone().map(|b| (p.key.clone(), b)))
                .collect(),
            applied_hashes: BTreeMap::new(),
            targets: prepared
                .iter()
                .map(|p| (p.key.clone(), p.absolute.to_string_lossy().into_owned()))
                .collect(),
        };
        self.with_store(|store| {
            store.changesets.insert(id.clone(), stored);
            store.idempotency.insert(idem_key, id.clone());
            store.persist()?;
            Ok(WorkspaceChangeSetCreateResponse {
                changeset: store
                    .changesets
                    .get(&id)
                    .expect("just inserted")
                    .change_set
                    .clone(),
            })
        })
    }

    pub(crate) async fn changeset_get(
        &self,
        params: WorkspaceChangeSetGetParams,
    ) -> Result<WorkspaceChangeSetGetResponse, JSONRPCErrorError> {
        let (_, stored) = self.stored_changeset(&params.changeset_id)?;
        Ok(WorkspaceChangeSetGetResponse {
            changeset: stored.change_set,
        })
    }

    pub(crate) async fn changeset_list(
        &self,
        params: WorkspaceChangeSetListParams,
    ) -> Result<WorkspaceChangeSetListResponse, JSONRPCErrorError> {
        let project = self.project(&params.project_id)?;
        let mut changesets = self.with_store(|store| {
            Ok(store
                .changesets
                .values()
                .filter(|stored| stored.change_set.project_id == project.id)
                .map(|stored| stored.change_set.clone())
                .collect::<Vec<_>>())
        })?;
        changesets.sort_by(|a, b| b.created_at_ms.cmp(&a.created_at_ms));
        Ok(WorkspaceChangeSetListResponse { changesets })
    }

    pub(crate) async fn changeset_apply(
        &self,
        params: WorkspaceChangeSetApplyParams,
    ) -> Result<WorkspaceChangeSetApplyResponse, JSONRPCErrorError> {
        let (project, mut stored) = self.stored_changeset(&params.changeset_id)?;
        if stored.change_set.status != WorkspaceChangeSetStatus::Pending {
            return Err(invalid_params(format!(
                "changeset {} is {:?}; expected Pending for apply",
                stored.change_set.id, stored.change_set.status
            )));
        }
        let prepared = rebuild_prepared(&project, &stored)?;
        // Phase 1: verify every file before any write.
        let mut failures = Vec::new();
        for change in &prepared {
            if let Err(message) = crate::workspace_changeset::verify_apply_state(change, true) {
                failures.push(message);
            }
        }
        if !failures.is_empty() {
            return Err(invalid_params(format!(
                "changeset {} cannot be applied: {}",
                stored.change_set.id,
                failures.join("; ")
            )));
        }
        // Phase 2: checkpoint per affected root before first write.
        let mut checkpoint = WorkspaceChangeSetCheckpoint::None;
        let affected_roots: std::collections::BTreeSet<usize> =
            prepared.iter().map(|p| p.root_index).collect();
        for root_index in affected_roots {
            let root_path = &project.roots[root_index].path;
            match ody_git_utils::get_git_repo_root(Path::new(root_path)) {
                Some(_) => {
                    // Git project: preserve user .git; record HEAD best-effort.
                    let head = ody_git_utils::get_head_commit_hash(Path::new(root_path))
                        .await
                        .map(|sha| sha.0);
                    checkpoint = WorkspaceChangeSetCheckpoint::Git { head_commit_hash: head };
                }
                None => {
                    ody_git_utils::ensure_git_baseline_repository(Path::new(root_path))
                        .await
                        .map_err(|err| {
                            internal_error(format!("checkpoint failed for {root_path}: {err}"))
                        })?;
                    checkpoint = WorkspaceChangeSetCheckpoint::Git { head_commit_hash: None };
                }
            }
        }
        // Phase 3: write.
        let mut applied_hashes = BTreeMap::new();
        let mut completed: Vec<String> = Vec::new();
        for change in &prepared {
            let result = match change.kind {
                WorkspaceFileChangeKind::Add | WorkspaceFileChangeKind::Update => {
                    crate::workspace_changeset::atomic_write(
                        &change.absolute,
                        change.content.as_deref().unwrap_or_default(),
                    )
                }
                WorkspaceFileChangeKind::Delete => fs::remove_file(&change.absolute),
            };
            if let Err(err) = result {
                let done = if completed.is_empty() {
                    "no files written".to_owned()
                } else {
                    format!("already written: {}", completed.join(", "))
                };
                return Err(internal_error(format!(
                    "changeset {} write failed at {} ({err}); {done}. Re-apply is safe: verified files are skipped.",
                    stored.change_set.id, change.key
                )));
            }
            completed.push(change.key.clone());
            applied_hashes.insert(
                change.key.clone(),
                match change.kind {
                    WorkspaceFileChangeKind::Delete => String::new(),
                    _ => crate::workspace_changeset::sha256_hex(
                        change.content.as_deref().unwrap_or_default().as_bytes(),
                    ),
                },
            );
        }
        // Phase 4: record.
        stored.change_set.status = WorkspaceChangeSetStatus::Applied;
        stored.change_set.checkpoint = checkpoint;
        stored.change_set.applied_at_ms = Some(now_ms());
        stored.change_set.updated_at_ms = now_ms();
        stored.applied_hashes = applied_hashes;
        let change_set = self.with_store(|store| {
            store
                .changesets
                .insert(stored.change_set.id.clone(), stored);
            store.persist()?;
            Ok(store
                .changesets
                .get(&params.changeset_id)
                .expect("just inserted")
                .change_set
                .clone())
        })?;
        Ok(WorkspaceChangeSetApplyResponse { changeset: change_set })
    }

    pub(crate) async fn changeset_reject(
        &self,
        params: WorkspaceChangeSetRejectParams,
    ) -> Result<WorkspaceChangeSetRejectResponse, JSONRPCErrorError> {
        let (_, mut stored) = self.stored_changeset(&params.changeset_id)?;
        if stored.change_set.status != WorkspaceChangeSetStatus::Pending {
            return Err(invalid_params(format!(
                "changeset {} is {:?}; expected Pending for reject",
                stored.change_set.id, stored.change_set.status
            )));
        }
        stored.change_set.status = WorkspaceChangeSetStatus::Rejected;
        stored.change_set.updated_at_ms = now_ms();
        let change_set = self.with_store(|store| {
            store
                .changesets
                .insert(stored.change_set.id.clone(), stored);
            store.persist()?;
            Ok(store
                .changesets
                .get(&params.changeset_id)
                .expect("just inserted")
                .change_set
                .clone())
        })?;
        Ok(WorkspaceChangeSetRejectResponse { changeset: change_set })
    }

    pub(crate) async fn changeset_restore(
        &self,
        params: WorkspaceChangeSetRestoreParams,
    ) -> Result<WorkspaceChangeSetRestoreResponse, JSONRPCErrorError> {
        let (project, mut stored) = self.stored_changeset(&params.changeset_id)?;
        if stored.change_set.status != WorkspaceChangeSetStatus::Applied {
            return Err(invalid_params(format!(
                "changeset {} is {:?}; expected Applied for restore",
                stored.change_set.id, stored.change_set.status
            )));
        }
        let prepared = rebuild_prepared(&project, &stored)?;
        // Verify every file is exactly as apply left it before any write.
        let mut failures = Vec::new();
        for change in &prepared {
            if let Err(message) = crate::workspace_changeset::verify_restore_state(change) {
                failures.push(message);
            }
        }
        if !failures.is_empty() {
            return Err(invalid_params(format!(
                "changeset {} cannot be restored: {}",
                stored.change_set.id,
                failures.join("; ")
            )));
        }
        for change in &prepared {
            let result = match change.kind {
                WorkspaceFileChangeKind::Add => fs::remove_file(&change.absolute),
                WorkspaceFileChangeKind::Update | WorkspaceFileChangeKind::Delete => {
                    let base = stored
                        .base_contents
                        .get(&change.key)
                        .cloned()
                        .unwrap_or_default();
                    crate::workspace_changeset::atomic_write(&change.absolute, &base)
                }
            };
            if let Err(err) = result {
                return Err(internal_error(format!(
                    "changeset {} restore failed at {}: {err}",
                    stored.change_set.id, change.key
                )));
            }
        }
        stored.change_set.status = WorkspaceChangeSetStatus::Restored;
        stored.change_set.updated_at_ms = now_ms();
        let change_set = self.with_store(|store| {
            store
                .changesets
                .insert(stored.change_set.id.clone(), stored);
            store.persist()?;
            Ok(store
                .changesets
                .get(&params.changeset_id)
                .expect("just inserted")
                .change_set
                .clone())
        })?;
        Ok(WorkspaceChangeSetRestoreResponse { changeset: change_set })
    }

    pub(crate) async fn diff(
        &self,
        params: WorkspaceSourceDiffParams,
    ) -> Result<WorkspaceSourceDiffResponse, JSONRPCErrorError> {
        let project = self.project(&params.project_id)?;
        let changesets = self.with_store(|store| {
            Ok(store
                .changesets
                .values()
                .filter(|stored| stored.change_set.project_id == params.project_id)
                .filter(|stored| stored.change_set.status != WorkspaceChangeSetStatus::Rejected)
                .map(|stored| WorkspaceChangeSetDiffEntry {
                    id: stored.change_set.id.clone(),
                    status: stored.change_set.status,
                    unified_diff: stored.change_set.unified_diff.clone(),
                })
                .collect::<Vec<_>>())
        })?;
        let git_diff = project_git_diff(&project).await;
        Ok(WorkspaceSourceDiffResponse {
            changesets,
            git_diff,
        })
    }

    fn project(&self, project_id: &str) -> Result<WorkspaceProjectRef, JSONRPCErrorError> {
        let store = self.project_store.lock().map_err(|_| {
            crate::error_code::internal_error("workspace project store lock poisoned")
        })?;
        store
            .projects
            .get(project_id)
            .cloned()
            .ok_or_else(|| unknown_project(project_id))
    }

    fn stored_changeset(
        &self,
        changeset_id: &str,
    ) -> Result<(WorkspaceProjectRef, StoredChangeSet), JSONRPCErrorError> {
        let stored = self.with_store(|store| {
            store
                .changesets
                .get(changeset_id)
                .cloned()
                .ok_or_else(|| invalid_params(format!("unknown changeset id: {changeset_id}")))
        })?;
        let project = self.project(&stored.change_set.project_id)?;
        Ok((project, stored))
    }

    fn with_store<T>(
        &self,
        operation: impl FnOnce(&mut WorkspaceSourceStore) -> Result<T, JSONRPCErrorError>,
    ) -> Result<T, JSONRPCErrorError> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| internal_error("workspace source store lock poisoned"))?;
        operation(&mut store)
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct StoredChangeSet {
    #[serde(flatten)]
    pub change_set: WorkspaceChangeSet,
    /// "{root_index}:{path}" -> base content captured at create.
    pub base_contents: BTreeMap<String, String>,
    /// "{root_index}:{path}" -> sha256 after apply.
    pub applied_hashes: BTreeMap<String, String>,
    /// "{root_index}:{path}" -> resolved absolute target.
    pub targets: BTreeMap<String, String>,
}

#[derive(Default, Serialize, Deserialize)]
pub(crate) struct WorkspaceSourceStore {
    schema_version: u32,
    changesets: BTreeMap<String, StoredChangeSet>,
    /// Namespaced idempotency key ("changeset:{key}") -> changeset id.
    idempotency: BTreeMap<String, String>,
    #[serde(skip)]
    path: PathBuf,
}

impl WorkspaceSourceStore {
    fn load(path: PathBuf) -> Self {
        let backup = path.with_extension("json.bak");
        for candidate in [&path, &backup] {
            if let Ok(raw) = fs::read(candidate)
                && let Ok(mut state) = serde_json::from_slice::<Self>(&raw)
                && state.schema_version == WORKSPACE_SOURCE_PROTOCOL_VERSION
            {
                state.path = path.clone();
                return state;
            }
        }
        Self {
            schema_version: WORKSPACE_SOURCE_PROTOCOL_VERSION,
            path,
            ..Default::default()
        }
    }

    fn persist(&self) -> Result<(), JSONRPCErrorError> {
        let directory = self
            .path
            .parent()
            .ok_or_else(|| internal_error("workspace source store path has no parent"))?;
        fs::create_dir_all(directory).map_err(|error| {
            internal_error(format!("failed to create workspace source store: {error}"))
        })?;
        let serialized = serde_json::to_vec_pretty(self).map_err(|error| {
            internal_error(format!("failed to serialize workspace source store: {error}"))
        })?;
        let temporary = self
            .path
            .with_extension(format!("json.{}.tmp", uuid::Uuid::now_v7()));
        fs::write(&temporary, serialized).map_err(|error| {
            internal_error(format!("failed to write workspace source store: {error}"))
        })?;
        if self.path.exists() {
            let _ = fs::copy(&self.path, self.path.with_extension("json.bak"));
        }
        fs::rename(&temporary, &self.path).map_err(|error| {
            internal_error(format!("failed to commit workspace source store: {error}"))
        })
    }
}

/// Rebuild prepared changes from the persisted record: re-resolve each
/// target (catching on-disk structure changes since create) and pair it
/// with the stored base content. Never re-reads base content from disk.
fn rebuild_prepared(
    project: &WorkspaceProjectRef,
    stored: &StoredChangeSet,
) -> Result<Vec<PreparedChange>, JSONRPCErrorError> {
    let mut prepared = Vec::with_capacity(stored.change_set.changes.len());
    for change in &stored.change_set.changes {
        let normalized = crate::workspace_changeset::normalize_relative(&change.path)?;
        let key = format!("{}:{normalized}", change.root_index);
        let root = project
            .roots
            .get(change.root_index as usize)
            .ok_or_else(|| invalid_params(format!("unknown root index {}", change.root_index)))?;
        let absolute = crate::workspace_changeset::resolve_target(Path::new(&root.path), &normalized)?;
        let recorded = stored.targets.get(&key).map(String::as_str);
        if recorded != Some(absolute.to_string_lossy().as_ref()) {
            return Err(invalid_params(format!(
                "path {key} changed on disk; re-create the changeset"
            )));
        }
        let key_for_prepared = key.clone();
        prepared.push(PreparedChange {
            key: key_for_prepared,
            root_index: change.root_index as usize,
            root_path: PathBuf::from(&root.path),
            relative: normalized,
            absolute,
            kind: change.kind,
            base_hash: change.base_hash.clone(),
            content: change.content.clone(),
            base_content: stored.base_contents.get(&key).cloned(),
        });
    }
    Ok(prepared)
}

async fn project_git_diff(project: &WorkspaceProjectRef) -> Option<WorkspaceGitDiff> {
    let primary = project.roots.first()?;
    let repo_root = ody_git_utils::get_git_repo_root(Path::new(&primary.path))?;
    let result = tokio::time::timeout(
        Duration::from_millis(GIT_DIFF_TIMEOUT_MS),
        tokio::process::Command::new("git")
            .args(["diff", "HEAD"])
            .current_dir(&repo_root)
            .output(),
    )
    .await;
    match result {
        Ok(Ok(output)) if output.status.success() => Some(WorkspaceGitDiff {
            repo_root: Some(repo_root.to_string_lossy().into_owned()),
            available: true,
            error: None,
            unified_diff: Some(String::from_utf8_lossy(&output.stdout).into_owned()),
        }),
        Ok(Ok(output)) => Some(WorkspaceGitDiff {
            repo_root: Some(repo_root.to_string_lossy().into_owned()),
            available: false,
            error: Some(format!("git diff exited with {}", output.status)),
            unified_diff: None,
        }),
        Ok(Err(err)) => Some(WorkspaceGitDiff {
            repo_root: Some(repo_root.to_string_lossy().into_owned()),
            available: false,
            error: Some(format!("git diff failed to run: {err}")),
            unified_diff: None,
        }),
        Err(_) => Some(WorkspaceGitDiff {
            repo_root: Some(repo_root.to_string_lossy().into_owned()),
            available: false,
            error: Some(format!("git diff timed out after {GIT_DIFF_TIMEOUT_MS}ms")),
            unified_diff: None,
        }),
    }
}

fn unknown_project(id: &str) -> JSONRPCErrorError {
    invalid_params(format!("unknown workspace project id: {id}"))
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
    use ody_app_server_protocol::WorkspaceFileChange;
    use ody_app_server_protocol::WorkspaceRoot;
    use ody_app_server_protocol::WorkspaceRootRole;

    #[test]
    fn store_corruption_falls_back_to_backup_then_empty_store() {
        let ody_home = tempfile::tempdir().expect("create ody home");
        let store_path = ody_home.path().join(STORE_FILE);
        fs::write(&store_path, b"{ not json").expect("write corrupt store");
        fs::write(store_path.with_extension("json.bak"), b"{ also not json")
            .expect("write corrupt backup");
        let store = WorkspaceSourceStore::load(store_path);
        assert!(store.changesets.is_empty());
        assert_eq!(store.schema_version, WORKSPACE_SOURCE_PROTOCOL_VERSION);
    }

    #[test]
    fn changeset_create_persists_and_reloads() {
        let root = tempfile::tempdir()
            .expect("create root")
            .keep()
            .canonicalize()
            .expect("canonicalize");
        let pages = root.join("src/pages");
        fs::create_dir_all(&pages).expect("create pages");
        fs::write(pages.join("HomePage.tsx"), "export default function HomePage() {}\n")
            .expect("write page");

        let project = WorkspaceProjectRef {
            id: "ws-test".to_owned(),
            name: "test".to_owned(),
            schema_version: 1,
            roots: vec![WorkspaceRoot {
                path: root.to_string_lossy().into_owned(),
                role: WorkspaceRootRole::Primary,
                auth_source: "user_selected".to_owned(),
            }],
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        let base = fs::read_to_string(pages.join("HomePage.tsx")).expect("read base");
        let prepared = crate::workspace_changeset::prepare_changes(
            &project,
            &[WorkspaceFileChange {
                root_index: 0,
                path: "src/pages/HomePage.tsx".to_owned(),
                kind: WorkspaceFileChangeKind::Update,
                base_hash: Some(crate::workspace_changeset::sha256_hex(base.as_bytes())),
                content: Some("export default function HomePage2() {}\n".to_owned()),
            }],
        )
        .expect("prepare changes");
        assert_eq!(prepared.len(), 1);
        let id = "cs-test-1".to_owned();
        let stored = StoredChangeSet {
            change_set: WorkspaceChangeSet {
                id: id.clone(),
                project_id: project.id.clone(),
                title: "t".to_owned(),
                schema_version: WORKSPACE_SOURCE_PROTOCOL_VERSION,
                changes: vec![WorkspaceFileChange {
                    root_index: 0,
                    path: "src/pages/HomePage.tsx".to_owned(),
                    kind: WorkspaceFileChangeKind::Update,
                    base_hash: prepared[0].base_hash.clone(),
                    content: Some("export default function HomePage2() {}\n".to_owned()),
                }],
                status: WorkspaceChangeSetStatus::Pending,
                checkpoint: WorkspaceChangeSetCheckpoint::None,
                unified_diff: crate::workspace_changeset::render_changeset_diff(&prepared),
                created_at_ms: 1,
                updated_at_ms: 1,
                applied_at_ms: None,
            },
            base_contents: prepared
                .iter()
                .map(|p| (p.key.clone(), p.base_content.clone().unwrap()))
                .collect(),
            applied_hashes: BTreeMap::new(),
            targets: prepared
                .iter()
                .map(|p| (p.key.clone(), p.absolute.to_string_lossy().into_owned()))
                .collect(),
        };

        let store_path = root.join("store/v1.json");
        {
            let mut store = WorkspaceSourceStore::load(store_path.clone());
            store.changesets.insert(id.clone(), stored);
            store.persist().expect("persist store");
        }
        let reloaded = WorkspaceSourceStore::load(store_path);
        let reloaded = reloaded.changesets.get(&id).expect("reloaded changeset");
        assert_eq!(reloaded.change_set.status, WorkspaceChangeSetStatus::Pending);
        assert_eq!(reloaded.base_contents.len(), 1);
        let target = reloaded.targets.values().next().expect("target");
        assert!(target.starts_with(root.to_string_lossy().as_ref()));
        let rebuilt = rebuild_prepared(&project, reloaded).expect("rebuild prepared");
        assert_eq!(rebuilt.len(), 1);
        assert_eq!(rebuilt[0].base_content.as_deref(), Some(base.as_str()));
    }
}
