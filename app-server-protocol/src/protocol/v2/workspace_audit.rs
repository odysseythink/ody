use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

pub const WORKSPACE_AUDIT_PROTOCOL_VERSION: u32 = 1;

/// One append-only audit record. `detail` is a whitelist-built summary:
/// paths, names, counts, ids, hashes — never file content, never env/secret
/// values (strategy 8.2 key discipline).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceAuditEvent {
    #[ts(type = "number")]
    pub seq: u64,
    #[ts(type = "number")]
    pub timestamp_ms: i64,
    pub project_id: Option<String>,
    /// Dotted operation id, e.g. "changeset.apply", "service.startAll".
    pub operation: String,
    /// "ok" | "error".
    pub outcome: String,
    /// Whitelisted structured summary (object), e.g. {"changesetId":"cs-1",
    /// "files":["0:src/a.ts"]} or {"error":"baseHash mismatch"}.
    pub detail: serde_json::Value,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceAuditListParams {
    pub project_id: Option<String>,
    /// Newest-first cap. Default 100, clamped to [1, 500].
    #[serde(default)]
    #[ts(optional)]
    #[ts(type = "number")]
    pub limit: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceAuditListResponse {
    /// Newest first.
    pub events: Vec<WorkspaceAuditEvent>,
}
