//! Versioned Workspace Source protocol: SourceRef indexing, resolution,
//! changesets, diff, and validation over user-authorized workspace roots.
//!
//! Source authority always lives in the user's directories. E1 lands this
//! protocol in steps: index/resolve first; changesets, diff, and validation
//! follow with their engines.

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

use super::workspace::WorkspaceSourceKind;

pub const WORKSPACE_SOURCE_PROTOCOL_VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceSourceRange {
    /// 1-based inclusive start line of the symbol block.
    #[ts(type = "number")]
    pub start_line: u32,
    /// 1-based inclusive end line of the symbol block.
    #[ts(type = "number")]
    pub end_line: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceSourceArtifact {
    /// Stable id: "{kind}:{root_index}:{path}".
    pub id: String,
    pub kind: WorkspaceSourceKind,
    pub name: String,
    /// Path relative to the owning root, `/`-separated.
    pub path: String,
    pub route_path: Option<String>,
    /// Framework signal id from discovery, e.g. "react", "vue", "next".
    pub framework: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceSourceRef {
    pub artifact_id: String,
    /// Canonicalized owning root path from the binding record.
    pub root_path: String,
    /// Path relative to the owning root, `/`-separated.
    pub file_path: String,
    /// 1-based inclusive line range of the symbol block; None = whole file.
    pub range: Option<WorkspaceSourceRange>,
    /// Export symbol name; file stem when no export is found.
    pub symbol: String,
    pub route_path: Option<String>,
    /// sha256 hex of the ranged content (whole file when range is None).
    pub content_hash: String,
    /// sha256 hex of the whole file.
    pub file_hash: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceSourceIndex {
    pub project_id: String,
    /// Artifacts and refs are parallel 1:1 vectors.
    pub artifacts: Vec<WorkspaceSourceArtifact>,
    pub refs: Vec<WorkspaceSourceRef>,
    #[ts(type = "number")]
    pub indexed_at_ms: i64,
    pub truncated: bool,
    /// Per-root diagnosable problems; remaining roots still index.
    pub errors: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceSourceIndexParams {
    pub project_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceSourceIndexResponse {
    pub index: WorkspaceSourceIndex,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum WorkspaceSourceQueryKind {
    Name,
    Symbol,
    RoutePath,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceSourceResolveParams {
    pub project_id: String,
    pub kind: WorkspaceSourceQueryKind,
    pub value: String,
    /// Default 20, capped at 100.
    #[ts(optional = nullable)]
    pub limit: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceSourceResolveMatch {
    pub artifact: WorkspaceSourceArtifact,
    pub source_ref: WorkspaceSourceRef,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceSourceResolveResponse {
    pub matches: Vec<WorkspaceSourceResolveMatch>,
}


#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum WorkspaceFileChangeKind {
    Add,
    Update,
    Delete,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceFileChange {
    /// Index into the project binding's `roots` vector.
    #[ts(type = "number")]
    pub root_index: u32,
    /// Root-relative `/`-separated path; normalized and escape-checked at
    /// create. The resolved absolute target is stored server-side.
    pub path: String,
    pub kind: WorkspaceFileChangeKind,
    /// sha256 hex the change expects on disk. Required for Update/Delete.
    pub base_hash: Option<String>,
    /// Full new file content. Required for Add/Update.
    pub content: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum WorkspaceChangeSetStatus {
    Pending,
    Applied,
    Rejected,
    Restored,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum WorkspaceChangeSetCheckpoint {
    /// A usable git metadata set covers the root: an existing user
    /// repository (never modified) or a gix baseline initialized for a
    /// previously non-git root on first apply.
    Git {
        /// HEAD commit hash at apply time; None when unreadable.
        head_commit_hash: Option<String>,
    },
    /// No checkpoint could be established.
    None,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceChangeSet {
    /// Server-generated `cs-{uuid}`.
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub schema_version: u32,
    pub changes: Vec<WorkspaceFileChange>,
    pub status: WorkspaceChangeSetStatus,
    pub checkpoint: WorkspaceChangeSetCheckpoint,
    /// Unified diff (base -> proposed) computed at create time.
    pub unified_diff: String,
    #[ts(type = "number")]
    pub created_at_ms: i64,
    #[ts(type = "number")]
    pub updated_at_ms: i64,
    #[ts(type = "number")]
    pub applied_at_ms: Option<i64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceChangeSetCreateParams {
    pub project_id: String,
    pub title: String,
    pub changes: Vec<WorkspaceFileChange>,
    /// Client-generated key makes create retries idempotent.
    pub idempotency_key: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceChangeSetCreateResponse {
    pub changeset: WorkspaceChangeSet,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceChangeSetGetParams {
    pub changeset_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceChangeSetGetResponse {
    pub changeset: WorkspaceChangeSet,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceChangeSetListParams {
    pub project_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceChangeSetListResponse {
    /// Most recently created first.
    pub changesets: Vec<WorkspaceChangeSet>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceChangeSetApplyParams {
    pub changeset_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceChangeSetApplyResponse {
    pub changeset: WorkspaceChangeSet,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceChangeSetRejectParams {
    pub changeset_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceChangeSetRejectResponse {
    pub changeset: WorkspaceChangeSet,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceChangeSetRestoreParams {
    pub changeset_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceChangeSetRestoreResponse {
    pub changeset: WorkspaceChangeSet,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceChangeSetDiffEntry {
    pub id: String,
    pub status: WorkspaceChangeSetStatus,
    pub unified_diff: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceGitDiff {
    pub repo_root: Option<String>,
    /// False when the git binary is unavailable, errors, or times out.
    pub available: bool,
    pub error: Option<String>,
    /// `git diff HEAD` output. Untracked files are not included (v1
    /// limitation; per-changeset diffs cover them).
    pub unified_diff: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceSourceDiffParams {
    pub project_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceSourceDiffResponse {
    /// Pending, Applied, and Restored changesets with their unified diffs.
    pub changesets: Vec<WorkspaceChangeSetDiffEntry>,
    /// Whole-repo diff when the primary root is inside a git repo.
    pub git_diff: Option<WorkspaceGitDiff>,
}


#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum WorkspaceValidationKind {
    Format,
    Typecheck,
    Build,
    Test,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceValidationCheck {
    pub kind: WorkspaceValidationKind,
    /// Script name from the root's package.json (E0 scan `scripts`).
    /// The runtime does not guess script mappings (narrow protocol).
    pub script: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum WorkspaceValidationStatus {
    Succeeded,
    Failed,
    TimedOut,
    SpawnError,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceValidationRun {
    pub kind: WorkspaceValidationKind,
    pub script: String,
    pub status: WorkspaceValidationStatus,
    pub exit_code: Option<i32>,
    /// Trailing stdout bytes, capped at 64 KiB.
    pub stdout_tail: String,
    /// Trailing stderr bytes, capped at 64 KiB.
    pub stderr_tail: String,
    #[ts(type = "number")]
    pub started_at_ms: i64,
    #[ts(type = "number")]
    pub duration_ms: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum WorkspaceValidationOverall {
    Succeeded,
    Failed,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceValidationReport {
    pub runs: Vec<WorkspaceValidationRun>,
    pub overall: WorkspaceValidationOverall,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceSourceValidateParams {
    pub project_id: String,
    /// Optional changeset association; echoed in the response only. The
    /// changeset must exist and belong to the project.
    pub changeset_id: Option<String>,
    pub checks: Vec<WorkspaceValidationCheck>,
    /// Per-check timeout; defaults to 120s, clamped to [1s, 600s].
    #[ts(optional = nullable)]
    pub timeout_ms: Option<i64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceSourceValidateResponse {
    pub project_id: String,
    pub changeset_id: Option<String>,
    pub report: WorkspaceValidationReport,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ClientRequest;
    use crate::RequestId;

    #[test]
    fn workspace_source_validate_has_stable_wire_name_and_is_experimental() {
        let request = ClientRequest::WorkspaceSourceValidate {
            request_id: RequestId::Integer(40),
            params: WorkspaceSourceValidateParams {
                project_id: "ws-1".to_owned(),
                changeset_id: Some("cs-1".to_owned()),
                checks: vec![
                    WorkspaceValidationCheck {
                        kind: WorkspaceValidationKind::Build,
                        script: "build".to_owned(),
                    },
                    WorkspaceValidationCheck {
                        kind: WorkspaceValidationKind::Typecheck,
                        script: "typecheck".to_owned(),
                    },
                ],
                timeout_ms: Some(60_000),
            },
        };

        assert_eq!(request.method(), "workspace/source/validate");
        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&request),
            Some("workspace/source/v1")
        );
        let value = serde_json::to_value(request).expect("serialize validate");
        assert_eq!(value["params"]["checks"][0]["kind"], "build");
        assert_eq!(value["params"]["checks"][1]["script"], "typecheck");
        assert_eq!(value["params"]["timeoutMs"], 60_000);
    }

    #[test]
    fn workspace_source_index_has_stable_wire_name_and_is_experimental() {
        let request = ClientRequest::WorkspaceSourceIndex {
            request_id: RequestId::Integer(21),
            params: WorkspaceSourceIndexParams {
                project_id: "ws-1".to_owned(),
            },
        };

        assert_eq!(request.method(), "workspace/source/index");
        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&request),
            Some("workspace/source/v1")
        );
        assert_eq!(
            serde_json::to_value(request).expect("serialize source index request"),
            serde_json::json!({
                "method": "workspace/source/index",
                "id": 21,
                "params": { "projectId": "ws-1" }
            })
        );
    }

    #[test]
    fn workspace_source_resolve_has_stable_wire_name_and_is_experimental() {
        let request = ClientRequest::WorkspaceSourceResolve {
            request_id: RequestId::Integer(22),
            params: WorkspaceSourceResolveParams {
                project_id: "ws-1".to_owned(),
                kind: WorkspaceSourceQueryKind::Symbol,
                value: "HomePage".to_owned(),
                limit: Some(5),
            },
        };

        assert_eq!(request.method(), "workspace/source/resolve");
        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&request),
            Some("workspace/source/v1")
        );
        assert_eq!(
            serde_json::to_value(request).expect("serialize source resolve request"),
            serde_json::json!({
                "method": "workspace/source/resolve",
                "id": 22,
                "params": {
                    "projectId": "ws-1",
                    "kind": "symbol",
                    "value": "HomePage",
                    "limit": 5
                }
            })
        );
    }

    #[test]
    fn workspace_source_changeset_create_has_stable_wire_name_and_is_experimental() {
        let request = ClientRequest::WorkspaceChangeSetCreate {
            request_id: RequestId::Integer(30),
            params: WorkspaceChangeSetCreateParams {
                project_id: "ws-1".to_owned(),
                title: "update homepage".to_owned(),
                changes: vec![WorkspaceFileChange {
                    root_index: 0,
                    path: "src/pages/HomePage.tsx".to_owned(),
                    kind: WorkspaceFileChangeKind::Update,
                    base_hash: Some("aa".repeat(32)),
                    content: Some("export default function HomePage() {}".to_owned()),
                }],
                idempotency_key: "cs-key-1".to_owned(),
            },
        };

        assert_eq!(request.method(), "workspace/source/changeset/create");
        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&request),
            Some("workspace/source/v1")
        );
        let value = serde_json::to_value(request).expect("serialize changeset create");
        assert_eq!(value["method"], "workspace/source/changeset/create");
        assert_eq!(value["params"]["title"], "update homepage");
        assert_eq!(value["params"]["changes"][0]["kind"], "update");
        assert_eq!(value["params"]["changes"][0]["rootIndex"], 0);
        assert_eq!(value["params"]["idempotencyKey"], "cs-key-1");
    }

    #[test]
    fn workspace_source_changeset_lifecycle_methods_have_stable_wire_names() {
        let cases: Vec<(ClientRequest, &str)> = vec![
            (
                ClientRequest::WorkspaceChangeSetGet {
                    request_id: RequestId::Integer(31),
                    params: WorkspaceChangeSetGetParams {
                        changeset_id: "cs-1".to_owned(),
                    },
                },
                "workspace/source/changeset/get",
            ),
            (
                ClientRequest::WorkspaceChangeSetList {
                    request_id: RequestId::Integer(32),
                    params: WorkspaceChangeSetListParams {
                        project_id: "ws-1".to_owned(),
                    },
                },
                "workspace/source/changeset/list",
            ),
            (
                ClientRequest::WorkspaceChangeSetApply {
                    request_id: RequestId::Integer(33),
                    params: WorkspaceChangeSetApplyParams {
                        changeset_id: "cs-1".to_owned(),
                    },
                },
                "workspace/source/changeset/apply",
            ),
            (
                ClientRequest::WorkspaceChangeSetReject {
                    request_id: RequestId::Integer(34),
                    params: WorkspaceChangeSetRejectParams {
                        changeset_id: "cs-1".to_owned(),
                    },
                },
                "workspace/source/changeset/reject",
            ),
            (
                ClientRequest::WorkspaceChangeSetRestore {
                    request_id: RequestId::Integer(35),
                    params: WorkspaceChangeSetRestoreParams {
                        changeset_id: "cs-1".to_owned(),
                    },
                },
                "workspace/source/changeset/restore",
            ),
            (
                ClientRequest::WorkspaceSourceDiff {
                    request_id: RequestId::Integer(36),
                    params: WorkspaceSourceDiffParams {
                        project_id: "ws-1".to_owned(),
                    },
                },
                "workspace/source/diff",
            ),
        ];
        for (request, expected) in cases {
            assert_eq!(request.method(), expected);
            assert_eq!(
                crate::experimental_api::ExperimentalApi::experimental_reason(&request),
                Some("workspace/source/v1")
            );
        }
    }
}
