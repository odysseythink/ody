//! Project-level workspace watcher: recursive root watches with
//! .gitignore-aware event filtering, content-hash snapshots, and broadcast
//! `workspace/changed` notifications (E4). Broadcast (not connection-targeted)
//! is deliberate: every window must see external edits (multi-window
//! visibility, strategy E4 concurrency constraints).

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use ignore::gitignore::Gitignore;
use ignore::gitignore::GitignoreBuilder;
use ody_app_server_protocol::JSONRPCErrorError;
use ody_app_server_protocol::ServerNotification;
use ody_app_server_protocol::WorkspaceChangedNotification;
use ody_app_server_protocol::WorkspaceFileEvent;
use ody_app_server_protocol::WorkspaceFileEventKind;
use ody_app_server_protocol::WorkspaceUnwatchParams;
use ody_app_server_protocol::WorkspaceUnwatchResponse;
use ody_app_server_protocol::WorkspaceWatchParams;
use ody_app_server_protocol::WorkspaceWatchResponse;
use ody_file_watcher::DebouncedWatchReceiver;
use ody_file_watcher::FileWatcher;
use ody_file_watcher::FileWatcherSubscriber;
use ody_file_watcher::WatchPath;
use ody_file_watcher::WatchRegistration;
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::oneshot;

use crate::error_code::internal_error;
use crate::error_code::invalid_params;
use crate::outgoing_message::ConnectionId;
use crate::outgoing_message::OutgoingMessageSender;
use crate::request_processors::workspace_project_processor::WorkspaceProjectStore;

/// Debounce window for external-change batching (fs_watch precedent).
const WATCH_DEBOUNCE: Duration = Duration::from_millis(200);
/// Cap per notification; excess folds into `overflow: true` (risk R1).
pub(crate) const MAX_WATCH_EVENTS_PER_NOTIFICATION: usize = 1000;
/// Per-connection project watch cap (risk R2).
const MAX_WATCHES_PER_CONNECTION: usize = 32;
/// Default-ignored top-level entries, mirroring the E0 scan skip list.
const DEFAULT_IGNORED: &[&str] = &[
    ".git", "node_modules", "dist", "build", "out", "target",
    ".next", ".nuxt", ".output", "coverage", ".cache", ".turbo",
];
/// Cap for the initial content-hash snapshot walk; beyond this the snapshot
/// starts empty and the first events classify as Added (degraded but safe).
const MAX_SNAPSHOT_ENTRIES: usize = 200_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum EventClass {
    Pass,
    Drop,
}

/// Build the per-root matcher: <root>/.gitignore (if present) over defaults.
pub(crate) fn root_matcher(root: &Path) -> Result<Gitignore, JSONRPCErrorError> {
    let mut builder = GitignoreBuilder::new(root);
    for pattern in DEFAULT_IGNORED {
        // Leading slash anchors to root; trailing /* covers the directory tree.
        builder.add_line(None, &format!("/{pattern}")).map_err(|err| {
            internal_error(format!("workspace watch default ignore: {err}"))
        })?;
        builder.add_line(None, &format!("/{pattern}/*")).map_err(|err| {
            internal_error(format!("workspace watch default ignore: {err}"))
        })?;
    }
    let gitignore_path = root.join(".gitignore");
    if gitignore_path.is_file() {
        builder.add(&gitignore_path);
    }
    builder.build().map_err(|err| {
        internal_error(format!("workspace watch ignore build failed: {err}"))
    })
}

/// Classify one root-relative `/`-joined path.
pub(crate) fn classify_event_path(
    matcher: &Gitignore,
    root: &Path,
    relative: &str,
) -> EventClass {
    let path = root.join(relative);
    let is_dir = path.is_dir();
    // matched_path_or_any_parents: a dir pattern like `ignoredir/` must also
    // drop everything beneath it, mirroring git's check-ignore semantics.
    if matcher
        .matched_path_or_any_parents(relative, is_dir)
        .is_ignore()
    {
        return EventClass::Drop;
    }
    EventClass::Pass
}

/// Map + filter + cap raw watch paths. Returns (events, overflow).
/// Content hashes are computed by the async caller, not here (sync fn).
pub(crate) fn fold_events(
    matcher: &Gitignore,
    root: &Path,
    raw: impl Iterator<Item = (String, WorkspaceFileEventKind)>,
) -> (Vec<(String, WorkspaceFileEventKind)>, bool) {
    let mut kept: Vec<(String, WorkspaceFileEventKind)> = Vec::new();
    let mut overflow = false;
    for (relative, kind) in raw {
        if classify_event_path(matcher, root, &relative) == EventClass::Drop {
            continue;
        }
        if kept.len() >= MAX_WATCH_EVENTS_PER_NOTIFICATION {
            overflow = true;
            continue;
        }
        kept.push((relative, kind));
    }
    (kept, overflow)
}

/// Collapse duplicate paths in one window, keeping the latest kind.
fn dedup_latest(
    raw: impl Iterator<Item = (String, WorkspaceFileEventKind)>,
) -> impl Iterator<Item = (String, WorkspaceFileEventKind)> {
    let mut latest: std::collections::BTreeMap<String, WorkspaceFileEventKind> =
        std::collections::BTreeMap::new();
    for (path, kind) in raw {
        latest.insert(path, kind);
    }
    latest.into_iter()
}

/// Snapshot key: (root index, root-relative path) -> content sha256.
type Snapshot = HashMap<(u32, String), String>;

/// Resolve a debounced event path to a root-relative `/`-joined string.
///
/// The file-watcher reports paths in the requested (registration) namespace,
/// i.e. relative to the registered root. Absolute spellings (synthetic
/// events, symlink escapes) are stripped when possible.
fn resolve_relative(root: &Path, changed: &Path) -> Option<String> {
    let relative = if changed.is_absolute() {
        changed.strip_prefix(root).ok()?
    } else {
        changed
    };
    let relative = relative.to_string_lossy().replace('\\', "/");
    if relative.is_empty() { None } else { Some(relative) }
}

/// Build the initial per-root content-hash snapshot so the first event for a
/// pre-existing file classifies as Modified (not Added). Ignored directories
/// are pruned during the walk; the walk is capped at MAX_SNAPSHOT_ENTRIES.
fn build_initial_snapshot(matchers: &[(PathBuf, Gitignore)]) -> Snapshot {
    let mut snapshot = Snapshot::new();
    for (root_index, (root_path, matcher)) in matchers.iter().enumerate() {
        let root_index = root_index as u32;
        let filter_root = root_path.clone();
        let filter_matcher = matcher.clone();
        let walker = ignore::WalkBuilder::new(root_path)
            .standard_filters(false)
            .hidden(false)
            .filter_entry(move |entry| {
                let Ok(relative) = entry.path().strip_prefix(&filter_root) else {
                    return true;
                };
                let relative = relative.to_string_lossy().replace('\\', "/");
                if relative.is_empty() {
                    return true;
                }
                classify_event_path(&filter_matcher, &filter_root, &relative) == EventClass::Pass
            })
            .build();
        for entry in walker {
            let Ok(entry) = entry else { continue };
            if !entry.file_type().is_some_and(|file_type| file_type.is_file()) {
                continue;
            }
            let Ok(relative) = entry.path().strip_prefix(root_path) else {
                continue;
            };
            let relative = relative.to_string_lossy().replace('\\', "/");
            if relative.is_empty() {
                continue;
            }
            if snapshot.len() >= MAX_SNAPSHOT_ENTRIES {
                tracing::warn!(
                    "workspace watch snapshot cap {MAX_SNAPSHOT_ENTRIES} reached at {relative}"
                );
                return snapshot;
            }
            if let Ok(bytes) = std::fs::read(entry.path()) {
                snapshot.insert(
                    (root_index, relative),
                    crate::workspace_changeset::sha256_hex(&bytes),
                );
            }
        }
    }
    snapshot
}

struct WatchEntry {
    _subscriber: FileWatcherSubscriber,
    _registrations: Vec<WatchRegistration>,
    terminate_tx: oneshot::Sender<oneshot::Sender<()>>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct WatchKey {
    connection_id: ConnectionId,
    project_id: String,
}

#[derive(Default)]
struct WatchState {
    entries: HashMap<WatchKey, WatchEntry>,
}

/// E4: watcher -> source-processor conflict hook. Receives the project id,
/// the classified event batch, and the overflow flag (true when the batch
/// was truncated at MAX_WATCH_EVENTS_PER_NOTIFICATION, in which case the
/// callee should re-verify all Pending changesets on disk).
pub(crate) type ExternalChangeSink =
    Arc<dyn Fn(&str, &[WorkspaceFileEvent], bool) -> Vec<String> + Send + Sync>;

#[derive(Clone)]
pub(crate) struct WorkspaceWatchManager {
    outgoing: Arc<OutgoingMessageSender>,
    file_watcher: Arc<FileWatcher>,
    project_store: Arc<Mutex<WorkspaceProjectStore>>,
    conflict_sink: ExternalChangeSink,
    state: Arc<AsyncMutex<WatchState>>,
}

impl WorkspaceWatchManager {
    pub(crate) fn new(
        outgoing: Arc<OutgoingMessageSender>,
        file_watcher: Arc<FileWatcher>,
        project_store: Arc<Mutex<WorkspaceProjectStore>>,
        conflict_sink: ExternalChangeSink,
    ) -> Self {
        Self {
            outgoing,
            file_watcher,
            project_store,
            conflict_sink,
            state: Arc::new(AsyncMutex::new(WatchState::default())),
        }
    }

    pub(crate) async fn watch(
        &self,
        connection_id: ConnectionId,
        params: WorkspaceWatchParams,
    ) -> Result<WorkspaceWatchResponse, JSONRPCErrorError> {
        let key = WatchKey {
            connection_id,
            project_id: params.project_id.clone(),
        };
        let project = self
            .project_store
            .lock()
            .map_err(|_| internal_error("workspace project store lock poisoned"))?
            .projects
            .get(&params.project_id)
            .cloned()
            .ok_or_else(|| invalid_params(format!("unknown project {}", params.project_id)))?;
        {
            let state = self.state.lock().await;
            if state.entries.contains_key(&key) {
                return Err(invalid_params(format!(
                    "project {} is already watched by this connection",
                    params.project_id
                )));
            }
            let per_connection = state
                .entries
                .keys()
                .filter(|k| k.connection_id == connection_id)
                .count();
            if per_connection >= MAX_WATCHES_PER_CONNECTION {
                return Err(invalid_params(format!(
                    "watch limit {MAX_WATCHES_PER_CONNECTION} reached for this connection"
                )));
            }
        }
        let (subscriber, rx) = self.file_watcher.add_subscriber();
        let mut registrations = Vec::with_capacity(project.roots.len());
        let mut matchers = Vec::with_capacity(project.roots.len());
        for root in project.roots.iter() {
            let root_path = PathBuf::from(&root.path);
            // Recursive registration is deliberate: non-recursive top-level
            // registration would miss files in directories created after the
            // watch (new src/ trees). Noise is handled by event-layer ignore
            // filtering + debounce + overflow folding (risk R1).
            registrations.push(subscriber.register_paths(vec![WatchPath {
                path: root_path.clone(),
                recursive: true,
            }]));
            matchers.push((root_path.clone(), root_matcher(&root_path)?));
        }
        let snapshot = Arc::new(Mutex::new(build_initial_snapshot(&matchers)));
        let (terminate_tx, terminate_rx) = oneshot::channel();
        self.state.lock().await.entries.insert(
            key,
            WatchEntry {
                _subscriber: subscriber,
                _registrations: registrations,
                terminate_tx,
            },
        );

        let outgoing = self.outgoing.clone();
        let conflict_sink = self.conflict_sink.clone();
        let project_id = params.project_id.clone();
        tokio::spawn(async move {
            let mut rx = DebouncedWatchReceiver::new(rx, WATCH_DEBOUNCE);
            tokio::pin!(terminate_rx);
            loop {
                let event = tokio::select! {
                    biased;
                    _ = &mut terminate_rx => break,
                    event = rx.recv() => match event {
                        Some(event) => event,
                        None => break,
                    },
                };
                // Classify each notify path against the owning root matcher and
                // the content-hash snapshot. The file-watcher crate coalesces
                // events to paths only (no create/modify/delete kind), so the
                // kind is derived here: missing file -> Removed (only if we
                // witnessed it before), known path -> Modified, new path ->
                // Added.
                let mut classified: Vec<(u32, String, WorkspaceFileEventKind)> = Vec::new();
                {
                    let mut snapshot = snapshot.lock().expect("watch snapshot lock");
                    for (root_index, (root_path, matcher)) in matchers.iter().enumerate() {
                        let root_index = root_index as u32;
                        for changed in &event.paths {
                            let Some(relative) = resolve_relative(root_path, changed) else {
                                continue;
                            };
                            if classify_event_path(matcher, root_path, &relative)
                                == EventClass::Drop
                            {
                                continue;
                            }
                            let key = (root_index, relative.clone());
                            let full_path = root_path.join(&relative);
                            let kind = if !full_path.exists() {
                                // Only report removals for paths we have seen;
                                // a transient create+delete the client never
                                // knew about is not a change.
                                if snapshot.remove(&key).is_none() {
                                    continue;
                                }
                                WorkspaceFileEventKind::Removed
                            } else {
                                let existed = snapshot.contains_key(&key);
                                if let Ok(bytes) = std::fs::read(&full_path) {
                                    snapshot
                                        .insert(key, crate::workspace_changeset::sha256_hex(&bytes));
                                }
                                if existed {
                                    WorkspaceFileEventKind::Modified
                                } else {
                                    WorkspaceFileEventKind::Added
                                }
                            };
                            classified.push((root_index, relative, kind));
                        }
                    }
                }
                // Per-root fold with shared cap.
                let mut changes: Vec<WorkspaceFileEvent> = Vec::new();
                let mut overflow = false;
                for (root_index, (root_path, matcher)) in matchers.iter().enumerate() {
                    let root_index = root_index as u32;
                    let root_raw = classified
                        .iter()
                        .filter(|(i, _, _)| *i == root_index)
                        .map(|(_, p, k)| (p.clone(), *k));
                    let (folded, root_overflow) =
                        fold_events(matcher, root_path, dedup_latest(root_raw));
                    overflow |= root_overflow;
                    for (relative, kind) in folded {
                        if changes.len() >= MAX_WATCH_EVENTS_PER_NOTIFICATION {
                            overflow = true;
                            break;
                        }
                        let content_hash = match kind {
                            WorkspaceFileEventKind::Removed => None,
                            _ => std::fs::read(root_path.join(&relative))
                                .ok()
                                .map(|bytes| crate::workspace_changeset::sha256_hex(&bytes)),
                        };
                        changes.push(WorkspaceFileEvent {
                            root_index,
                            path: relative,
                            kind,
                            content_hash,
                        });
                    }
                }
                if changes.is_empty() && !overflow {
                    continue;
                }
                // E4: cross-reference the batch against Pending changesets
                // before broadcasting; invalidated ids ride the same
                // notification so clients can prompt re-create. On overflow
                // the sink re-verifies every Pending changeset on disk
                // (the truncated batch cannot be trusted to be complete).
                let invalidated = conflict_sink(&project_id, &changes, overflow);
                outgoing
                    .send_server_notification(ServerNotification::WorkspaceChanged(
                        WorkspaceChangedNotification {
                            project_id: project_id.clone(),
                            changes,
                            overflow,
                            invalidated_changesets: invalidated,
                        },
                    ))
                    .await;
            }
        });

        Ok(WorkspaceWatchResponse {
            project_id: params.project_id,
            watched_roots: (0..project.roots.len() as u32).collect(),
        })
    }

    pub(crate) async fn unwatch(
        &self,
        connection_id: ConnectionId,
        params: WorkspaceUnwatchParams,
    ) -> Result<WorkspaceUnwatchResponse, JSONRPCErrorError> {
        let key = WatchKey {
            connection_id,
            project_id: params.project_id.clone(),
        };
        let entry = self.state.lock().await.entries.remove(&key);
        if let Some(entry) = entry {
            let (done_tx, done_rx) = oneshot::channel();
            let _ = entry.terminate_tx.send(done_tx);
            let _ = done_rx.await;
        }
        Ok(WorkspaceUnwatchResponse {
            project_id: params.project_id,
        })
    }

    pub(crate) async fn connection_closed(&self, connection_id: ConnectionId) {
        let keys: Vec<WatchKey> = self
            .state
            .lock()
            .await
            .entries
            .extract_if(|key, _| key.connection_id == connection_id)
            .map(|(key, _)| key)
            .collect();
        for key in keys {
            let _ = self
                .unwatch(
                    connection_id,
                    WorkspaceUnwatchParams {
                        project_id: key.project_id,
                    },
                )
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn root_with_gitignore() -> TempDir {
        let dir = TempDir::new().expect("temp root");
        fs::write(dir.path().join(".gitignore"), "*.log\nignoredir/\n").expect("gitignore");
        fs::create_dir_all(dir.path().join("ignoredir")).expect("ignoredir");
        fs::create_dir_all(dir.path().join("src")).expect("src");
        fs::write(dir.path().join("src/app.ts"), "export const a = 1;\n").expect("app.ts");
        fs::write(dir.path().join("debug.log"), "noise\n").expect("debug.log");
        fs::write(dir.path().join("ignoredir/x.ts"), "noise\n").expect("ignored file");
        dir
    }

    #[test]
    fn filter_drops_gitignored_and_default_ignored_paths() {
        let dir = root_with_gitignore();
        let matcher = root_matcher(dir.path()).expect("matcher");
        // Must-survive inputs: none of these may be filtered out.
        assert_eq!(
            classify_event_path(&matcher, dir.path(), "src/app.ts"),
            EventClass::Pass
        );
        assert_eq!(
            classify_event_path(&matcher, dir.path(), "src/new.ts"),
            EventClass::Pass
        );
        assert_eq!(
            classify_event_path(&matcher, dir.path(), ".gitignore"),
            EventClass::Pass
        );
        // Root-anchored defaults must not catch a source file named like a
        // noise directory.
        assert_eq!(
            classify_event_path(&matcher, dir.path(), "src/dist.ts"),
            EventClass::Pass
        );
        // Must-drop inputs.
        assert_eq!(
            classify_event_path(&matcher, dir.path(), "debug.log"),
            EventClass::Drop
        );
        assert_eq!(
            classify_event_path(&matcher, dir.path(), "ignoredir/x.ts"),
            EventClass::Drop
        );
        assert_eq!(
            classify_event_path(&matcher, dir.path(), "node_modules/pkg/index.js"),
            EventClass::Drop
        );
        assert_eq!(
            classify_event_path(&matcher, dir.path(), ".git/HEAD"),
            EventClass::Drop
        );
        assert_eq!(
            classify_event_path(&matcher, dir.path(), "dist/bundle.js"),
            EventClass::Drop
        );
    }

    #[test]
    fn fold_caps_events_and_sets_overflow() {
        let dir = root_with_gitignore();
        let matcher = root_matcher(dir.path()).expect("matcher");
        let paths: Vec<String> = (0..MAX_WATCH_EVENTS_PER_NOTIFICATION + 500)
            .map(|i| format!("src/f{i}.ts"))
            .collect();
        let (events, overflow) = fold_events(
            &matcher,
            dir.path(),
            paths
                .iter()
                .map(|p| (p.clone(), WorkspaceFileEventKind::Modified)),
        );
        assert!(overflow, "over-cap window must set overflow");
        assert_eq!(events.len(), MAX_WATCH_EVENTS_PER_NOTIFICATION);
        // Deterministic truncation: lexicographically smallest paths survive.
        assert_eq!(events.first().expect("first").0, "src/f0.ts");
    }

    #[test]
    fn initial_snapshot_hashes_existing_files_and_prunes_ignored() {
        let dir = root_with_gitignore();
        let matcher = root_matcher(dir.path()).expect("matcher");
        let matchers = vec![(dir.path().to_path_buf(), matcher)];
        let snapshot = build_initial_snapshot(&matchers);
        let key = (0u32, "src/app.ts".to_string());
        let expected = crate::workspace_changeset::sha256_hex(b"export const a = 1;\n");
        assert_eq!(snapshot.get(&key).map(String::as_str), Some(expected.as_str()));
        assert!(!snapshot.contains_key(&(0, "debug.log".to_string())));
        assert!(!snapshot.contains_key(&(0, "ignoredir/x.ts".to_string())));
    }

    async fn project_store_with_root(path: &Path) -> Arc<Mutex<WorkspaceProjectStore>> {
        use crate::request_processors::workspace_project_processor::WorkspaceProjectRequestProcessor;
        use ody_app_server_protocol::WorkspaceProjectBindParams;
        use ody_utils_absolute_path::AbsolutePathBuf;

        let ody_home = TempDir::new().expect("ody home");
        let audit = Arc::new(crate::workspace_audit::WorkspaceAuditLog::new(
            ody_home.path().join("workspace-audit").join("v1.jsonl"),
        ));
        let processor =
            WorkspaceProjectRequestProcessor::new(ody_home.path().to_path_buf(), audit);
        let root =
            AbsolutePathBuf::try_from(path.to_path_buf()).expect("root should be absolute");
        processor
            .bind(WorkspaceProjectBindParams {
                id: "p1".to_string(),
                name: "p1".to_string(),
                roots: vec![root],
                idempotency_key: "bind-1".to_string(),
            })
            .await
            .expect("bind should succeed");
        processor.store_handle()
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn external_write_emits_changed_with_content_hash() {
        let dir = root_with_gitignore();
        let (outgoing, mut rx) = test_outgoing();
        let file_watcher = Arc::new(FileWatcher::new().expect("watcher"));
        let project_store = project_store_with_root(dir.path()).await;
        let manager = WorkspaceWatchManager::new(
            outgoing,
            file_watcher,
            project_store,
            Arc::new(|_, _, _| Vec::new()),
        );
        manager
            .watch(
                ConnectionId(1),
                WorkspaceWatchParams {
                    project_id: "p1".into(),
                },
            )
            .await
            .expect("watch");
        std::fs::write(dir.path().join("src/app.ts"), "export const a = 2;\n").expect("edit");
        let notification = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let envelope = rx.recv().await.expect("envelope");
                let crate::outgoing_message::OutgoingEnvelope::Broadcast { message } = envelope
                else {
                    continue;
                };
                let crate::outgoing_message::OutgoingMessage::AppServerNotification(
                    ServerNotification::WorkspaceChanged(changed),
                ) = message
                else {
                    continue;
                };
                return changed;
            }
        })
        .await
        .expect("timed out waiting for workspace/changed");
        assert_eq!(notification.project_id, "p1");
        assert!(!notification.overflow);
        let app_change = notification
            .changes
            .iter()
            .find(|c| c.path == "src/app.ts")
            .expect("src/app.ts change");
        assert_eq!(app_change.kind, WorkspaceFileEventKind::Modified);
        let expected = crate::workspace_changeset::sha256_hex(b"export const a = 2;\n");
        assert_eq!(app_change.content_hash.as_deref(), Some(expected.as_str()));
        assert!(notification.changes.iter().all(|c| c.path != "debug.log"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sink_receives_batch_and_notification_carries_invalidated_ids() {
        let dir = root_with_gitignore();
        let (outgoing, mut rx) = test_outgoing();
        let file_watcher = Arc::new(FileWatcher::new().expect("watcher"));
        let project_store = project_store_with_root(dir.path()).await;
        // Controlled stand-in for the source processor's conflict check:
        // records the batch it received and reports one invalidated id.
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen_for_sink = Arc::clone(&seen);
        let manager = WorkspaceWatchManager::new(
            outgoing,
            file_watcher,
            project_store,
            Arc::new(move |project_id, events, overflow| {
                seen_for_sink
                    .lock()
                    .expect("seen lock")
                    .push((project_id.to_owned(), events.len(), overflow));
                vec!["cs-fake".to_owned()]
            }),
        );
        manager
            .watch(
                ConnectionId(1),
                WorkspaceWatchParams {
                    project_id: "p1".into(),
                },
            )
            .await
            .expect("watch");
        std::fs::write(dir.path().join("src/app.ts"), "export const a = 3;\n").expect("edit");
        let notification = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let envelope = rx.recv().await.expect("envelope");
                let crate::outgoing_message::OutgoingEnvelope::Broadcast { message } = envelope
                else {
                    continue;
                };
                if let crate::outgoing_message::OutgoingMessage::AppServerNotification(
                    ServerNotification::WorkspaceChanged(changed),
                ) = message
                {
                    return changed;
                }
            }
        })
        .await
        .expect("timed out waiting for workspace/changed");
        assert_eq!(notification.invalidated_changesets, vec!["cs-fake"]);
        let seen = seen.lock().expect("seen lock");
        assert_eq!(seen.len(), 1, "sink invoked exactly once: {seen:?}");
        assert_eq!(seen[0].0, "p1");
        assert!(seen[0].1 > 0);
        assert!(!seen[0].2, "no overflow for a single-file batch");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn external_create_and_remove_classify_via_snapshot() {
        let dir = root_with_gitignore();
        let (outgoing, mut rx) = test_outgoing();
        let file_watcher = Arc::new(FileWatcher::new().expect("watcher"));
        let project_store = project_store_with_root(dir.path()).await;
        let manager = WorkspaceWatchManager::new(
            outgoing,
            file_watcher,
            project_store,
            Arc::new(|_, _, _| Vec::new()),
        );
        manager
            .watch(
                ConnectionId(1),
                WorkspaceWatchParams {
                    project_id: "p1".into(),
                },
            )
            .await
            .expect("watch");

        async fn next_changed(
            rx: &mut tokio::sync::mpsc::Receiver<
                crate::outgoing_message::OutgoingEnvelope,
            >,
        ) -> WorkspaceChangedNotification {
            loop {
                let envelope = rx.recv().await.expect("envelope");
                let crate::outgoing_message::OutgoingEnvelope::Broadcast { message } = envelope
                else {
                    continue;
                };
                if let crate::outgoing_message::OutgoingMessage::AppServerNotification(
                    ServerNotification::WorkspaceChanged(changed),
                ) = message
                {
                    return changed;
                }
            }
        }

        // Create: new file not in the initial snapshot -> Added.
        std::fs::write(dir.path().join("src/new.ts"), "new\n").expect("create");
        let created =
            tokio::time::timeout(Duration::from_secs(5), next_changed(&mut rx))
                .await
                .expect("timed out waiting for create event");
        let new_change = created
            .changes
            .iter()
            .find(|c| c.path == "src/new.ts")
            .expect("src/new.ts change");
        assert_eq!(new_change.kind, WorkspaceFileEventKind::Added);
        assert_eq!(
            new_change.content_hash.as_deref(),
            Some(crate::workspace_changeset::sha256_hex(b"new\n").as_str())
        );

        // Remove: witnessed file disappears -> Removed with no content hash.
        std::fs::remove_file(dir.path().join("src/new.ts")).expect("remove");
        let removed =
            tokio::time::timeout(Duration::from_secs(5), next_changed(&mut rx))
                .await
                .expect("timed out waiting for remove event");
        let remove_change = removed
            .changes
            .iter()
            .find(|c| c.path == "src/new.ts")
            .expect("src/new.ts removal");
        assert_eq!(remove_change.kind, WorkspaceFileEventKind::Removed);
        assert_eq!(remove_change.content_hash, None);
    }

    #[tokio::test]
    async fn watch_rejects_unknown_project_and_duplicates() {
        let dir = root_with_gitignore();
        let (outgoing, _rx) = test_outgoing();
        let file_watcher = Arc::new(FileWatcher::noop());
        let project_store = project_store_with_root(dir.path()).await;
        let manager = WorkspaceWatchManager::new(
            outgoing,
            file_watcher,
            project_store,
            Arc::new(|_, _, _| Vec::new()),
        );
        let err = manager
            .watch(
                ConnectionId(1),
                WorkspaceWatchParams {
                    project_id: "nope".into(),
                },
            )
            .await
            .expect_err("unknown project rejected");
        assert!(err.message.contains("unknown project"));
        manager
            .watch(
                ConnectionId(1),
                WorkspaceWatchParams {
                    project_id: "p1".into(),
                },
            )
            .await
            .expect("watch");
        let err = manager
            .watch(
                ConnectionId(1),
                WorkspaceWatchParams {
                    project_id: "p1".into(),
                },
            )
            .await
            .expect_err("duplicate watch rejected");
        assert!(err.message.contains("already watched"));
    }
}
