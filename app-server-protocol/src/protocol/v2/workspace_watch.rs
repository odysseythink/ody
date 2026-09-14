//! E4 workspace watcher protocol: project-level recursive watches with
//! .gitignore-aware filtering, content-hash snapshots, and broadcast
//! `workspace/changed` notifications.

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

pub const WORKSPACE_WATCH_PROTOCOL_VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceWatchParams {
    pub project_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceWatchResponse {
    pub project_id: String,
    /// Root indexes that were registered for watching.
    pub watched_roots: Vec<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceUnwatchParams {
    pub project_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceUnwatchResponse {
    pub project_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum WorkspaceFileEventKind {
    Added,
    Modified,
    Removed,
}

/// One debounced external file change under a project root.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceFileEvent {
    #[ts(type = "number")]
    pub root_index: u32,
    /// Root-relative `/`-joined path.
    pub path: String,
    pub kind: WorkspaceFileEventKind,
    /// sha256 of the file content after the change; None for removals or
    /// unreadable/racing reads (best-effort, never blocks notification).
    pub content_hash: Option<String>,
}

/// Broadcast to all connections; one per debounce window per project.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceChangedNotification {
    pub project_id: String,
    /// Capped at MAX_WATCH_EVENTS_PER_NOTIFICATION; see `overflow`.
    pub changes: Vec<WorkspaceFileEvent>,
    /// True when the debounce window held more events than the cap.
    pub overflow: bool,
    /// Pending changesets whose base hashes were contradicted by this
    /// event batch; clients should prompt re-index/re-create.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub invalidated_changesets: Vec<String>,
}
