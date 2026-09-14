//! Versioned Workspace Service protocol: Ody-managed dev server lifecycle
//! for workspace projects (strategy E2).
//!
//! Discovery of dev scripts is owned by `workspace/project/scan` (E0); this
//! protocol starts, observes, and stops the long-running dev server processes
//! that back a real framework preview. Source authority stays in the user's
//! directories; this protocol never transports root file contents.

use std::collections::BTreeMap;
use std::collections::HashMap;

use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

use super::workspace_source::WorkspaceSourceRef;

pub const WORKSPACE_SERVICE_PROTOCOL_VERSION: u32 = 3;

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
    /// E4: resolved values for the matching spec's `envRefs` (or ad-hoc
    /// names when no spec exists). Request-scoped only: never persisted,
    /// never logged, never included in audit detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub secret_values: Option<BTreeMap<String, String>>,
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

/// Health check definition attached to a service spec (E3): probed on the
/// service's loopback origin and used as the orchestration gate between
/// dependency levels.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceServiceHealthCheck {
    /// HTTP path probed on the service's loopback origin, e.g. "/api/health".
    pub path: String,
    /// Exact expected status; default 200. Stricter than the `list`
    /// probe's 2xx tolerance — a degraded 500 must fail the gate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    #[ts(type = "number")]
    pub expect_status: Option<u16>,
    /// Per-attempt timeout; default 2000 ms, clamped to [200, 10000].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    #[ts(type = "number")]
    pub timeout_ms: Option<i64>,
}

/// Declarative, project-scoped service definition (E3). Single-root
/// monorepo sub-packages use `cwd`; dual-root projects use per-spec
/// `rootIndex`. Source authority stays in the user's directories.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceServiceSpec {
    /// Server-generated `spec-{uuid}`.
    pub id: String,
    pub project_id: String,
    /// Unique among the project's specs; 1-64 chars.
    pub name: String,
    /// Index into the bound project's `roots`.
    #[ts(type = "number")]
    pub root_index: u32,
    /// package.json script name from the E0 scan; the Runtime never guesses.
    pub script: String,
    /// Requested port; occupied -> structured error at start time. Omitted
    /// -> framework default probed, then a free port auto-selected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    #[ts(type = "number")]
    pub port: Option<u16>,
    /// Root-relative working directory for the service process. Normalized
    /// and escape-checked at define time; the resolved cwd never leaves
    /// the bound root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub cwd: Option<String>,
    /// Names of other specs in the same project that must be Ready (and
    /// healthy when they define a health check) before this one starts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub health_check: Option<WorkspaceServiceHealthCheck>,
    /// Readiness deadline per service; default 60s, clamp [5s, 300s].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    #[ts(type = "number")]
    pub ready_timeout_ms: Option<i64>,
    /// E4: names of environment variables the service expects at runtime
    /// (e.g. ["STRIPE_KEY"]). Only reference NAMES are persisted here —
    /// values are supplied per-request via `secretValues` and never stored.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub env_refs: Vec<String>,
    #[ts(type = "number")]
    pub created_at_ms: i64,
    #[ts(type = "number")]
    pub updated_at_ms: i64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceServiceDefineParams {
    pub project_id: String,
    pub name: String,
    #[ts(type = "number")]
    pub root_index: u32,
    /// Explicit script name from the E0 scan; the Runtime never guesses.
    pub script: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    #[ts(type = "number")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub health_check: Option<WorkspaceServiceHealthCheck>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    #[ts(type = "number")]
    pub ready_timeout_ms: Option<i64>,
    /// Optional sensitive env var names the service needs; validated
    /// (upper-snake, ≤64 chars, ≤16 entries, no duplicates).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub env_refs: Option<Vec<String>>,
    /// Client-generated key makes define retries idempotent.
    pub idempotency_key: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceServiceDefineResponse {
    pub spec: WorkspaceServiceSpec,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceServiceSpecsParams {
    pub project_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceServiceSpecsResponse {
    pub specs: Vec<WorkspaceServiceSpec>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceServiceStartAllParams {
    pub project_id: String,
    /// Subset of spec names; empty = all specs of the project. Unknown
    /// names are a request-level `invalid_params` error.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<String>,
    /// E4: resolved values for every selected spec's `envRefs`; one map is
    /// shared across the orchestration. Request-scoped only: never
    /// persisted, never logged, never included in audit detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub secret_values: Option<BTreeMap<String, String>>,
    /// Client-generated key: a recorded orchestration outcome is replayed
    /// verbatim, so client retries never respawn processes.
    pub idempotency_key: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceServiceStartAllResponse {
    /// Final state of every service touched by this orchestration.
    pub services: Vec<WorkspaceServiceRef>,
    /// Service ids newly started by this call.
    pub started: Vec<String>,
    /// Already-active same-name services reused without respawn.
    pub reused: Vec<String>,
    /// Service ids that failed; dependency levels after the failure were
    /// not started. Already-started services keep running (8.2).
    pub failed: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceServiceStopAllParams {
    pub project_id: String,
    /// Subset of spec names; empty = all specs of the project.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceServiceStopAllResponse {
    pub stopped: Vec<WorkspaceServiceRef>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceServiceHealthParams {
    pub service_id: String,
    /// Probe path override; default spec.healthCheck.path or "/".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub path: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceServiceHealthResponse {
    pub service_id: String,
    pub health: WorkspaceServiceHealth,
    /// The path actually probed (params override > spec health check > "/").
    pub probed_path: String,
    /// Exact status expectation used, when defined.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    #[ts(type = "number")]
    pub expected_status: Option<u16>,
}

/// A network failure observed by the caller's browser (renderer preview
/// surface or agent tool result). E3 diagnose correlates these against
/// managed services, their logs, and the workspace source index; the
/// Runtime never opens a browser itself (narrow-protocol decision).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceObservedNetworkFailure {
    /// Full request URL as observed.
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    #[ts(type = "number")]
    pub status: Option<u16>,
    /// Browser-level error text (e.g. "net::ERR_CONNECTION_REFUSED").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    #[ts(type = "number")]
    pub occurred_at_ms: Option<i64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceNetworkDiagnosis {
    /// The failure this diagnosis answers (echoed).
    pub failure: WorkspaceObservedNetworkFailure,
    /// Project service matched by loopback port: preferred non-terminal,
    /// else the most recent terminal record — a crashed backend is
    /// exactly what diagnosis must surface.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub matched_service: Option<WorkspaceServiceRef>,
    /// On-demand health probe of the matched service.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub health: Option<WorkspaceServiceHealth>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub stdout_tail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub stderr_tail: Option<String>,
    /// Log lines mentioning any path segment of the failure URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    pub log_excerpt: Option<String>,
    /// Source candidates: route entries matching the path (and its
    /// prefix-stripped variants) plus file-name matches on the last
    /// path segment, from the on-demand workspace source index.
    pub source_candidates: Vec<WorkspaceSourceRef>,
    /// Human-readable correlation notes (non-loopback URL, no service
    /// match, index truncation, ...).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspacePreviewDiagnoseParams {
    pub project_id: String,
    /// 1..=16 failures per call.
    pub failures: Vec<WorkspaceObservedNetworkFailure>,
    /// Per-stream tail cap; default 16 KiB, clamped to [1 KiB, 64 KiB].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    #[ts(type = "number")]
    pub log_tail_bytes: Option<u64>,
    /// Source candidate cap; default 10, clamped to [1, 50].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional = nullable)]
    #[ts(type = "number")]
    pub max_candidates: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspacePreviewDiagnoseResponse {
    pub project_id: String,
    pub results: Vec<WorkspaceNetworkDiagnosis>,
    #[ts(type = "number")]
    pub diagnosed_at_ms: i64,
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
                secret_values: None,
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

    #[test]
    fn workspace_service_define_has_stable_wire_name_and_is_experimental() {
        let request = ClientRequest::WorkspaceServiceDefine {
            request_id: RequestId::Integer(31),
            params: WorkspaceServiceDefineParams {
                project_id: "ws-1".to_owned(),
                name: "backend".to_owned(),
                root_index: 1,
                script: "dev".to_owned(),
                port: None,
                cwd: Some("backend".to_owned()),
                depends_on: vec![],
                health_check: Some(WorkspaceServiceHealthCheck {
                    path: "/api/health".to_owned(),
                    expect_status: Some(200),
                    timeout_ms: None,
                }),
                ready_timeout_ms: None,
                env_refs: None,
                idempotency_key: "spec-1".to_owned(),
            },
        };

        assert_eq!(request.method(), "workspace/service/define");
        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&request),
            Some("workspace/service/v1")
        );
        assert_eq!(
            serde_json::to_value(&request).expect("serialize define request"),
            serde_json::json!({
                "method": "workspace/service/define",
                "id": 31,
                "params": {
                    "projectId": "ws-1",
                    "name": "backend",
                    "rootIndex": 1,
                    "script": "dev",
                    "cwd": "backend",
                    "healthCheck": { "path": "/api/health", "expectStatus": 200 },
                    "idempotencyKey": "spec-1"
                }
            })
        );
    }

    #[test]
    fn workspace_service_orchestration_and_health_methods_have_stable_wire_names() {
        for (request, method) in [
            (
                ClientRequest::WorkspaceServiceSpecs {
                    request_id: RequestId::Integer(32),
                    params: WorkspaceServiceSpecsParams {
                        project_id: "ws-1".to_owned(),
                    },
                },
                "workspace/service/specs",
            ),
            (
                ClientRequest::WorkspaceServiceStartAll {
                    request_id: RequestId::Integer(33),
                    params: WorkspaceServiceStartAllParams {
                        project_id: "ws-1".to_owned(),
                        names: vec!["backend".to_owned(), "web".to_owned()],
                        secret_values: None,
                        idempotency_key: "orch-1".to_owned(),
                    },
                },
                "workspace/service/startAll",
            ),
            (
                ClientRequest::WorkspaceServiceStopAll {
                    request_id: RequestId::Integer(34),
                    params: WorkspaceServiceStopAllParams {
                        project_id: "ws-1".to_owned(),
                        names: vec![],
                    },
                },
                "workspace/service/stopAll",
            ),
            (
                ClientRequest::WorkspaceServiceHealth {
                    request_id: RequestId::Integer(35),
                    params: WorkspaceServiceHealthParams {
                        service_id: "svc-1".to_owned(),
                        path: Some("/api/health".to_owned()),
                    },
                },
                "workspace/service/health",
            ),
        ] {
            assert_eq!(request.method(), method);
            assert_eq!(
                crate::experimental_api::ExperimentalApi::experimental_reason(&request),
                Some("workspace/service/v1")
            );
        }
    }

    #[test]
    fn workspace_preview_diagnose_has_stable_wire_name_and_is_experimental() {
        let request = ClientRequest::WorkspacePreviewDiagnose {
            request_id: RequestId::Integer(36),
            params: WorkspacePreviewDiagnoseParams {
                project_id: "ws-1".to_owned(),
                failures: vec![WorkspaceObservedNetworkFailure {
                    url: "http://127.0.0.1:8787/api/items".to_owned(),
                    method: Some("GET".to_owned()),
                    status: Some(404),
                    error: None,
                    occurred_at_ms: None,
                }],
                log_tail_bytes: None,
                max_candidates: None,
            },
        };

        assert_eq!(request.method(), "workspace/preview/diagnose");
        assert_eq!(
            crate::experimental_api::ExperimentalApi::experimental_reason(&request),
            Some("workspace/preview/v1")
        );
    }

    #[test]
    fn workspace_service_spec_serializes_full_shape() {
        let spec = WorkspaceServiceSpec {
            id: "spec-1".to_owned(),
            project_id: "ws-1".to_owned(),
            name: "backend".to_owned(),
            root_index: 1,
            script: "dev".to_owned(),
            port: None,
            cwd: Some("backend".to_owned()),
            depends_on: vec!["db".to_owned()],
            health_check: Some(WorkspaceServiceHealthCheck {
                path: "/api/health".to_owned(),
                expect_status: Some(200),
                timeout_ms: None,
            }),
            ready_timeout_ms: None,
            env_refs: Vec::new(),
            created_at_ms: 1_700_000_000_000,
            updated_at_ms: 1_700_000_000_000,
        };

        let value = serde_json::to_value(&spec).expect("serialize spec");
        assert_eq!(value["rootIndex"], 1);
        assert_eq!(value["cwd"], "backend");
        assert_eq!(value["dependsOn"], serde_json::json!(["db"]));
        assert_eq!(value["healthCheck"]["path"], "/api/health");
        assert_eq!(value["healthCheck"]["expectStatus"], 200);
        // Absent-optionals stay off the wire (E2 convention).
        assert!(value.get("port").is_none() || value["port"].is_null());
        assert!(value.get("readyTimeoutMs").is_none() || value["readyTimeoutMs"].is_null());
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub enum WorkspaceServiceChangedReason {
    /// A service reached Ready after start/startAll.
    Started,
    /// Service stopped by stop/stopAll/close (exit recorded, not a crash).
    Stopped,
    /// Process exited on its own with code 0.
    Exited,
    /// Process exited non-zero, readiness timed out, or start failed.
    Failed,
    /// Runtime restarted: previously non-terminal services were normalized
    /// to Stopped; clients should re-issue startAll if desired.
    RuntimeRestarted,
}

/// Broadcast service lifecycle change (E4: replaces client polling).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct WorkspaceServiceChangedNotification {
    pub project_id: String,
    pub reason: WorkspaceServiceChangedReason,
    /// Affected services after the state transition.
    pub services: Vec<WorkspaceServiceRef>,
}
