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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ClientRequest;
    use crate::RequestId;

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
}
