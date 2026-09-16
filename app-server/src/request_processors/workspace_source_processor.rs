use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use ody_app_server_protocol::JSONRPCErrorError;
use ody_app_server_protocol::WORKSPACE_SOURCE_PROTOCOL_VERSION;
use ody_app_server_protocol::WorkspaceArtifactBridge;
use ody_app_server_protocol::WorkspaceArtifactBridgeListParams;
use ody_app_server_protocol::WorkspaceArtifactBridgeListResponse;
use ody_app_server_protocol::WorkspaceArtifactBridgeParams;
use ody_app_server_protocol::WorkspaceArtifactBridgeResponse;
use ody_app_server_protocol::WorkspaceArtifactBridgeStatus;
use ody_app_server_protocol::WorkspaceChangeSet;
use ody_app_server_protocol::WorkspaceChangeSetApplyParams;
use ody_app_server_protocol::WorkspaceChangeSetApplyResponse;
use ody_app_server_protocol::WorkspaceChangeSetCheckpoint;
use ody_app_server_protocol::WorkspaceChangeSetCommitOutcome;
use ody_app_server_protocol::WorkspaceChangeSetCommitReport;
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
use ody_app_server_protocol::WorkspaceFileChange;
use ody_app_server_protocol::WorkspaceFileChangeKind;
use ody_app_server_protocol::WorkspaceFileEvent;
use ody_app_server_protocol::WorkspaceFileEventKind;
use ody_app_server_protocol::WorkspaceGitDiff;
use ody_app_server_protocol::WorkspaceProjectRef;
use ody_app_server_protocol::WorkspaceRootGitDiff;
use ody_app_server_protocol::WorkspaceSourceDiffParams;
use ody_app_server_protocol::WorkspaceSourceDiffResponse;
use ody_app_server_protocol::WorkspaceSourceIndexParams;
use ody_app_server_protocol::WorkspaceSourceIndexResponse;
use ody_app_server_protocol::WorkspaceSourceQueryKind;
use ody_app_server_protocol::WorkspaceSourceResolveMatch;
use ody_app_server_protocol::WorkspaceSourceResolveParams;
use ody_app_server_protocol::WorkspaceSourceResolveResponse;
use ody_app_server_protocol::WorkspaceSourceValidateParams;
use ody_app_server_protocol::WorkspaceSourceValidateResponse;
use ody_app_server_protocol::WorkspaceValidationCheck;
use serde::Deserialize;
use serde::Serialize;

use crate::error_code::internal_error;
use crate::error_code::invalid_params;
use crate::outgoing_message::ConnectionId;
use crate::request_processors::workspace_project_processor::WorkspaceProjectStore;
use crate::workspace_audit::audit_event;
use crate::workspace_audit::error_detail;
use crate::workspace_audit::WorkspaceAuditLog;
use crate::workspace_lock::WorkspaceWriteLock;
use crate::workspace_changeset::PreparedChange;

const RESOLVE_DEFAULT_LIMIT: usize = 20;
const RESOLVE_MAX_LIMIT: usize = 100;

const STORE_DIR: &str = "workspace-source";
const STORE_FILE: &str = "v1.json";

/// Standalone artifact sources (self-contained HTML today) are bounded so a
/// bridge call stays a protocol message, not a bulk transfer channel.
const MAX_ARTIFACT_BRIDGE_CONTENT_BYTES: usize = 2 * 1024 * 1024;

/// `git diff HEAD` timeout for the whole-repo diff channel.
const GIT_DIFF_TIMEOUT_MS: u64 = 5_000;

#[derive(Clone)]
pub(crate) struct WorkspaceSourceRequestProcessor {
    project_store: Arc<Mutex<WorkspaceProjectStore>>,
    store: Arc<Mutex<WorkspaceSourceStore>>,
    audit: Arc<WorkspaceAuditLog>,
    write_lock: Arc<WorkspaceWriteLock>,
    /// P1 切片1 暂存收集器：core 写通知 -> 可逆 changeset；同时供外部
    /// watcher 冲突检测排除暂存写盘自身（recent_staged）。
    staging: Arc<crate::workspace_staging::WorkspaceStagingCollector>,
}

impl WorkspaceSourceRequestProcessor {
    pub(crate) fn new(
        ody_home: PathBuf,
        project_store: Arc<Mutex<WorkspaceProjectStore>>,
        audit: Arc<WorkspaceAuditLog>,
        write_lock: Arc<WorkspaceWriteLock>,
    ) -> Self {
        let path = ody_home.join(STORE_DIR).join(STORE_FILE);
        let store = Arc::new(Mutex::new(WorkspaceSourceStore::load(path)));
        let staging = Arc::new(crate::workspace_staging::WorkspaceStagingCollector::new(
            Arc::clone(&project_store),
            Arc::clone(&store),
            Arc::clone(&audit),
        ));
        Self {
            project_store,
            store,
            audit,
            write_lock,
            staging,
        }
    }

    /// 暂存收集器句柄：app 启动接线时注入 core thread（见 message_processor）。
    pub(crate) fn staging_handle(&self) -> Arc<crate::workspace_staging::WorkspaceStagingCollector> {
        Arc::clone(&self.staging)
    }

    /// 以暂存收集器作为 core sink（`Arc<dyn StagedWriteSink>`）。
    pub(crate) fn staged_write_sink(
        &self,
    ) -> Arc<dyn ody_core::StagedWriteSink> {
        self.staging.clone()
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

    /// Import a Visual Artifact's standalone source into a bound root as a
    /// Pending changeset (single Add change), recording provenance so the
    /// prototype-to-workspace merge ratio stays computable runtime-side.
    /// The import itself reuses the changeset pipeline: base-hash checks,
    /// conflict detection, checkpoint capture, and the review UI all apply.
    pub(crate) async fn artifact_bridge(
        &self,
        params: WorkspaceArtifactBridgeParams,
    ) -> Result<WorkspaceArtifactBridgeResponse, JSONRPCErrorError> {
        let project_id = params.project_id.clone();
        let result = self.artifact_bridge_inner(params).await;
        match &result {
            Ok(response) => self.audit.record(audit_event(
                Some(&response.bridge.project_id),
                "artifact.bridge",
                "ok",
                serde_json::json!({
                    "bridgeId": response.bridge.id,
                    "changesetId": response.bridge.changeset_id,
                    "artifactId": response.bridge.artifact_id,
                    "targetPath": response.bridge.target_path,
                }),
            )),
            Err(err) => self.audit.record(audit_event(
                Some(&project_id),
                "artifact.bridge",
                "error",
                error_detail("artifact.bridge", err),
            )),
        }
        result
    }

    async fn artifact_bridge_inner(
        &self,
        params: WorkspaceArtifactBridgeParams,
    ) -> Result<WorkspaceArtifactBridgeResponse, JSONRPCErrorError> {
        // Idempotent retry: return the existing record before any validation.
        if let Some(existing) = self.with_store(|store| {
            Ok(store
                .bridges
                .values()
                .find(|bridge| bridge.idempotency_key == params.idempotency_key)
                .cloned())
        })? {
            let changeset = self.stored_changeset(&existing.changeset_id)?.1.change_set;
            return Ok(WorkspaceArtifactBridgeResponse {
                bridge: existing,
                changeset,
            });
        }
        if params.content.len() > MAX_ARTIFACT_BRIDGE_CONTENT_BYTES {
            return Err(invalid_params(format!(
                "artifact bridge content exceeds {} bytes",
                MAX_ARTIFACT_BRIDGE_CONTENT_BYTES
            )));
        }
        let project = self.project(&params.project_id)?;
        let root_index = params.root_index.unwrap_or(0);
        let root = project
            .roots
            .get(root_index as usize)
            .ok_or_else(|| invalid_params(format!("root_index {root_index} out of range")))?;
        let normalized =
            crate::workspace_changeset::normalize_relative(&params.target_path)?;
        let absolute = Path::new(&root.path).join(&normalized);
        if absolute.exists() {
            return Err(invalid_params(format!(
                "artifact bridge target {normalized:?} already exists; overwrite via a changeset"
            )));
        }
        let target = normalized.clone();
        let response = self
            .changeset_create_inner(WorkspaceChangeSetCreateParams {
                project_id: params.project_id.clone(),
                title: format!("Import artifact into {target}").chars().take(200).collect(),
                changes: vec![WorkspaceFileChange {
                    root_index,
                    path: normalized,
                    kind: WorkspaceFileChangeKind::Add,
                    base_hash: None,
                    content: Some(params.content),
                }],
                idempotency_key: format!("artifact-bridge:{}", params.idempotency_key),
            })
            .await?;
        let now = now_ms();
        let bridge = WorkspaceArtifactBridge {
            id: format!("ab-{}", uuid::Uuid::new_v4()),
            project_id: params.project_id,
            artifact_id: params.artifact_id,
            target_path: target,
            changeset_id: response.changeset.id.clone(),
            idempotency_key: params.idempotency_key,
            status: WorkspaceArtifactBridgeStatus::Pending,
            created_at_ms: now,
            updated_at_ms: now,
        };
        self.with_store(|store| {
            store.bridges.insert(bridge.id.clone(), bridge.clone());
            store.persist()?;
            Ok(())
        })?;
        Ok(WorkspaceArtifactBridgeResponse {
            bridge,
            changeset: response.changeset,
        })
    }

    pub(crate) async fn artifact_bridge_list(
        &self,
        params: WorkspaceArtifactBridgeListParams,
    ) -> Result<WorkspaceArtifactBridgeListResponse, JSONRPCErrorError> {
        let project = self.project(&params.project_id)?;
        let mut bridges = self.with_store(|store| {
            Ok(store
                .bridges
                .values()
                .filter(|bridge| bridge.project_id == project.id)
                .cloned()
                .collect::<Vec<_>>())
        })?;
        bridges.sort_by(|a, b| b.created_at_ms.cmp(&a.created_at_ms));
        Ok(WorkspaceArtifactBridgeListResponse { bridges })
    }

    pub(crate) async fn changeset_create(
        &self,
        params: WorkspaceChangeSetCreateParams,
    ) -> Result<WorkspaceChangeSetCreateResponse, JSONRPCErrorError> {
        let project_id = params.project_id.clone();
        let result = self.changeset_create_inner(params).await;
        match &result {
            Ok(response) => {
                let files: Vec<String> = response
                    .changeset
                    .changes
                    .iter()
                    .map(|change| format!("{}:{}", change.root_index, change.path))
                    .collect();
                self.audit.record(audit_event(
                    Some(&response.changeset.project_id),
                    "changeset.create",
                    "ok",
                    serde_json::json!({
                        "changesetId": response.changeset.id,
                        "files": files,
                    }),
                ));
            }
            Err(err) => self.audit.record(audit_event(
                Some(&project_id),
                "changeset.create",
                "error",
                error_detail("changeset.create", err),
            )),
        }
        result
    }

    async fn changeset_create_inner(
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
            return Ok(WorkspaceChangeSetCreateResponse {
                changeset: existing,
            });
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
                invalidated_reason: None,
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
        connection_id: ConnectionId,
    ) -> Result<WorkspaceChangeSetApplyResponse, JSONRPCErrorError> {
        let changeset_id = params.changeset_id.clone();
        let result = self.changeset_apply_inner(params, connection_id).await;
        match &result {
            Ok(response) => self.audit.record(audit_event(
                Some(&response.changeset.project_id),
                "changeset.apply",
                "ok",
                serde_json::json!({
                    "changesetId": response.changeset.id,
                    "outcome": "applied",
                }),
            )),
            Err(err) => self.audit.record(audit_event(
                None,
                "changeset.apply",
                "error",
                serde_json::json!({
                    "changesetId": changeset_id,
                    "error": err.message.chars().take(200).collect::<String>(),
                }),
            )),
        }
        result
    }

    async fn changeset_apply_inner(
        &self,
        params: WorkspaceChangeSetApplyParams,
        connection_id: ConnectionId,
    ) -> Result<WorkspaceChangeSetApplyResponse, JSONRPCErrorError> {
        let (project, mut stored) = self.stored_changeset(&params.changeset_id)?;
        // E4 T05: cross-connection write guard runs *before* the T02
        // invalidated check — a holder is always allowed in, and a foreign
        // writer is rejected before any file verification happens.
        self.write_lock.check(&project.id, connection_id)?;
        if stored.change_set.status != WorkspaceChangeSetStatus::Pending {
            return Err(invalid_params(format!(
                "changeset {} is {:?}; expected Pending for apply",
                stored.change_set.id, stored.change_set.status
            )));
        }
        // E4: an external edit already contradicted a base hash while the
        // changeset was Pending — refuse before touching any file (the
        // watcher marks this; apply is the enforcement point, so even a
        // client that ignored `invalidatedChangesets` cannot be clobbered).
        if let Some(reason) = &stored.change_set.invalidated_reason {
            return Err(invalid_params(format!(
                "changeset {} was invalidated: {reason}. Re-index and re-create the changeset; no files were written.",
                stored.change_set.id
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
                    checkpoint = WorkspaceChangeSetCheckpoint::Git {
                        head_commit_hash: head,
                    };
                }
                None => {
                    ody_git_utils::ensure_git_baseline_repository(Path::new(root_path))
                        .await
                        .map_err(|err| {
                            internal_error(format!("checkpoint failed for {root_path}: {err}"))
                        })?;
                    checkpoint = WorkspaceChangeSetCheckpoint::Git {
                        head_commit_hash: None,
                    };
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
            sync_bridge_status(store, &params.changeset_id, WorkspaceArtifactBridgeStatus::Applied);
            store.persist()?;
            Ok(store
                .changesets
                .get(&params.changeset_id)
                .expect("just inserted")
                .change_set
                .clone())
        })?;
        // P1 切片1：确认写回闭环——apply 成功后按 root 分别 git commit
        // （best-effort，单 root 失败不影响 apply 结果本身）。
        let commit = match params.commit_message.as_deref().map(str::trim) {
            Some(message) if !message.is_empty() => {
                let mut files_by_root: BTreeMap<usize, Vec<String>> = BTreeMap::new();
                for change in &prepared {
                    files_by_root
                        .entry(change.root_index)
                        .or_default()
                        .push(change.relative.clone());
                }
                let mut reports = Vec::new();
                for (root_index, files) in files_by_root {
                    let root_path = project
                        .roots
                        .get(root_index)
                        .map(|root| root.path.clone())
                        .unwrap_or_default();
                    let outcome = commit_changeset_in_root(&root_path, &files, message).await;
                    reports.push(WorkspaceChangeSetCommitReport { root_path, outcome });
                }
                Some(reports)
            }
            _ => None,
        };
        Ok(WorkspaceChangeSetApplyResponse {
            changeset: change_set,
            commit,
        })
    }

    pub(crate) async fn changeset_reject(
        &self,
        params: WorkspaceChangeSetRejectParams,
    ) -> Result<WorkspaceChangeSetRejectResponse, JSONRPCErrorError> {
        let changeset_id = params.changeset_id.clone();
        let result = self.changeset_reject_inner(params).await;
        match &result {
            Ok(response) => self.audit.record(audit_event(
                Some(&response.changeset.project_id),
                "changeset.reject",
                "ok",
                serde_json::json!({"changesetId": response.changeset.id}),
            )),
            Err(err) => self.audit.record(audit_event(
                None,
                "changeset.reject",
                "error",
                serde_json::json!({
                    "changesetId": changeset_id,
                    "error": err.message.chars().take(200).collect::<String>(),
                }),
            )),
        }
        result
    }

    async fn changeset_reject_inner(
        &self,
        params: WorkspaceChangeSetRejectParams,
    ) -> Result<WorkspaceChangeSetRejectResponse, JSONRPCErrorError> {
        let (project, mut stored) = self.stored_changeset(&params.changeset_id)?;
        if stored.change_set.status != WorkspaceChangeSetStatus::Pending {
            return Err(invalid_params(format!(
                "changeset {} is {:?}; expected Pending for reject",
                stored.change_set.id, stored.change_set.status
            )));
        }
        // P1 切片1（可逆写盘）：reject 同时把已落盘的暂存内容还原回 base。
        // 仅当磁盘仍与暂存内容一致（agent 写盘落地、未被用户再改）；不一致
        // 的文件视为用户工作，绝不动。artifact 时代的 changeset 文件未写盘，
        // 磁盘 == base ≠ staged，自然跳过，语义无损。
        {
            let prepared = rebuild_prepared(&project, &stored)?;
            for change in &prepared {
                let on_disk_matches_staged = match change.kind {
                    WorkspaceFileChangeKind::Add | WorkspaceFileChangeKind::Update => {
                        change.absolute.exists()
                            && fs::read(&change.absolute).ok().as_deref()
                                == change.content.as_deref().map(str::as_bytes)
                    }
                    WorkspaceFileChangeKind::Delete => !change.absolute.exists(),
                };
                if !on_disk_matches_staged {
                    continue;
                }
                match change.kind {
                    WorkspaceFileChangeKind::Add => {
                        let _ = fs::remove_file(&change.absolute);
                    }
                    WorkspaceFileChangeKind::Update | WorkspaceFileChangeKind::Delete => {
                        if let Some(base) = stored.base_contents.get(&change.key) {
                            let _ =
                                crate::workspace_changeset::atomic_write(&change.absolute, base);
                        }
                    }
                }
            }
        }
        stored.change_set.status = WorkspaceChangeSetStatus::Rejected;
        stored.change_set.updated_at_ms = now_ms();
        let changeset_id = stored.change_set.id.clone();
        let change_set = self.with_store(|store| {
            store
                .changesets
                .insert(changeset_id.clone(), stored);
            sync_bridge_status(store, &changeset_id, WorkspaceArtifactBridgeStatus::Rejected);
            store.persist()?;
            Ok(store
                .changesets
                .get(&params.changeset_id)
                .expect("just inserted")
                .change_set
                .clone())
        })?;
        Ok(WorkspaceChangeSetRejectResponse {
            changeset: change_set,
        })
    }

    pub(crate) async fn changeset_restore(
        &self,
        params: WorkspaceChangeSetRestoreParams,
        connection_id: ConnectionId,
    ) -> Result<WorkspaceChangeSetRestoreResponse, JSONRPCErrorError> {
        let changeset_id = params.changeset_id.clone();
        let result = self.changeset_restore_inner(params, connection_id).await;
        match &result {
            Ok(response) => self.audit.record(audit_event(
                Some(&response.changeset.project_id),
                "changeset.restore",
                "ok",
                serde_json::json!({"changesetId": response.changeset.id}),
            )),
            Err(err) => self.audit.record(audit_event(
                None,
                "changeset.restore",
                "error",
                serde_json::json!({
                    "changesetId": changeset_id,
                    "error": err.message.chars().take(200).collect::<String>(),
                }),
            )),
        }
        result
    }

    async fn changeset_restore_inner(
        &self,
        params: WorkspaceChangeSetRestoreParams,
        connection_id: ConnectionId,
    ) -> Result<WorkspaceChangeSetRestoreResponse, JSONRPCErrorError> {
        let (project, mut stored) = self.stored_changeset(&params.changeset_id)?;
        self.write_lock.check(&project.id, connection_id)?;
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
        let changeset_id = stored.change_set.id.clone();
        let change_set = self.with_store(|store| {
            store
                .changesets
                .insert(changeset_id.clone(), stored);
            sync_bridge_status(store, &changeset_id, WorkspaceArtifactBridgeStatus::Pending);
            store.persist()?;
            Ok(store
                .changesets
                .get(&params.changeset_id)
                .expect("just inserted")
                .change_set
                .clone())
        })?;
        Ok(WorkspaceChangeSetRestoreResponse {
            changeset: change_set,
        })
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
        let mut root_git_diffs: Vec<WorkspaceRootGitDiff> = Vec::new();
        for (index, root) in project.roots.iter().enumerate() {
            if let Some(git) = root_git_diff(Path::new(&root.path)).await {
                root_git_diffs.push(WorkspaceRootGitDiff {
                    root_index: index as u32,
                    git,
                });
            }
        }
        // Back-compat (ADR decision 9): the legacy field carries the
        // primary root's diff exactly as E1 produced it.
        let git_diff = root_git_diffs
            .first()
            .filter(|entry| entry.root_index == 0)
            .map(|entry| entry.git.clone());
        Ok(WorkspaceSourceDiffResponse {
            changesets,
            git_diff,
            root_git_diffs,
        })
    }

    pub(crate) async fn validate(
        &self,
        params: WorkspaceSourceValidateParams,
    ) -> Result<WorkspaceSourceValidateResponse, JSONRPCErrorError> {
        let project = self.project(&params.project_id)?;
        if params.checks.is_empty() {
            return Err(invalid_params("validate requires at least one check"));
        }
        if params.checks.len() > crate::workspace_validation::MAX_VALIDATION_CHECKS {
            return Err(invalid_params(format!(
                "validate accepts at most {} checks",
                crate::workspace_validation::MAX_VALIDATION_CHECKS
            )));
        }
        for check in &params.checks {
            if check.script.trim().is_empty() {
                return Err(invalid_params("check script must not be empty"));
            }
        }
        // E3: resolve every check's root up front; an out-of-bounds index
        // fails the whole request before anything spawns (ADR decision 8).
        let mut checks_with_roots: Vec<(PathBuf, WorkspaceValidationCheck)> =
            Vec::with_capacity(params.checks.len());
        for check in &params.checks {
            let root = crate::workspace_validation::resolve_check_root(&project, check.root_index)
                .map_err(invalid_params)?;
            checks_with_roots.push((PathBuf::from(&root.path), check.clone()));
        }
        let timeout_ms = params
            .timeout_ms
            .unwrap_or(crate::workspace_validation::VALIDATE_DEFAULT_TIMEOUT_MS)
            .clamp(1_000, crate::workspace_validation::VALIDATE_MAX_TIMEOUT_MS);
        // changeset association is display-only but must be valid.
        if let Some(changeset_id) = &params.changeset_id {
            let (found, belongs) = self.with_store(|store| {
                Ok(match store.changesets.get(changeset_id) {
                    Some(stored) => (true, stored.change_set.project_id == params.project_id),
                    None => (false, false),
                })
            })?;
            if !found {
                return Err(invalid_params(format!(
                    "unknown changeset id: {changeset_id}"
                )));
            }
            if !belongs {
                return Err(invalid_params(format!(
                    "changeset {changeset_id} does not belong to project {}",
                    params.project_id
                )));
            }
        }
        let report = crate::workspace_validation::run_checks(&checks_with_roots, timeout_ms).await;
        Ok(WorkspaceSourceValidateResponse {
            project_id: params.project_id,
            changeset_id: params.changeset_id,
            report,
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

    /// E4: cross-reference an external-change batch against this project's
    /// Pending changesets. Returns the changeset ids that became invalid.
    /// Called by the workspace watcher (see workspace_watch.rs sink).
    pub(crate) fn invalidate_from_external_events(
        &self,
        project_id: &str,
        events: &[WorkspaceFileEvent],
    ) -> Vec<String> {
        if events.is_empty() {
            return Vec::new();
        }
        let mut invalidated = Vec::new();
        let mut touched = false;
        {
            let mut store = self.store.lock().expect("store lock");
            for (id, stored) in store.changesets.iter_mut() {
                if stored.change_set.project_id != project_id
                    || stored.change_set.status != WorkspaceChangeSetStatus::Pending
                {
                    continue;
                }
                let conflicted: Vec<String> =
                    conflicted_change_keys(&stored.change_set.changes, events)
                        .into_iter()
                        .filter(|key| {
                            // 可逆写盘的中间代内容：watcher 批量事件可能携带
                            // 既不是 base 也不是最终 staged 的代际；只要它出
                            // 现在本会话最近暂存写盘记录里，就不是外部冲突。
                            let event = events
                                .iter()
                                .find(|event| format!("{}:{}", event.root_index, event.path) == *key);
                            match event.and_then(|event| event.content_hash.as_ref()) {
                                Some(hash) => !self.staging.is_recent_staged(key, hash),
                                None => true,
                            }
                        })
                        .collect();
                if !conflicted.is_empty() && stored.change_set.invalidated_reason.is_none() {
                    stored.change_set.invalidated_reason = Some(format!(
                        "external edit conflicted with base hashes: {}",
                        conflicted.join(", ")
                    ));
                    stored.change_set.updated_at_ms = now_ms();
                    touched = true;
                    invalidated.push(id.clone());
                }
            }
        }
        if touched {
            let _ = self.with_store(|store| store.persist());
        }
        invalidated
    }

    /// E4 overflow rescan: the watcher truncated its event batch, so
    /// unobserved events may have missed a conflict. Re-verify every
    /// Pending changeset of the project against the on-disk bytes
    /// (fail-closed: unreadable files count as conflicts).
    pub(crate) fn invalidate_all_pending(&self, project_id: &str) -> Vec<String> {
        let mut invalidated = Vec::new();
        let mut touched = false;
        {
            let mut store = self.store.lock().expect("store lock");
            let pending: Vec<String> = store
                .changesets
                .values()
                .filter(|stored| {
                    stored.change_set.project_id == project_id
                        && stored.change_set.status == WorkspaceChangeSetStatus::Pending
                })
                .map(|stored| stored.change_set.id.clone())
                .collect();
            for id in pending {
                let Some(stored) = store.changesets.get(&id) else {
                    continue;
                };
                let mut conflicted: Vec<String> = Vec::new();
                for change in &stored.change_set.changes {
                    let key = format!("{}:{}", change.root_index, change.path);
                    let on_disk = stored.targets.get(&key).and_then(|target| {
                        std::fs::read(target)
                            .ok()
                            .map(|bytes| crate::workspace_changeset::sha256_hex(&bytes))
                    });
                    let base_ok = match on_disk {
                        Some(hash) => Some(hash.as_str()) == change.base_hash.as_deref(),
                        // Unreadable = conflict (fail closed).
                        None => false,
                    };
                    if !base_ok {
                        conflicted.push(key);
                    }
                }
                if !conflicted.is_empty() {
                    let stored = store.changesets.get_mut(&id).expect("pending id collected");
                    if stored.change_set.invalidated_reason.is_none() {
                        stored.change_set.invalidated_reason = Some(format!(
                            "external edit conflicted with base hashes: {}",
                            conflicted.join(", ")
                        ));
                        stored.change_set.updated_at_ms = now_ms();
                        touched = true;
                        invalidated.push(id);
                    }
                }
            }
        }
        if touched {
            let _ = self.with_store(|store| store.persist());
        }
        invalidated
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
    pub(crate) changesets: BTreeMap<String, StoredChangeSet>,
    /// Artifact import provenance records; status mirrors the paired
    /// changeset lifecycle (serde(default) upgrades v1/v2 store files).
    #[serde(default)]
    bridges: BTreeMap<String, WorkspaceArtifactBridge>,
    /// Namespaced idempotency key ("changeset:{key}") -> changeset id.
    idempotency: BTreeMap<String, String>,
    #[serde(skip)]
    path: PathBuf,
}

impl WorkspaceSourceStore {
    pub(crate) fn load(path: PathBuf) -> Self {
        let backup = path.with_extension("json.bak");
        for candidate in [&path, &backup] {
            if let Ok(raw) = fs::read(candidate)
                && let Ok(mut state) = serde_json::from_slice::<Self>(&raw)
                && state.schema_version >= 1
                && state.schema_version <= WORKSPACE_SOURCE_PROTOCOL_VERSION
            {
                // Accept older versions (serde(default) fills new fields —
                // same normalization the service store uses); stamp the
                // current version and persist the upgrade so a v1 file's
                // changesets survive the bump instead of vanishing.
                let upgraded = state.schema_version != WORKSPACE_SOURCE_PROTOCOL_VERSION;
                state.schema_version = WORKSPACE_SOURCE_PROTOCOL_VERSION;
                state.path = path.clone();
                if upgraded {
                    let _ = state.persist();
                }
                return state;
            }
        }
        Self {
            schema_version: WORKSPACE_SOURCE_PROTOCOL_VERSION,
            path,
            ..Default::default()
        }
    }

    pub(crate) fn persist(&self) -> Result<(), JSONRPCErrorError> {
        let directory = self
            .path
            .parent()
            .ok_or_else(|| internal_error("workspace source store path has no parent"))?;
        fs::create_dir_all(directory).map_err(|error| {
            internal_error(format!("failed to create workspace source store: {error}"))
        })?;
        let serialized = serde_json::to_vec_pretty(self).map_err(|error| {
            internal_error(format!(
                "failed to serialize workspace source store: {error}"
            ))
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
        let absolute =
            crate::workspace_changeset::resolve_target(Path::new(&root.path), &normalized)?;
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

/// Cap for per-file untracked diffs: each entry costs one `git` fork, so an
/// unbounded untracked list would blow the request budget. Excess files are
/// reported in a trailing notice line instead of being silently dropped.
const MAX_UNTRACKED_DIFF_FILES: usize = 50;

/// Run one git command inside `repo_root` under the shared diff timeout.
/// `None` = the binary could not be spawned; `Some(Err)` = non-zero exit or
/// timeout (the message is caller-facing). `tolerate_exit_1` treats exit code
/// 1 as success (`git diff --no-index` reports "differences found" that way).
async fn run_git(
    repo_root: &Path,
    args: &[&str],
    tolerate_exit_1: bool,
) -> Option<Result<String, String>> {
    let result = tokio::time::timeout(
        Duration::from_millis(GIT_DIFF_TIMEOUT_MS),
        tokio::process::Command::new("git")
            .args(args)
            .current_dir(repo_root)
            .output(),
    )
    .await;
    match result {
        Ok(Ok(output))
            if output.status.success() || (tolerate_exit_1 && output.status.code() == Some(1)) =>
        {
            Some(Ok(String::from_utf8_lossy(&output.stdout).into_owned()))
        }
        Ok(Ok(output)) => Some(Err(format!(
            "git {} exited with {}",
            args.join(" "),
            output.status
        ))),
        Ok(Err(err)) => Some(Err(format!("git {} failed to run: {err}", args.join(" ")))),
        Err(_) => Some(Err(format!(
            "git {} timed out after {GIT_DIFF_TIMEOUT_MS}ms",
            args.join(" ")
        ))),
    }
}

async fn root_git_diff(root_path: &Path) -> Option<WorkspaceGitDiff> {
    root_git_diff_limited(root_path, MAX_UNTRACKED_DIFF_FILES).await
}

async fn root_git_diff_limited(root_path: &Path, max_untracked: usize) -> Option<WorkspaceGitDiff> {
    let repo_root = ody_git_utils::get_git_repo_root(root_path)?;
    let repo_string = repo_root.to_string_lossy().into_owned();

    // Tracked channel. Repos without a first commit (empty HEAD) fail here;
    // that is tolerated because the untracked channel below still renders.
    let tracked = run_git(&repo_root, &["diff", "HEAD"], false).await;
    let tracked_diff = match &tracked {
        Some(Ok(stdout)) => stdout.clone(),
        _ => String::new(),
    };

    // Untracked channel: `ls-files --others --exclude-standard` respects
    // .gitignore, then each file becomes a `/dev/null -> new file` diff via
    // `git diff --no-index` (exit code 1 means "differences found").
    let untracked_list = run_git(
        &repo_root,
        &["ls-files", "--others", "--exclude-standard"],
        false,
    )
    .await;
    let mut untracked_parts: Vec<String> = Vec::new();
    let mut omitted = 0usize;
    if let Some(Ok(list)) = &untracked_list {
        for path in list.lines().map(str::trim).filter(|line| !line.is_empty()) {
            if untracked_parts.len() >= max_untracked {
                omitted += 1;
                continue;
            }
            let Some(Ok(diff)) = run_git(
                &repo_root,
                &["diff", "--no-index", "--", "/dev/null", path],
                true,
            )
            .await
            else {
                continue;
            };
            untracked_parts.push(diff);
        }
    }

    let tracked_ok = matches!(tracked, Some(Ok(_)));
    let untracked_ok = matches!(untracked_list, Some(Ok(_)));
    if !tracked_ok && !untracked_ok {
        // Both channels failed: git itself is unusable here.
        let error = tracked
            .and_then(|result| result.err())
            .or_else(|| untracked_list.and_then(|result| result.err()))
            .unwrap_or_else(|| "git unavailable".to_owned());
        return Some(WorkspaceGitDiff {
            repo_root: Some(repo_string),
            available: false,
            error: Some(error),
            unified_diff: None,
        });
    }

    let mut unified = tracked_diff;
    if !untracked_parts.is_empty() {
        if !unified.is_empty() {
            unified.push('\n');
        }
        unified.push_str(&untracked_parts.join(""));
    }
    if omitted > 0 {
        if !unified.is_empty() {
            unified.push('\n');
        }
        unified.push_str(&format!(
            "# {omitted} untracked files omitted (cap {max_untracked})\n"
        ));
    }
    Some(WorkspaceGitDiff {
        repo_root: Some(repo_string),
        available: true,
        error: None,
        unified_diff: Some(unified),
    })
}

fn unknown_project(id: &str) -> JSONRPCErrorError {
    invalid_params(format!("unknown workspace project id: {id}"))
}

/// 在 root 内运行一条 git 命令，尾部附加本 changeset 的文件路径。
async fn run_git_for_paths(
    root: &Path,
    files: &[String],
    args: &[&str],
) -> std::io::Result<std::process::Output> {
    let mut command = tokio::process::Command::new("git");
    command.args(args).current_dir(root);
    for file in files {
        command.arg(file);
    }
    command.output().await
}

/// P1 切片1：apply 成功后在一个 root 内提交本 changeset 的文件。
/// 只 add/commit 这些路径，不碰用户其他暂存。
async fn commit_changeset_in_root(
    root_path: &str,
    files: &[String],
    message: &str,
) -> WorkspaceChangeSetCommitOutcome {
    let root = Path::new(root_path);
    if ody_git_utils::get_git_repo_root(root).is_none() {
        return WorkspaceChangeSetCommitOutcome::NotAGitRepo;
    }
    match run_git_for_paths(root, files, &["add", "-A", "--"]).await {
        Err(err) => {
            return WorkspaceChangeSetCommitOutcome::Failed {
                error: format!("git add: {err}"),
            }
        }
        Ok(output) if !output.status.success() => {
            return WorkspaceChangeSetCommitOutcome::Failed {
                error: String::from_utf8_lossy(&output.stderr).chars().take(200).collect(),
            }
        }
        Ok(_) => {}
    }
    match run_git_for_paths(root, files, &["commit", "-m", message, "--"]).await {
        Err(err) => WorkspaceChangeSetCommitOutcome::Failed {
            error: format!("git commit: {err}"),
        },
        Ok(output) if output.status.success() => {
            let hash = tokio::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(root)
                .output()
                .await
                .ok()
                .filter(|output| output.status.success())
                .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
                .unwrap_or_default();
            WorkspaceChangeSetCommitOutcome::Committed { commit_hash: hash }
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // 非 git 工程的 apply 会先把当前磁盘快照成 baseline 仓库
            // （Phase 2 checkpoint），此时 changeset 内容已在 baseline
            // commit 里，工作区对这些路径是干净的——属于 NothingToCommit，
            // 不是失败。
            let nothing_staged = tokio::process::Command::new("git")
                .args(["status", "--porcelain", "--"])
                .current_dir(root)
                .args(files)
                .output()
                .await
                .map(|status| {
                    status.status.success()
                        && String::from_utf8_lossy(&status.stdout).trim().is_empty()
                })
                .unwrap_or(false);
            if nothing_staged {
                WorkspaceChangeSetCommitOutcome::NothingToCommit
            } else {
                WorkspaceChangeSetCommitOutcome::Failed {
                    error: stderr.chars().take(200).collect(),
                }
            }
        }
    }
}

/// E4: the change keys of a Pending changeset contradicted by an external
/// event batch. Fail-closed rules: a `Removed` event invalidates an
/// Update/Delete base (file gone); an unreadable file (None hash) counts as
/// a conflict; an `Added` event colliding with an `Add` change is a
/// conflict (apply would clobber an externally created file); an event
/// whose hash equals the base hash (IDE round-trip save) is NOT a
/// conflict.
fn conflicted_change_keys(
    changes: &[ody_app_server_protocol::WorkspaceFileChange],
    events: &[WorkspaceFileEvent],
) -> Vec<String> {
    let mut conflicted = Vec::new();
    for change in changes {
        let key = format!("{}:{}", change.root_index, change.path);
        let Some(event) = events
            .iter()
            .find(|event| event.root_index == change.root_index && event.path == change.path)
        else {
            continue;
        };
        match change.kind {
            WorkspaceFileChangeKind::Add => {
                // 暂存写盘落地（Added 且内容 == staged）不是冲突。
                let staged_landed = event.kind == WorkspaceFileEventKind::Added
                    && staged_content_matches(change, event);
                if event.kind == WorkspaceFileEventKind::Added && !staged_landed {
                    conflicted.push(key);
                }
            }
            WorkspaceFileChangeKind::Update => {
                // 可逆写盘：磁盘内容 == staged 内容即 agent 自己的写盘落地。
                if staged_content_matches(change, event) {
                    continue;
                }
                let base_ok = match event.kind {
                    WorkspaceFileEventKind::Removed => false,
                    _ => {
                        event.content_hash.is_some()
                            && event.content_hash.as_deref() == change.base_hash.as_deref()
                    }
                };
                if !base_ok {
                    conflicted.push(key);
                }
            }
            WorkspaceFileChangeKind::Delete => {
                // 暂存的删除已落盘（文件已不存在）；reject 可还原 base，
                // 两种成因下还原都是正确行为，不构成冲突。
                if event.kind == WorkspaceFileEventKind::Removed {
                    continue;
                }
                let base_ok = event.content_hash.is_some()
                    && event.content_hash.as_deref() == change.base_hash.as_deref();
                if !base_ok {
                    conflicted.push(key);
                }
            }
        }
    }
    conflicted
}

/// 事件携带的内容 hash 是否等于 changeset 的 staged（新）内容 hash。
/// P1 切片1 可逆写盘：agent 写盘会触发 watcher 事件，落地内容与新内容
/// 一致时必须视为「自己的写盘」而非外部编辑。
fn staged_content_matches(
    change: &ody_app_server_protocol::WorkspaceFileChange,
    event: &WorkspaceFileEvent,
) -> bool {
    match (&change.content, &event.content_hash) {
        (Some(content), Some(hash)) => {
            crate::workspace_changeset::sha256_hex(content.as_bytes()) == *hash
        }
        _ => false,
    }
}

/// Mirror a changeset lifecycle transition onto its artifact bridge record
/// (a bridge is always paired 1:1 with the changeset that carries it).
fn sync_bridge_status(
    store: &mut WorkspaceSourceStore,
    changeset_id: &str,
    status: WorkspaceArtifactBridgeStatus,
) {
    let now = now_ms();
    for bridge in store.bridges.values_mut() {
        if bridge.changeset_id == changeset_id && bridge.status != status {
            bridge.status = status;
            bridge.updated_at_ms = now;
        }
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
        fs::write(
            pages.join("HomePage.tsx"),
            "export default function HomePage() {}\n",
        )
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
            locked_by: None,
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
                invalidated_reason: None,
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
        assert_eq!(
            reloaded.change_set.status,
            WorkspaceChangeSetStatus::Pending
        );
        assert_eq!(reloaded.base_contents.len(), 1);
        let target = reloaded.targets.values().next().expect("target");
        assert!(target.starts_with(root.to_string_lossy().as_ref()));
        let rebuilt = rebuild_prepared(&project, reloaded).expect("rebuild prepared");
        assert_eq!(rebuilt.len(), 1);
        assert_eq!(rebuilt[0].base_content.as_deref(), Some(base.as_str()));
    }

    // ---- E4 T02: external-edit conflict invalidation ----

    #[test]
    fn load_upgrades_version_1_store_without_dropping_changesets() {
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("v1.json");
        // Hand-written v1 payload: no invalidated_reason field anywhere.
        // Store-level keys are snake_case (the store struct has no serde
        // rename_all); the flattened changeset keeps protocol camelCase.
        let v1 = serde_json::json!({
            "schema_version": 1,
            "changesets": {
                "cs-1": {
                    "id": "cs-1", "projectId": "p1", "title": "t",
                    "schemaVersion": 1,
                    "changes": [{
                        "rootIndex": 0, "path": "src/a.ts",
                        "kind": "update", "baseHash": "abc", "content": "x"
                    }],
                    "status": "pending",
                    "checkpoint": "none",
                    "unifiedDiff": "", "createdAtMs": 1, "updatedAtMs": 2,
                    "appliedAtMs": null,
                    "base_contents": {"0:src/a.ts": "old"},
                    "applied_hashes": {},
                    "targets": {"0:src/a.ts": "/tmp/src/a.ts"}
                }
            },
            "idempotency": {}
        });
        fs::write(&path, serde_json::to_vec_pretty(&v1).unwrap()).unwrap();
        let store = WorkspaceSourceStore::load(path);
        assert_eq!(store.schema_version, WORKSPACE_SOURCE_PROTOCOL_VERSION);
        let cs = store.changesets.get("cs-1").expect("v1 changeset must survive");
        assert_eq!(cs.change_set.status, WorkspaceChangeSetStatus::Pending);
        assert_eq!(cs.change_set.invalidated_reason, None);
    }

    /// Bind a one-root project and create a Pending changeset through the
    /// real create path; returns the processor and the created changeset id.
    async fn processor_with_project_and_pending_changeset(
        ody_home: &Path,
        project_id: &str,
        root: &Path,
        changes: Vec<WorkspaceFileChange>,
    ) -> (
        WorkspaceSourceRequestProcessor,
        String,
        Arc<WorkspaceWriteLock>,
    ) {
        use crate::request_processors::workspace_project_processor::WorkspaceProjectStore;

        let project_store = Arc::new(Mutex::new(WorkspaceProjectStore::default()));
        project_store
            .lock()
            .expect("project store lock")
            .projects
            .insert(
                project_id.to_owned(),
                WorkspaceProjectRef {
                    id: project_id.to_owned(),
                    name: "fixture".to_owned(),
                    schema_version: 1,
                    roots: vec![WorkspaceRoot {
                        path: root.to_string_lossy().into_owned(),
                        role: WorkspaceRootRole::Primary,
                        auth_source: "user_selected".to_owned(),
                    }],
                    created_at_ms: 0,
                    updated_at_ms: 0,
                    locked_by: None,
                },
            );
        let audit = Arc::new(crate::workspace_audit::WorkspaceAuditLog::new(
            ody_home.join("workspace-audit").join("v1.jsonl"),
        ));
        let write_lock = Arc::new(WorkspaceWriteLock::default());
        let processor = WorkspaceSourceRequestProcessor::new(
            ody_home.to_path_buf(),
            project_store,
            audit,
            Arc::clone(&write_lock),
        );
        let response = processor
            .changeset_create(WorkspaceChangeSetCreateParams {
                project_id: project_id.to_owned(),
                title: "t".to_owned(),
                changes,
                idempotency_key: "cs-key-1".to_owned(),
            })
            .await
            .expect("create changeset");
        (processor, response.changeset.id, write_lock)
    }

    fn pending_update_change(base: &str) -> WorkspaceFileChange {
        WorkspaceFileChange {
            root_index: 0,
            path: "src/a.ts".to_owned(),
            kind: WorkspaceFileChangeKind::Update,
            base_hash: Some(crate::workspace_changeset::sha256_hex(base.as_bytes())),
            content: Some("new\n".to_owned()),
        }
    }

    async fn processor_with_project(
        ody_home: &Path,
        project_id: &str,
        root: &Path,
    ) -> WorkspaceSourceRequestProcessor {
        use crate::request_processors::workspace_project_processor::WorkspaceProjectStore;

        let project_store = Arc::new(Mutex::new(WorkspaceProjectStore::default()));
        project_store
            .lock()
            .expect("project store lock")
            .projects
            .insert(
                project_id.to_owned(),
                WorkspaceProjectRef {
                    id: project_id.to_owned(),
                    name: "fixture".to_owned(),
                    schema_version: 1,
                    roots: vec![WorkspaceRoot {
                        path: root.to_string_lossy().into_owned(),
                        role: WorkspaceRootRole::Primary,
                        auth_source: "user_selected".to_owned(),
                    }],
                    created_at_ms: 0,
                    updated_at_ms: 0,
                    locked_by: None,
                },
            );
        let audit = Arc::new(crate::workspace_audit::WorkspaceAuditLog::new(
            ody_home.join("workspace-audit").join("v1.jsonl"),
        ));
        WorkspaceSourceRequestProcessor::new(
            ody_home.to_path_buf(),
            project_store,
            audit,
            Arc::new(WorkspaceWriteLock::default()),
        )
    }

    fn bridge_params(project_id: &str, target: &str, key: &str) -> WorkspaceArtifactBridgeParams {
        WorkspaceArtifactBridgeParams {
            project_id: project_id.to_owned(),
            root_index: None,
            artifact_id: Some("artifact-1".to_owned()),
            target_path: target.to_owned(),
            content: "<html><body>hello</body></html>".to_owned(),
            idempotency_key: key.to_owned(),
        }
    }

    #[tokio::test]
    async fn artifact_bridge_creates_pending_pair_and_is_idempotent() {
        let dir = tempfile::tempdir().expect("temp");
        let root = dir.path().join("root");
        fs::create_dir_all(&root).unwrap();
        let root = fs::canonicalize(&root).unwrap();
        let processor = processor_with_project(&dir.path().join("ody"), "p1", &root).await;

        let response = processor
            .artifact_bridge(bridge_params("p1", "src/Hello.html", "bridge-1"))
            .await
            .expect("bridge");
        assert_eq!(response.bridge.status, WorkspaceArtifactBridgeStatus::Pending);
        assert_eq!(response.changeset.status, WorkspaceChangeSetStatus::Pending);
        assert_eq!(response.changeset.changes.len(), 1);
        assert!(response.bridge.id.starts_with("ab-"));

        // Idempotent retry returns the same pair without a second record.
        let retry = processor
            .artifact_bridge(bridge_params("p1", "src/Hello.html", "bridge-1"))
            .await
            .expect("bridge retry");
        assert_eq!(retry.bridge.id, response.bridge.id);

        let listed = processor
            .artifact_bridge_list(WorkspaceArtifactBridgeListParams {
                project_id: "p1".to_owned(),
            })
            .await
            .expect("list bridges");
        assert_eq!(listed.bridges.len(), 1);
        assert_eq!(listed.bridges[0].artifact_id.as_deref(), Some("artifact-1"));
    }

    #[tokio::test]
    async fn artifact_bridge_rejects_existing_target_escaping_path_and_oversize() {
        let dir = tempfile::tempdir().expect("temp");
        let root = dir.path().join("root");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/Hello.html"), "existing").unwrap();
        let root = fs::canonicalize(&root).unwrap();
        let processor = processor_with_project(&dir.path().join("ody"), "p1", &root).await;

        let existing = processor
            .artifact_bridge(bridge_params("p1", "src/Hello.html", "b-existing"))
            .await
            .expect_err("existing target must be rejected for Add-only v1");
        assert!(existing.message.contains("already exists"), "{}", existing.message);

        let escaping = processor
            .artifact_bridge(bridge_params("p1", "../out.html", "b-escape"))
            .await
            .expect_err("escaping path must be rejected");
        assert!(escaping.message.contains("escape"), "{}", escaping.message);

        let oversize = processor
            .artifact_bridge(WorkspaceArtifactBridgeParams {
                content: "x".repeat(MAX_ARTIFACT_BRIDGE_CONTENT_BYTES + 1),
                ..bridge_params("p1", "big.html", "b-big")
            })
            .await
            .expect_err("oversize content must be rejected");
        assert!(oversize.message.contains("exceeds"), "{}", oversize.message);
    }

    #[tokio::test]
    async fn artifact_bridge_status_mirrors_changeset_lifecycle() {
        let dir = tempfile::tempdir().expect("temp");
        let root = dir.path().join("root");
        fs::create_dir_all(&root).unwrap();
        let root = fs::canonicalize(&root).unwrap();
        let processor = processor_with_project(&dir.path().join("ody"), "p1", &root).await;

        let bridged = processor
            .artifact_bridge(bridge_params("p1", "Hello.html", "bridge-life"))
            .await
            .expect("bridge");
        let bridge_id = bridged.bridge.id.clone();
        let changeset_id = bridged.changeset.id.clone();

        processor
            .changeset_apply(
                WorkspaceChangeSetApplyParams {
                    changeset_id: changeset_id.clone(),
                    commit_message: None,
                },
                ConnectionId(1),
            )
            .await
            .expect("apply");
        let applied = processor
            .artifact_bridge_list(WorkspaceArtifactBridgeListParams {
                project_id: "p1".to_owned(),
            })
            .await
            .expect("list");
        let bridge = applied
            .bridges
            .iter()
            .find(|bridge| bridge.id == bridge_id)
            .expect("bridge record");
        assert_eq!(bridge.status, WorkspaceArtifactBridgeStatus::Applied);
        assert_eq!(
            fs::read_to_string(root.join("Hello.html")).expect("imported file"),
            "<html><body>hello</body></html>"
        );

        // Restore rolls the changeset and the bridge back together.
        processor
            .changeset_restore(
                WorkspaceChangeSetRestoreParams {
                    changeset_id: changeset_id.clone(),
                },
                ConnectionId(1),
            )
            .await
            .expect("restore");
        let restored = processor
            .artifact_bridge_list(WorkspaceArtifactBridgeListParams {
                project_id: "p1".to_owned(),
            })
            .await
            .expect("list");
        assert_eq!(
            restored.bridges[0].status,
            WorkspaceArtifactBridgeStatus::Pending
        );
    }

    #[tokio::test]
    async fn artifact_bridge_reject_marks_bridge_rejected() {
        let dir = tempfile::tempdir().expect("temp");
        let root = dir.path().join("root");
        fs::create_dir_all(&root).unwrap();
        let root = fs::canonicalize(&root).unwrap();
        let processor = processor_with_project(&dir.path().join("ody"), "p1", &root).await;

        let bridged = processor
            .artifact_bridge(bridge_params("p1", "Hello.html", "bridge-reject"))
            .await
            .expect("bridge");
        processor
            .changeset_reject(WorkspaceChangeSetRejectParams {
                changeset_id: bridged.changeset.id,
            })
            .await
            .expect("reject");
        let listed = processor
            .artifact_bridge_list(WorkspaceArtifactBridgeListParams {
                project_id: "p1".to_owned(),
            })
            .await
            .expect("list");
        assert_eq!(
            listed.bridges[0].status,
            WorkspaceArtifactBridgeStatus::Rejected
        );
    }

    #[tokio::test]
    async fn external_modified_event_invalidates_pending_changeset_on_hash_mismatch() {
        let dir = tempfile::tempdir().expect("temp");
        let root = dir.path().join("root");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/a.ts"), "old\n").unwrap();
        let root = fs::canonicalize(&root).unwrap();
        let base = crate::workspace_changeset::sha256_hex(b"old\n");
        let (processor, cs_id, _write_lock) = processor_with_project_and_pending_changeset(
            &dir.path().join("ody"),
            "p1",
            &root,
            vec![pending_update_change("old\n")],
        )
        .await;
        assert_eq!(base.len(), 64);
        // External IDE saves different content: hash differs from base.
        let new_hash = crate::workspace_changeset::sha256_hex(b"ide edit\n");
        let invalidated = processor.invalidate_from_external_events(
            "p1",
            &[WorkspaceFileEvent {
                root_index: 0,
                path: "src/a.ts".into(),
                kind: WorkspaceFileEventKind::Modified,
                content_hash: Some(new_hash),
            }],
        );
        assert_eq!(invalidated, vec![cs_id.clone()]);
        let cs = processor
            .with_store(|store| {
                store
                    .changesets
                    .get(&cs_id)
                    .map(|stored| stored.change_set.clone())
                    .ok_or_else(|| internal_error("missing"))
            })
            .expect("changeset");
        assert!(cs.invalidated_reason.as_deref().unwrap().contains("src/a.ts"));
    }

    #[tokio::test]
    async fn external_event_matching_base_hash_does_not_invalidate() {
        let dir = tempfile::tempdir().expect("temp");
        let root = dir.path().join("root");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/a.ts"), "old\n").unwrap();
        let root = fs::canonicalize(&root).unwrap();
        let base = crate::workspace_changeset::sha256_hex(b"old\n");
        let (processor, cs_id, _write_lock) = processor_with_project_and_pending_changeset(
            &dir.path().join("ody"),
            "p1",
            &root,
            vec![pending_update_change("old\n")],
        )
        .await;
        // Event hash == base_hash (IDE saved identical bytes): no conflict.
        let invalidated = processor.invalidate_from_external_events(
            "p1",
            &[WorkspaceFileEvent {
                root_index: 0,
                path: "src/a.ts".into(),
                kind: WorkspaceFileEventKind::Modified,
                content_hash: Some(base),
            }],
        );
        assert!(invalidated.is_empty(), "{invalidated:?}");
        let cs = processor
            .with_store(|store| {
                store
                    .changesets
                    .get(&cs_id)
                    .map(|stored| stored.change_set.invalidated_reason.clone())
                    .ok_or_else(|| internal_error("missing"))
            })
            .expect("changeset");
        assert_eq!(cs, None);
    }

    #[tokio::test]
    async fn apply_rejects_when_another_connection_holds_the_lock() {
        let dir = tempfile::tempdir().expect("temp");
        let root = dir.path().join("root");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/a.ts"), "old\n").unwrap();
        let root = fs::canonicalize(&root).unwrap();
        let (processor, cs_id, write_lock) = processor_with_project_and_pending_changeset(
            &dir.path().join("ody"),
            "p1",
            &root,
            vec![pending_update_change("old\n")],
        )
        .await;
        write_lock.acquire("p1", ConnectionId(7)).expect("foreign lock");
        let err = processor
            .changeset_apply(
                WorkspaceChangeSetApplyParams {
                    changeset_id: cs_id.clone(),
                    commit_message: None,
                },
                ConnectionId(8),
            )
            .await
            .expect_err("locked project must reject foreign apply");
        assert!(err.message.contains("locked by another session"), "got: {}", err.message);
        assert_eq!(fs::read_to_string(root.join("src/a.ts")).unwrap(), "old\n");
    }

    #[tokio::test]
    async fn apply_succeeds_while_same_connection_holds_the_lock() {
        let dir = tempfile::tempdir().expect("temp");
        let root = dir.path().join("root");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/a.ts"), "old\n").unwrap();
        let root = fs::canonicalize(&root).unwrap();
        let (processor, cs_id, write_lock) = processor_with_project_and_pending_changeset(
            &dir.path().join("ody"),
            "p1",
            &root,
            vec![pending_update_change("old\n")],
        )
        .await;
        write_lock.acquire("p1", ConnectionId(8)).expect("holder lock");
        processor
            .changeset_apply(
                WorkspaceChangeSetApplyParams {
                    changeset_id: cs_id.clone(),
                    commit_message: None,
                },
                ConnectionId(8),
            )
            .await
            .expect("holder applies");
        assert_eq!(fs::read_to_string(root.join("src/a.ts")).unwrap(), "new\n");
    }

    #[tokio::test]
    async fn apply_rejects_invalidated_changeset_before_file_verification() {
        let dir = tempfile::tempdir().expect("temp");
        let root = dir.path().join("root");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/a.ts"), "old\n").unwrap();
        let root = fs::canonicalize(&root).unwrap();
        let (processor, cs_id, _write_lock) = processor_with_project_and_pending_changeset(
            &dir.path().join("ody"),
            "p1",
            &root,
            vec![pending_update_change("old\n")],
        )
        .await;
        // Mark invalidated via a mismatched external event.
        let new_hash = crate::workspace_changeset::sha256_hex(b"ide edit\n");
        let invalidated = processor.invalidate_from_external_events(
            "p1",
            &[WorkspaceFileEvent {
                root_index: 0,
                path: "src/a.ts".into(),
                kind: WorkspaceFileEventKind::Modified,
                content_hash: Some(new_hash),
            }],
        );
        assert_eq!(invalidated.len(), 1);
        let err = processor
            .changeset_apply(
                WorkspaceChangeSetApplyParams {
                    changeset_id: cs_id.clone(),
                    commit_message: None,
                },
                ConnectionId(8),
            )
            .await
            .expect_err("invalidated changeset must not apply");
        assert!(err.message.contains("invalidated"), "got: {}", err.message);
        assert!(err.message.contains("src/a.ts"));
        // And the on-disk file must be untouched.
        assert_eq!(fs::read_to_string(root.join("src/a.ts")).unwrap(), "old\n");
    }

    #[tokio::test]
    async fn overflow_rescan_invalidates_pending_changeset() {
        let dir = tempfile::tempdir().expect("temp");
        let root = dir.path().join("root");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/a.ts"), "old\n").unwrap();
        let root = fs::canonicalize(&root).unwrap();
        let (processor, cs_id, _write_lock) = processor_with_project_and_pending_changeset(
            &dir.path().join("ody"),
            "p1",
            &root,
            vec![pending_update_change("old\n")],
        )
        .await;
        // External edit lands without any watcher event (truncated batch):
        // the full Pending rescan must still catch it from disk bytes.
        fs::write(root.join("src/a.ts"), "ide edit\n").unwrap();
        let invalidated = processor.invalidate_all_pending("p1");
        assert_eq!(invalidated, vec![cs_id.clone()]);
        let cs = processor
            .with_store(|store| {
                store
                    .changesets
                    .get(&cs_id)
                    .map(|stored| stored.change_set.invalidated_reason.clone())
                    .ok_or_else(|| internal_error("missing"))
            })
            .expect("changeset");
        assert!(cs.as_deref().unwrap().contains("src/a.ts"));
    }

    #[test]
    fn diff_response_keeps_legacy_primary_git_diff_field() {
        // Back-compat contract (ADR decision 9): the additive rootGitDiffs
        // aggregation must not disturb the primary-root gitDiff field shape.
        let response = WorkspaceSourceDiffResponse {
            changesets: vec![],
            git_diff: Some(WorkspaceGitDiff {
                repo_root: Some("/repo/frontend".to_owned()),
                available: true,
                error: None,
                unified_diff: Some("diff --git a/x b/x\n".to_owned()),
            }),
            root_git_diffs: vec![WorkspaceRootGitDiff {
                root_index: 0,
                git: WorkspaceGitDiff {
                    repo_root: Some("/repo/frontend".to_owned()),
                    available: true,
                    error: None,
                    unified_diff: Some("diff --git a/x b/x\n".to_owned()),
                },
            }],
        };
        assert!(response.git_diff.is_some());
        assert_eq!(response.root_git_diffs.len(), 1);
        assert_eq!(response.root_git_diffs[0].root_index, 0);
    }

    fn git_available() -> bool {
        std::process::Command::new("git")
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    fn git(repo: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .status()
            .expect("run git");
        assert!(status.success(), "git {args:?} failed with {status}");
    }

    fn init_repo_with_commit(repo: &Path) {
        git(repo, &["init", "-q", "."]);
        git(repo, &["config", "user.email", "ody@test"]);
        git(repo, &["config", "user.name", "ody test"]);
        fs::write(repo.join("tracked.txt"), "tracked line\n").expect("write tracked");
        git(repo, &["add", "tracked.txt"]);
        git(repo, &["commit", "-q", "-m", "initial"]);
    }

    #[tokio::test]
    async fn root_git_diff_includes_untracked_files_as_new_file_diffs() {
        if !git_available() {
            eprintln!("skipping: git binary unavailable");
            return;
        }
        let repo = tempfile::tempdir().expect("create repo");
        init_repo_with_commit(repo.path());
        // Tracked modification (covered by `git diff HEAD`).
        fs::write(repo.path().join("tracked.txt"), "tracked line changed\n")
            .expect("modify tracked");
        // Untracked additions (previously invisible in the whole-repo diff).
        fs::create_dir_all(repo.path().join("src")).expect("create src");
        fs::write(repo.path().join("src/new.ts"), "fresh untracked\n").expect("write untracked");
        // Ignored files must stay out of the untracked list.
        fs::write(repo.path().join(".gitignore"), "ignored.log\n").expect("write gitignore");
        fs::write(repo.path().join("ignored.log"), "secret\n").expect("write ignored");

        let diff = root_git_diff(repo.path()).await.expect("git repo detected");

        assert_eq!(diff.available, true);
        assert_eq!(diff.error, None);
        let unified = diff.unified_diff.expect("unified diff");
        assert!(
            unified.contains("-tracked line\n+tracked line changed"),
            "tracked diff: {unified}"
        );
        assert!(
            unified.contains("diff --git a/src/new.ts b/src/new.ts"),
            "untracked header: {unified}"
        );
        assert!(
            unified.contains("new file mode"),
            "new file marker: {unified}"
        );
        assert!(
            unified.contains("+fresh untracked"),
            "untracked body: {unified}"
        );
        assert!(
            !unified.contains("diff --git a/ignored.log"),
            "ignored leak: {unified}"
        );
    }

    #[tokio::test]
    async fn root_git_diff_serves_untracked_only_in_empty_repo_without_head() {
        if !git_available() {
            eprintln!("skipping: git binary unavailable");
            return;
        }
        let repo = tempfile::tempdir().expect("create repo");
        git(repo.path(), &["init", "-q", "."]);
        fs::write(repo.path().join("seed.ts"), "seed\n").expect("write untracked");

        let diff = root_git_diff(repo.path()).await.expect("git repo detected");

        assert_eq!(diff.available, true);
        let unified = diff.unified_diff.expect("unified diff");
        assert!(
            unified.contains("diff --git a/seed.ts b/seed.ts"),
            "{unified}"
        );
        assert!(unified.contains("+seed"), "{unified}");
    }

    #[tokio::test]
    async fn root_git_diff_caps_untracked_files_with_a_notice() {
        if !git_available() {
            eprintln!("skipping: git binary unavailable");
            return;
        }
        let repo = tempfile::tempdir().expect("create repo");
        init_repo_with_commit(repo.path());
        for index in 0..4 {
            fs::write(repo.path().join(format!("file{index}.ts")), "x\n").expect("write untracked");
        }

        let diff = root_git_diff_limited(repo.path(), 2)
            .await
            .expect("git repo detected");

        let unified = diff.unified_diff.expect("unified diff");
        let rendered = unified.matches("new file mode").count();
        assert!(
            rendered <= 2,
            "expected at most 2 untracked diffs: {unified}"
        );
        assert!(
            unified.contains("untracked files omitted"),
            "truncation notice: {unified}"
        );
    }
    // ---- P1 切片1（暂存写回闭环）：reject 还原 / apply commit / 自失效防护 ----

    fn conflict_fixture_change() -> WorkspaceFileChange {
        WorkspaceFileChange {
            root_index: 0,
            path: "src/a.ts".to_owned(),
            kind: WorkspaceFileChangeKind::Update,
            base_hash: Some(crate::workspace_changeset::sha256_hex(b"base\n")),
            content: Some("staged\n".to_owned()),
        }
    }

    #[test]
    fn conflicted_keys_ignore_event_matching_staged_content() {
        let change = conflict_fixture_change();
        let staged_hash = crate::workspace_changeset::sha256_hex(b"staged\n");
        let events = vec![WorkspaceFileEvent {
            root_index: 0,
            path: "src/a.ts".to_owned(),
            kind: WorkspaceFileEventKind::Modified,
            content_hash: Some(staged_hash),
        }];
        // 暂存写盘落地（hash == staged）不是外部冲突
        assert!(conflicted_change_keys(&[change], &events).is_empty());
    }

    #[test]
    fn conflicted_keys_ignore_added_event_matching_staged_add() {
        let change = WorkspaceFileChange {
            root_index: 0,
            path: "src/new.ts".to_owned(),
            kind: WorkspaceFileChangeKind::Add,
            base_hash: None,
            content: Some("staged\n".to_owned()),
        };
        let events = vec![WorkspaceFileEvent {
            root_index: 0,
            path: "src/new.ts".to_owned(),
            kind: WorkspaceFileEventKind::Added,
            content_hash: Some(crate::workspace_changeset::sha256_hex(b"staged\n")),
        }];
        assert!(conflicted_change_keys(&[change], &events).is_empty());
    }

    #[test]
    fn conflicted_keys_still_flag_user_edit_different_from_staged() {
        let change = conflict_fixture_change();
        let events = vec![WorkspaceFileEvent {
            root_index: 0,
            path: "src/a.ts".to_owned(),
            kind: WorkspaceFileEventKind::Modified,
            content_hash: Some(crate::workspace_changeset::sha256_hex(b"user edit\n")),
        }];
        assert_eq!(conflicted_change_keys(&[change], &events).len(), 1);
    }

    /// 造一个磁盘已是新内容（可逆写盘）的 pending changeset。
    async fn processor_with_disk_backed_changeset(
        project_id: &str,
        root: &Path,
        change: WorkspaceFileChange,
    ) -> WorkspaceSourceRequestProcessor {
        let ody_home = tempfile::tempdir().expect("ody home");
        let processor = processor_with_project(ody_home.path(), project_id, root).await;
        processor
            .changeset_create(WorkspaceChangeSetCreateParams {
                project_id: project_id.to_owned(),
                title: "disk backed".to_owned(),
                changes: vec![change],
                idempotency_key: format!("disk-backed-{}", uuid::Uuid::new_v4()),
            })
            .await
            .expect("create changeset");
        processor
    }

    fn update_change(base: &str, new: &str, path: &str) -> WorkspaceFileChange {
        WorkspaceFileChange {
            root_index: 0,
            path: path.to_owned(),
            kind: WorkspaceFileChangeKind::Update,
            base_hash: Some(crate::workspace_changeset::sha256_hex(base.as_bytes())),
            content: Some(new.to_owned()),
        }
    }

    fn only_changeset_id(processor: &WorkspaceSourceRequestProcessor) -> String {
        processor
            .store
            .lock()
            .expect("store lock")
            .changesets
            .keys()
            .next()
            .expect("id")
            .clone()
    }

    #[tokio::test]
    async fn reject_reverts_disk_content_back_to_base() {
        let root = tempfile::tempdir().expect("root");
        fs::create_dir_all(root.path().join("src")).expect("mkdir");
        fs::write(root.path().join("src/a.ts"), "base\n").expect("seed");
        let processor = processor_with_disk_backed_changeset(
            "ws-1",
            root.path(),
            update_change("base\n", "staged\n", "src/a.ts"),
        )
        .await;
        // 模拟 agent 已写盘（热更新生效中）
        fs::write(root.path().join("src/a.ts"), "staged\n").expect("agent write");
        let changeset_id = only_changeset_id(&processor);

        processor
            .changeset_reject(WorkspaceChangeSetRejectParams {
                changeset_id: changeset_id.clone(),
            })
            .await
            .expect("reject");

        assert_eq!(
            fs::read_to_string(root.path().join("src/a.ts")).expect("read"),
            "base\n",
            "reject 必须把磁盘还原回 base"
        );
        let store = processor.store.lock().expect("store lock");
        assert_eq!(
            store.changesets.get(&changeset_id).expect("stored").change_set.status,
            WorkspaceChangeSetStatus::Rejected
        );
    }

    #[tokio::test]
    async fn reject_leaves_further_modified_file_untouched() {
        let root = tempfile::tempdir().expect("root");
        fs::create_dir_all(root.path().join("src")).expect("mkdir");
        fs::write(root.path().join("src/a.ts"), "base\n").expect("seed");
        let processor = processor_with_disk_backed_changeset(
            "ws-1",
            root.path(),
            update_change("base\n", "staged\n", "src/a.ts"),
        )
        .await;
        // 用户在暂存之上又手动改了（≠ staged）：reject 不得覆盖用户工作
        fs::write(root.path().join("src/a.ts"), "user edit\n").expect("user edit");
        let changeset_id = only_changeset_id(&processor);

        processor
            .changeset_reject(WorkspaceChangeSetRejectParams { changeset_id })
            .await
            .expect("reject");

        assert_eq!(
            fs::read_to_string(root.path().join("src/a.ts")).expect("read"),
            "user edit\n"
        );
    }

    #[tokio::test]
    async fn reject_removes_added_file_matching_staged_content() {
        let root = tempfile::tempdir().expect("root");
        fs::create_dir_all(root.path().join("src")).expect("mkdir");
        let processor = processor_with_disk_backed_changeset(
            "ws-1",
            root.path(),
            WorkspaceFileChange {
                root_index: 0,
                path: "src/new.ts".to_owned(),
                kind: WorkspaceFileChangeKind::Add,
                base_hash: None,
                content: Some("added\n".to_owned()),
            },
        )
        .await;
        fs::write(root.path().join("src/new.ts"), "added\n").expect("agent add");
        let changeset_id = only_changeset_id(&processor);

        processor
            .changeset_reject(WorkspaceChangeSetRejectParams { changeset_id })
            .await
            .expect("reject");

        assert!(
            !root.path().join("src/new.ts").exists(),
            "reject 删除新增文件"
        );
    }

    #[tokio::test]
    async fn apply_with_commit_message_commits_changeset_files() {
        if !git_available() {
            eprintln!("skipping: git binary unavailable");
            return;
        }
        let repo = tempfile::tempdir().expect("repo");
        init_repo_with_commit(repo.path());
        fs::create_dir_all(repo.path().join("src")).expect("mkdir");
        fs::write(repo.path().join("src/a.ts"), "base\n").expect("seed");
        git(repo.path(), &["add", "src/a.ts"]);
        git(repo.path(), &["commit", "-q", "-m", "seed a"]);
        let processor = processor_with_disk_backed_changeset(
            "ws-1",
            repo.path(),
            update_change("base\n", "staged\n", "src/a.ts"),
        )
        .await;
        fs::write(repo.path().join("src/a.ts"), "staged\n").expect("agent write");
        let changeset_id = only_changeset_id(&processor);

        let response = processor
            .changeset_apply(
                WorkspaceChangeSetApplyParams {
                    changeset_id,
                    commit_message: Some("feat: apply staged change".to_owned()),
                },
                ConnectionId(1),
            )
            .await
            .expect("apply with commit");

        let commits = response.commit.expect("commit report");
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].root_path, repo.path().to_string_lossy());
        match &commits[0].outcome {
            WorkspaceChangeSetCommitOutcome::Committed { commit_hash } => {
                assert!(!commit_hash.is_empty());
            }
            other => panic!("expected Committed, got {other:?}"),
        }
        let log = std::process::Command::new("git")
            .args(["log", "--oneline", "-1"])
            .current_dir(repo.path())
            .output()
            .expect("git log");
        assert!(
            String::from_utf8_lossy(&log.stdout).contains("apply staged change"),
            "HEAD must be the apply commit"
        );
        let status = std::process::Command::new("git")
            .args(["status", "--porcelain", "--", "src/a.ts"])
            .current_dir(repo.path())
            .output()
            .expect("git status");
        assert!(
            String::from_utf8_lossy(&status.stdout).trim().is_empty(),
            "changeset paths must be clean after commit"
        );
    }

    #[tokio::test]
    async fn apply_commit_reports_nothing_to_commit_after_baseline_snapshot() {
        let root = tempfile::tempdir().expect("root");
        fs::create_dir_all(root.path().join("src")).expect("mkdir");
        fs::write(root.path().join("src/a.ts"), "base\n").expect("seed");
        let processor = processor_with_disk_backed_changeset(
            "ws-1",
            root.path(),
            update_change("base\n", "staged\n", "src/a.ts"),
        )
        .await;
        fs::write(root.path().join("src/a.ts"), "staged\n").expect("agent write");
        let changeset_id = only_changeset_id(&processor);

        let response = processor
            .changeset_apply(
                WorkspaceChangeSetApplyParams {
                    changeset_id,
                    commit_message: Some("feat: x".to_owned()),
                },
                ConnectionId(1),
            )
            .await
            .expect("apply with commit");

        let commits = response.commit.expect("commit report");
        assert_eq!(commits.len(), 1);
        // 非 git 工程 apply 时已把含暂存内容的磁盘快照成 baseline 仓库，
        // 对这些路径而言没有新增可提交内容。
        assert_eq!(
            commits[0].outcome,
            WorkspaceChangeSetCommitOutcome::NothingToCommit
        );
    }
}
