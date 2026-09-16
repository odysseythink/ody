//! 工程模式 Canvas P1 切片1：暂存收集器（可逆写盘语义）。
//!
//! Core 的文件写工具（write_file/edit_file/apply_patch）在写盘成功后经
//! [`ody_core::tools::staged_write::StagedWriteSink`] 通知本收集器；凡写入
//! 路径落在已绑定 workspace project 的 root 内，即累积进该 project 的
//! 「Task staging」暂存 changeset：base 为首次触碰前内容（只在写时进程内
//! 可得），磁盘本身已是新内容（热更新天然生效）。用户在 Canvas 确认
//! apply（changeset 协议 accept_applied 分支校验）+ 可选 commit；reject
//! 时由 processor 把磁盘还原回 base（见 changeset_reject_inner）。

use std::collections::VecDeque;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;

use ody_app_server_protocol::WorkspaceChangeSet;
use ody_app_server_protocol::WorkspaceChangeSetCheckpoint;
use ody_app_server_protocol::WorkspaceChangeSetStatus;
use ody_app_server_protocol::WorkspaceFileChange;
use ody_app_server_protocol::WorkspaceFileChangeKind;
use ody_app_server_protocol::WorkspaceProjectRef;
use ody_app_server_protocol::WORKSPACE_SOURCE_PROTOCOL_VERSION;
use ody_core::StagedWrite;
use ody_core::StagedWriteSink;

use crate::request_processors::workspace_project_processor::WorkspaceProjectStore;
use crate::request_processors::workspace_source_processor::StoredChangeSet;
use crate::request_processors::workspace_source_processor::WorkspaceSourceStore;
use crate::workspace_audit::WorkspaceAuditLog;

/// 「Task staging」标题前缀：暂存 changeset 的人工可识别标记。
pub(crate) const STAGING_TITLE_PREFIX: &str = "Task staging";

/// 防自失效环形缓冲容量：覆盖 watcher 事件滞后/乱序的窗口。
const RECENT_STAGED_CAP: usize = 128;

/// 把一个绝对路径归属到某个 project root，返回 (project, root_index, relative)。
/// 宽松 canonicalize：解析父目录的符号链接后拼回文件名。
/// 与 `Path::canonicalize` 不同，目标文件不存在（删除写入）也能工作。
pub(crate) fn canonicalize_loose(path: &std::path::Path) -> std::path::PathBuf {
    let Some(parent) = path.parent() else {
        return path.to_path_buf();
    };
    let Ok(dir) = std::fs::canonicalize(parent) else {
        return path.to_path_buf();
    };
    match path.file_name() {
        Some(name) => dir.join(name),
        None => dir,
    }
}

pub(crate) fn find_project_for_path<'a>(
    projects: impl IntoIterator<Item = &'a WorkspaceProjectRef>,
    path: &Path,
) -> Option<(WorkspaceProjectRef, u32, String)> {
    let path_str = path.to_string_lossy();
    // 最长 root 前缀优先；并列取 root_index 小者。多 root 工程（如 monorepo）
    // 下内层 root 必须赢过外层 Primary root。
    let mut best: Option<(usize, u32, WorkspaceProjectRef, String)> = None;
    for project in projects {
        for (index, root) in project.roots.iter().enumerate() {
            let root_path = root.path.trim_end_matches('/');
            let relative = if path_str == root_path {
                Some(String::new())
            } else {
                path_str
                    .strip_prefix(&format!("{root_path}/"))
                    .map(str::to_owned)
            };
            let Some(relative) = relative else {
                continue;
            };
            let index = index as u32;
            let better = best
                .as_ref()
                .map(|(best_len, best_index, best_project, _)| {
                    root_path.len() > *best_len
                        || (root_path.len() == *best_len
                            && (index < *best_index
                                || (index == *best_index && project.id < best_project.id)))
                })
                .unwrap_or(true);
            if better {
                best = Some((root_path.len(), index, project.clone(), relative));
            }
        }
    }
    best.map(|(_, index, project, relative)| (project, index, relative))
}

/// 从暂存写入构造一条 file change。
pub(crate) fn change_from_write(
    root_index: u32,
    relative: &str,
    write: &StagedWrite,
) -> WorkspaceFileChange {
    let kind = match (&write.old_content, &write.new_content) {
        (None, Some(_)) => WorkspaceFileChangeKind::Add,
        (Some(_), None) => WorkspaceFileChangeKind::Delete,
        (Some(_), Some(_)) => WorkspaceFileChangeKind::Update,
        (None, None) => WorkspaceFileChangeKind::Delete,
    };
    WorkspaceFileChange {
        root_index,
        path: relative.to_owned(),
        kind,
        base_hash: write
            .old_content
            .as_deref()
            .map(|old| crate::workspace_changeset::sha256_hex(old.as_bytes())),
        content: write.new_content.clone(),
    }
}

/// 用 store 里保存的 base_contents 重算 unified diff（不读磁盘——磁盘已是
/// 新内容，base 只在写时快照里）。
pub(crate) fn render_stored_diff(stored: &StoredChangeSet) -> String {
    stored
        .change_set
        .changes
        .iter()
        .map(|change| {
            let key = format!("{}:{}", change.root_index, change.path);
            crate::workspace_changeset::render_file_diff(
                &change.path,
                stored.base_contents.get(&key).map(String::as_str),
                change.content.as_deref(),
            )
        })
        .collect::<Vec<_>>()
        .join("")
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 收集器：把 core 的写通知累积为 project 级暂存 changeset。
pub(crate) struct WorkspaceStagingCollector {
    project_store: Arc<Mutex<WorkspaceProjectStore>>,
    store: Arc<Mutex<WorkspaceSourceStore>>,
    #[allow(dead_code)]
    audit: Arc<WorkspaceAuditLog>,
    /// project_id -> 当前打开的暂存 changeset id。
    open: Mutex<std::collections::BTreeMap<String, String>>,
    /// 最近暂存写盘的 (key, new_content_hash)，供 watcher 冲突检测排除自身。
    recent_staged: Mutex<VecDeque<(String, String)>>,
}

impl WorkspaceStagingCollector {
    pub(crate) fn new(
        project_store: Arc<Mutex<WorkspaceProjectStore>>,
        store: Arc<Mutex<WorkspaceSourceStore>>,
        audit: Arc<WorkspaceAuditLog>,
    ) -> Self {
        Self {
            project_store,
            store,
            audit,
            open: Mutex::new(std::collections::BTreeMap::new()),
            recent_staged: Mutex::new(VecDeque::new()),
        }
    }

    fn record_recent_staged(&self, key: String, content_hash: String) {
        let mut recent = self.recent_staged.lock().expect("recent lock");
        recent.push_back((key, content_hash));
        while recent.len() > RECENT_STAGED_CAP {
            recent.pop_front();
        }
    }

    /// 外部 watcher 事件询问：该 (key, content_hash) 是否正是本会话最近的
    /// 暂存写盘落地（中间代内容，非用户外部编辑）。
    pub(crate) fn is_recent_staged(&self, key: &str, content_hash: &str) -> bool {
        self.recent_staged
            .lock()
            .expect("recent lock")
            .iter()
            .any(|(k, h)| k == key && h == content_hash)
    }

    /// 找 project 的打开暂存 changeset；失效（已 apply/reject/invalidate）
    /// 则返回 None 并清掉记录。调用方必须已持有 store 锁。
    fn open_changeset_id(&self, store: &WorkspaceSourceStore, project_id: &str) -> Option<String> {
        let id = self.open.lock().expect("open lock").get(project_id)?.clone();
        let valid = store.changesets.get(&id).is_some_and(|stored| {
            stored.change_set.project_id == project_id
                && stored.change_set.status == WorkspaceChangeSetStatus::Pending
                && stored.change_set.invalidated_reason.is_none()
        });
        if valid {
            Some(id)
        } else {
            self.open.lock().expect("open lock").remove(project_id);
            None
        }
    }

    /// Sink 入口：core 每次文件写成功都会调用。必须便宜：store 为内存
    /// Mutex，persist 是小 JSON 原子写。
    pub(crate) fn stage_write(&self, session_id: &str, write: StagedWrite) {
        // bind 时 roots 经 canonicalize 落库，而写路径来自 core 的
        // cwd 拼接（可能带符号链接分量，如 macOS /tmp -> /private/tmp），
        // 先做宽松 canonicalize 再归属，否则前缀匹配必失败。
        let path = canonicalize_loose(&write.path);
        let projects = {
            let store = self.project_store.lock().expect("project store lock");
            store.projects.values().cloned().collect::<Vec<_>>()
        };
        let Some((project, root_index, relative)) =
            find_project_for_path(&projects, &path)
        else {
            return;
        };
        let write = StagedWrite { path, ..write };
        let key = format!("{root_index}:{relative}");
        let new_hash = write
            .new_content
            .as_deref()
            .map(|new| crate::workspace_changeset::sha256_hex(new.as_bytes()));

        let mut store = self.store.lock().expect("store lock");
        let changeset_id = match self.open_changeset_id(&store, &project.id) {
            Some(id) => id,
            None => {
                let id = self.create_staging_changeset(
                    &mut store,
                    &project,
                    session_id,
                    root_index,
                    &relative,
                    &write,
                );
                self.open
                    .lock()
                    .expect("open lock")
                    .insert(project.id.clone(), id.clone());
                id
            }
        };
        let Some(stored) = store.changesets.get_mut(&changeset_id) else {
            return;
        };
        match stored
            .change_set
            .changes
            .iter_mut()
            .find(|change| change.root_index == root_index && change.path == relative)
        {
            Some(existing) => {
                // 同文件多次写：保留首次 base，只滚动 content。
                existing.kind = change_from_write(root_index, &relative, &write).kind;
                existing.content = write.new_content.clone();
                stored.change_set.updated_at_ms = now_ms();
            }
            None => {
                let change = change_from_write(root_index, &relative, &write);
                if let Some(old) = &write.old_content {
                    stored
                        .base_contents
                        .insert(key.clone(), old.clone());
                }
                stored.targets.insert(
                    key.clone(),
                    write.path.to_string_lossy().into_owned(),
                );
                stored.change_set.changes.push(change);
                stored.change_set.updated_at_ms = now_ms();
            }
        }
        stored.change_set.unified_diff = render_stored_diff(stored);
        let _ = store.persist();
        if let Some(hash) = new_hash {
            self.record_recent_staged(key, hash);
        }
    }

    fn create_staging_changeset(
        &self,
        store: &mut WorkspaceSourceStore,
        project: &WorkspaceProjectRef,
        session_id: &str,
        root_index: u32,
        relative: &str,
        write: &StagedWrite,
    ) -> String {
        let id = format!("cs-{}", uuid::Uuid::new_v4());
        let now = now_ms();
        let key = format!("{root_index}:{relative}");
        let change = change_from_write(root_index, relative, write);
        let unified_diff = crate::workspace_changeset::render_file_diff(
            relative,
            write.old_content.as_deref(),
            write.new_content.as_deref(),
        );
        let base_contents = write
            .old_content
            .clone()
            .map(|old| (key.clone(), old))
            .into_iter()
            .collect();
        let stored = StoredChangeSet {
            change_set: WorkspaceChangeSet {
                id: id.clone(),
                project_id: project.id.clone(),
                title: staging_title(session_id),
                schema_version: WORKSPACE_SOURCE_PROTOCOL_VERSION,
                changes: vec![change],
                status: WorkspaceChangeSetStatus::Pending,
                checkpoint: WorkspaceChangeSetCheckpoint::None,
                unified_diff,
                created_at_ms: now,
                updated_at_ms: now,
                applied_at_ms: None,
                invalidated_reason: None,
            },
            base_contents,
            applied_hashes: Default::default(),
            targets: std::collections::BTreeMap::from([(
                key,
                write.path.to_string_lossy().into_owned(),
            )]),
        };
        store.changesets.insert(id.clone(), stored);
        let _ = store.persist();
        id
    }
}

fn staging_title(session_id: &str) -> String {
    format!("{STAGING_TITLE_PREFIX} · {session_id}")
}

impl StagedWriteSink for WorkspaceStagingCollector {
    fn staged_write(&self, session_id: &str, write: StagedWrite) {
        self.stage_write(session_id, write);
    }
}

#[cfg(test)]
mod tests;
