//! Versioned Workspace Service protocol: Ody-managed dev server lifecycle
//! for workspace projects (strategy E2).
//!
//! Discovery of dev scripts is owned by `workspace/project/scan` (E0); this
//! protocol starts, observes, and stops the long-running dev server processes
//! that back a real framework preview. Source authority stays in the user's
//! directories; this protocol never transports root file contents.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

pub const WORKSPACE_SERVICE_PROTOCOL_VERSION: u32 = 1;

/// Lifecycle status of a managed dev server. Terminal: Failed, Exited,
/// Stopped. Runtime restart normalizes lingering Starting/Ready to Stopped.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum WorkspaceServiceStatus {
    /// Spawned; readiness probe in flight.
    Starting,
    /// Readiness probe succeeded; HTTP server answering on `url`.
    Ready,
    /// Spawn error, readiness timeout (process group killed), or non-zero
    /// exit. `error`/`exitCode` carry the diagnosable reason.
    Failed,
    /// Process exited with code zero while managed.
    Exited,
    /// Stopped by user or normalized after a Runtime restart.
    Stopped,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceServiceHealth {
    pub ok: bool,
    #[ts(type = "number")]
    pub status_code: Option<u16>,
    pub error: Option<String>,
    #[ts(type = "number")]
    pub checked_at_ms: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceServiceRef {
    pub id: String,
    pub project_id: String,
    pub name: String,
    /// Index into the bound project's `roots`; cwd of the dev server.
    #[ts(type = "number")]
    pub root_index: u32,
    /// package.json script name chosen by the client from the E0 scan.
    pub script: String,
    /// Fully resolved command line actually spawned (`<pm> run <script> -- ...`).
    pub command: String,
    #[ts(type = "number")]
    pub port: u16,
    /// Loopback URL of the managed dev server.
    pub url: String,
    pub status: WorkspaceServiceStatus,
    #[ts(type = "number")]
    pub pid: Option<u32>,
    #[ts(type = "number")]
    pub exit_code: Option<i32>,
    /// Last on-demand health probe result (`list` only; never mutates status).
    pub health: Option<WorkspaceServiceHealth>,
    /// Diagnosable terminal reason (startup failure, restart normalization).
    pub error: Option<String>,
    #[ts(type = "number")]
    pub created_at_ms: i64,
    #[ts(type = "number")]
    pub updated_at_ms: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceServiceStartParams {
    pub project_id: String,
    /// Display name; unique among non-terminal services of the project.
    pub name: String,
    #[ts(type = "number")]
    pub root_index: u32,
    /// Explicit script name from the E0 scan; the Runtime never guesses.
    pub script: String,
    /// Requested port. Occupied -> structured `invalid_params`. Omitted ->
    /// framework default port probed, then a free port auto-selected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    #[ts(type = "number")]
    pub port: Option<u16>,
    /// Readiness deadline; default 60s, clamp [5s, 300s].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    #[ts(type = "number")]
    pub ready_timeout_ms: Option<i64>,
    /// Environment overrides merged into the app-server environment. Values
    /// are never echoed into logs or responses (strategy 8.2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub env: Option<HashMap<String, Option<String>>>,
    /// Client-generated key makes reconnect retries idempotent.
    pub idempotency_key: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceServiceStartResponse {
    pub service: WorkspaceServiceRef,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceServiceStopParams {
    pub service_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceServiceStopResponse {
    pub service: WorkspaceServiceRef,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceServiceListParams {
    pub project_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceServiceListResponse {
    pub services: Vec<WorkspaceServiceRef>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceServiceLogsParams {
    pub service_id: String,
    /// Per-stream tail cap in bytes; default 64 KiB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    #[ts(type = "number")]
    pub tail_bytes: Option<u64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceServiceLogsResponse {
    pub service_id: String,
    pub stdout_tail: String,
    pub stderr_tail: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspacePreviewCheckParams {
    /// Service whose URL is checked; must be Ready.
    pub service_id: String,
    /// Optional override URL (http/https only); defaults to `service.url`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub url: Option<String>,
    /// Request timeout; default 10s, clamp [1s, 30s].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    #[ts(type = "number")]
    pub timeout_ms: Option<i64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspacePreviewCheckResponse {
    pub service_id: String,
    pub url: String,
    pub reachable: bool,
    #[ts(type = "number")]
    pub http_status: Option<u16>,
    #[ts(type = "number")]
    pub content_bytes: u64,
    /// sha256 hex of the response body; HMR liveness evidence between checks.
    pub content_sha256: String,
    /// `<title>` extracted from HTML responses when present.
    pub title: Option<String>,
    pub error: Option<String>,
    #[ts(type = "number")]
    pub checked_at_ms: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ClientRequest;
    use crate::RequestId;

    #[test]
    fn workspace_service_start_has_stable_wire_name_and_is_experimental() {
        let request = ClientRequest::WorkspaceServiceStart {
            request_id: RequestId::Integer(21),
            params: WorkspaceServiceStartParams {
                project_id: "ws-1".to_owned(),
                name: "web".to_owned(),
                root_index: 0,
                script: "dev".to_owned(),
                port: Some(5173),
                ready_timeout_ms: None,
                env: None,
                idempotency_key: "svc-1".to_owned(),
            },
        };

        assert_eq!(request.method(), "workspace/service/start");
        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&request),
            Some("workspace/service/v1")
        );
        assert_eq!(
            serde_json::to_value(request).expect("serialize service start request"),
            serde_json::json!({
                "method": "workspace/service/start",
                "id": 21,
                "params": {
                    "projectId": "ws-1",
                    "name": "web",
                    "rootIndex": 0,
                    "script": "dev",
                    "port": 5173,
                    "idempotencyKey": "svc-1"
                }
            })
        );
    }

    #[test]
    fn workspace_service_stop_list_logs_share_service_experimental_reason() {
        for (request, method, reason) in [
            (
                ClientRequest::WorkspaceServiceStop {
                    request_id: RequestId::Integer(22),
                    params: WorkspaceServiceStopParams {
                        service_id: "svc-1".to_owned(),
                    },
                },
                "workspace/service/stop",
                "workspace/service/v1",
            ),
            (
                ClientRequest::WorkspaceServiceList {
                    request_id: RequestId::Integer(23),
                    params: WorkspaceServiceListParams {
                        project_id: "ws-1".to_owned(),
                    },
                },
                "workspace/service/list",
                "workspace/service/v1",
            ),
            (
                ClientRequest::WorkspaceServiceLogs {
                    request_id: RequestId::Integer(24),
                    params: WorkspaceServiceLogsParams {
                        service_id: "svc-1".to_owned(),
                        tail_bytes: None,
                    },
                },
                "workspace/service/logs",
                "workspace/service/v1",
            ),
            (
                ClientRequest::WorkspacePreviewCheck {
                    request_id: RequestId::Integer(25),
                    params: WorkspacePreviewCheckParams {
                        service_id: "svc-1".to_owned(),
                        url: Some("http://127.0.0.1:5173/about".to_owned()),
                        timeout_ms: None,
                    },
                },
                "workspace/preview/check",
                "workspace/preview/v1",
            ),
        ] {
            assert_eq!(request.method(), method);
            assert_eq!(
                crate::experimental_api::ExperimentalApi::experimental_reason(&request),
                Some(reason)
            );
        }
    }

    #[test]
    fn workspace_service_ref_serializes_full_lifecycle_shape() {
        let service = WorkspaceServiceRef {
            id: "svc-1".to_owned(),
            project_id: "ws-1".to_owned(),
            name: "web".to_owned(),
            root_index: 0,
            script: "dev".to_owned(),
            command: "npm run dev -- --port 5173 --strictPort".to_owned(),
            port: 5173,
            url: "http://127.0.0.1:5173/".to_owned(),
            status: WorkspaceServiceStatus::Ready,
            pid: Some(4242),
            exit_code: None,
            health: Some(WorkspaceServiceHealth {
                ok: true,
                status_code: Some(200),
                error: None,
                checked_at_ms: 1_700_000_000_000,
            }),
            error: None,
            created_at_ms: 1_700_000_000_000,
            updated_at_ms: 1_700_000_000_000,
        };

        let value = serde_json::to_value(service).expect("serialize service ref");
        assert_eq!(value["status"], "ready");
        assert_eq!(value["port"], 5173);
        assert_eq!(value["rootIndex"], 0);
        assert_eq!(value["health"]["statusCode"], 200);
        assert_eq!(value["url"], "http://127.0.0.1:5173/");
        assert!(value.get("exitCode").is_none() || value["exitCode"].is_null());
    }
}
