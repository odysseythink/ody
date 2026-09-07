//! Versioned Visual Workspace protocol shared by graphical clients and Ody Runtime.
//!
//! The protocol intentionally transports self-contained artifact source instead of
//! framework-specific component trees. Framework/AST source mapping remains a later
//! capability; v1 establishes one durable runtime authority for visual revisions.

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

pub const VISUAL_WORKSPACE_PROTOCOL_VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct VisualProject {
    pub id: String,
    pub name: String,
    pub schema_version: u32,
    #[ts(type = "number")]
    pub created_at_ms: i64,
    #[ts(type = "number")]
    pub updated_at_ms: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct VisualProjectUpsertParams {
    pub id: String,
    pub name: String,
    /// Client-generated key makes reconnect retries idempotent.
    pub idempotency_key: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct VisualProjectUpsertResponse {
    pub project: VisualProject,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct VisualProjectListParams {}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct VisualProjectListResponse {
    pub projects: Vec<VisualProject>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum VisualRevisionReason {
    Generate,
    Iterate,
    Restore,
    Instrument,
    StylePatch,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct VisualStyleDeclaration {
    pub property: String,
    pub value: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(tag = "type", export_to = "v2/")]
pub enum VisualPatchOperation {
    SetStyle {
        element_id: String,
        declarations: Vec<VisualStyleDeclaration>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct VisualPatchSet {
    pub id: String,
    pub base_artifact_id: String,
    pub result_artifact_id: String,
    pub operations: Vec<VisualPatchOperation>,
    #[ts(type = "number")]
    pub created_at_ms: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct VisualArtifact {
    pub id: String,
    pub project_id: String,
    pub lineage_id: String,
    pub revision: u32,
    pub parent_artifact_id: Option<String>,
    pub name: String,
    /// Complete standalone source. The runtime applies deterministic patches to this value.
    pub html: String,
    pub revision_reason: VisualRevisionReason,
    pub patch_set: Option<VisualPatchSet>,
    #[ts(type = "number")]
    pub created_at_ms: i64,
    #[ts(type = "number")]
    pub updated_at_ms: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct VisualArtifactCreateParams {
    pub id: String,
    pub project_id: String,
    pub lineage_id: Option<String>,
    pub parent_artifact_id: Option<String>,
    pub name: String,
    pub html: String,
    pub revision_reason: VisualRevisionReason,
    pub idempotency_key: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct VisualArtifactCreateResponse {
    pub artifact: VisualArtifact,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct VisualArtifactListParams {
    pub project_id: String,
    pub lineage_id: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct VisualArtifactListResponse {
    pub artifacts: Vec<VisualArtifact>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct VisualPatchApplyParams {
    pub project_id: String,
    pub patch_set: VisualPatchSet,
    pub idempotency_key: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct VisualPatchApplyResponse {
    pub artifact: VisualArtifact,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum VisualViewport {
    Desktop,
    Tablet,
    Mobile,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct VisualSnapshot {
    pub id: String,
    pub project_id: String,
    pub artifact_id: String,
    pub viewport: VisualViewport,
    pub width: u32,
    pub height: u32,
    /// PNG bytes encoded by the capture-capable client/browser.
    pub data_base64: String,
    #[ts(type = "number")]
    pub created_at_ms: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct VisualSnapshotCreateParams {
    pub id: String,
    pub project_id: String,
    pub artifact_id: String,
    pub viewport: VisualViewport,
    pub width: u32,
    pub height: u32,
    pub data_base64: String,
    pub idempotency_key: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct VisualSnapshotCreateResponse {
    pub snapshot: VisualSnapshot,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct VisualPreviewOpenParams {
    pub project_id: String,
    pub artifact_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct VisualPreviewOpenResponse {
    pub preview_session_id: String,
    pub artifact: VisualArtifact,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct VisualWorkspaceChangedNotification {
    pub project_id: String,
    pub artifact_id: Option<String>,
    pub kind: String,
    #[ts(type = "number")]
    pub updated_at_ms: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ClientRequest;
    use crate::RequestId;

    #[test]
    fn visual_workspace_requests_have_stable_wire_names_and_are_experimental() {
        let request = ClientRequest::VisualProjectUpsert {
            request_id: RequestId::Integer(7),
            params: VisualProjectUpsertParams {
                id: "project-1".to_owned(),
                name: "Landing".to_owned(),
                idempotency_key: "retry-1".to_owned(),
            },
        };

        assert_eq!(request.method(), "visualWorkspace/project/upsert");
        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&request),
            Some("visualWorkspace/v1")
        );
        assert_eq!(
            serde_json::to_value(request).expect("serialize visual request"),
            serde_json::json!({
                "method": "visualWorkspace/project/upsert",
                "id": 7,
                "params": {
                    "id": "project-1",
                    "name": "Landing",
                    "idempotencyKey": "retry-1"
                }
            })
        );
    }
}
