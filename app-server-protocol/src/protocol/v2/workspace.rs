//! Versioned Workspace Project protocol: durable runtime authority for bindings
//! between Ody and user-authorized filesystem roots.
//!
//! Source authority for a workspace project always lives in the user's own
//! directories; this protocol binds and validates roots, it never transports
//! root file contents. E0 covers the project lifecycle; read-only discovery
//! (`workspace/project/scan`) lands with the discovery engine.

use ody_utils_absolute_path::AbsolutePathBuf;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

pub const WORKSPACE_PROJECT_PROTOCOL_VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum WorkspaceRootRole {
    Primary,
    Secondary,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceRoot {
    /// Canonicalized absolute path on the local filesystem.
    pub path: String,
    /// Derived from bind order: `roots[0]` is the primary (default cwd) root.
    pub role: WorkspaceRootRole,
    /// Authorization provenance. E0 only supports `user_selected`.
    pub auth_source: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceProjectRef {
    pub id: String,
    pub name: String,
    pub schema_version: u32,
    pub roots: Vec<WorkspaceRoot>,
    #[ts(type = "number")]
    pub created_at_ms: i64,
    #[ts(type = "number")]
    pub updated_at_ms: i64,
    /// E4: connection id (as integer) currently holding the write lock, if
    /// any. Runtime-only: never persisted; `get`/`list` fill it from the
    /// in-memory lock table on every response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    #[ts(type = "number")]
    pub locked_by: Option<u64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceProjectBindParams {
    pub id: String,
    pub name: String,
    /// Absolute, existing directories chosen by the user. The first entry
    /// becomes the primary root; nesting between entries is rejected.
    pub roots: Vec<AbsolutePathBuf>,
    /// Client-generated key makes reconnect retries idempotent.
    pub idempotency_key: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceProjectBindResponse {
    pub project: WorkspaceProjectRef,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceProjectGetParams {
    pub project_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceProjectGetResponse {
    pub project: WorkspaceProjectRef,
    /// Connection id of the caller, so a client can tell whether a
    /// `locked_by` holder is itself (same connection) or another session.
    /// Runtime-only: filled per request, never persisted.
    #[ts(type = "number")]
    pub caller_connection_id: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceProjectListParams {}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceProjectListResponse {
    pub projects: Vec<WorkspaceProjectRef>,
    /// See `WorkspaceProjectGetResponse::caller_connection_id`.
    #[ts(type = "number")]
    pub caller_connection_id: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceProjectCloseParams {
    pub project_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceProjectCloseResponse {
    /// The final binding record that was removed. Source data is untouched.
    pub project: WorkspaceProjectRef,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum WorkspaceSourceKind {
    Page,
    Component,
    Route,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceTechEntry {
    /// Framework or tool signal id, e.g. "react", "vite", "next", "vue",
    /// "svelte", "angular", "express".
    pub id: String,
    /// Raw dependency declaration version when discovered from package.json.
    pub version: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceScript {
    pub name: String,
    pub command: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceSourceEntry {
    pub kind: WorkspaceSourceKind,
    /// File stem (no extension).
    pub name: String,
    /// Path relative to the scanned root, `/`-separated.
    pub path: String,
    /// Route path when `kind` is `Route`, derived from directory conventions.
    pub route_path: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceGitStatus {
    /// True when a `.git` entry is found at or above the root (pure fs probe,
    /// no git binary required).
    pub is_repo: bool,
    pub repo_root: Option<String>,
    pub branch: Option<String>,
    pub head_commit_hash: Option<String>,
    pub has_changes: Option<bool>,
    /// False when the git binary is unavailable, errors, or times out; `error`
    /// carries the diagnosable reason and the rest of the scan is unaffected.
    pub available: bool,
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceScanStats {
    #[ts(type = "number")]
    pub files_visited: u64,
    #[ts(type = "number")]
    pub dirs_visited: u64,
    #[ts(type = "number")]
    pub skipped_dirs: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceRootDiscovery {
    /// Canonicalized root path from the binding record.
    pub root_path: String,
    pub package_name: Option<String>,
    /// "pnpm" | "yarn" | "npm" | "bun" from lockfile priority.
    pub package_manager: Option<String>,
    pub tech_stack: Vec<WorkspaceTechEntry>,
    pub scripts: Vec<WorkspaceScript>,
    pub sources: Vec<WorkspaceSourceEntry>,
    pub git: WorkspaceGitStatus,
    pub stats: WorkspaceScanStats,
    /// Per-root diagnosable problems; remaining roots still scan.
    pub errors: Vec<String>,
    pub truncated: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceDiscovery {
    pub project_id: String,
    pub roots: Vec<WorkspaceRootDiscovery>,
    pub truncated: bool,
    #[ts(type = "number")]
    pub scanned_at_ms: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceProjectScanParams {
    pub project_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceProjectScanResponse {
    pub discovery: WorkspaceDiscovery,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ClientRequest;
    use crate::RequestId;

    #[test]
    fn workspace_project_bind_has_stable_wire_name_and_is_experimental() {
        let request = ClientRequest::WorkspaceProjectBind {
            request_id: RequestId::Integer(11),
            params: WorkspaceProjectBindParams {
                id: "ws-1".to_owned(),
                name: "storefront".to_owned(),
                roots: vec![ody_utils_absolute_path::test_support::PathBufExt::abs(
                    &std::path::PathBuf::from("/tmp/storefront"),
                )],
                idempotency_key: "bind-1".to_owned(),
            },
        };

        assert_eq!(request.method(), "workspace/project/bind");
        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&request),
            Some("workspace/project/v1")
        );
        assert_eq!(
            serde_json::to_value(request).expect("serialize workspace project request"),
            serde_json::json!({
                "method": "workspace/project/bind",
                "id": 11,
                "params": {
                    "id": "ws-1",
                    "name": "storefront",
                    "roots": ["/tmp/storefront"],
                    "idempotencyKey": "bind-1"
                }
            })
        );
    }

    #[test]
    fn workspace_project_scan_has_stable_wire_name_and_is_experimental() {
        let request = ClientRequest::WorkspaceProjectScan {
            request_id: RequestId::Integer(12),
            params: WorkspaceProjectScanParams {
                project_id: "ws-1".to_owned(),
            },
        };

        assert_eq!(request.method(), "workspace/project/scan");
        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&request),
            Some("workspace/project/v1")
        );
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceProjectLockParams {
    pub project_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceProjectLockResponse {
    pub project_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceProjectUnlockParams {
    pub project_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceProjectUnlockResponse {
    pub project_id: String,
}
