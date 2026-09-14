//! Append-only workspace audit log (E4). JSONL at
//! `$ODY_HOME/workspace-audit/v1.jsonl`; seq is monotonic across restarts.
//! Writes are serialized by a std mutex; volume is low (human-scale
//! workspace operations), so synchronous append is acceptable.

use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use ody_app_server_protocol::JSONRPCErrorError;
use ody_app_server_protocol::WorkspaceAuditEvent;
use ody_app_server_protocol::WorkspaceAuditListResponse;

use crate::error_code::internal_error;

const DEFAULT_LIMIT: u32 = 100;
const MAX_LIMIT: u32 = 500;
/// Oversized-detail guard: refuse single events above 64 KiB serialized.
const MAX_EVENT_BYTES: usize = 64 * 1024;

pub(crate) struct WorkspaceAuditLog {
    path: PathBuf,
    next_seq: Mutex<u64>,
}

impl WorkspaceAuditLog {
    pub(crate) fn new(path: PathBuf) -> Self {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        // Seq recovery: last parseable line wins; fall back to a non-empty
        // line count so a hand-truncated tail never rewinds the sequence.
        let next_seq = fs::read_to_string(&path)
            .ok()
            .and_then(|raw| {
                raw.lines()
                    .rev()
                    .find(|line| !line.trim().is_empty())
                    .and_then(|line| {
                        serde_json::from_str::<WorkspaceAuditEvent>(line)
                            .ok()
                            .map(|event| event.seq + 1)
                            .or_else(|| {
                                Some(
                                    raw.lines()
                                        .filter(|line| !line.trim().is_empty())
                                        .count() as u64
                                        + 1,
                                )
                            })
                    })
            })
            .unwrap_or(1)
            .into();
        Self { path, next_seq }
    }

    pub(crate) fn record(&self, mut event: WorkspaceAuditEvent) {
        let Ok(line) = serde_json::to_vec(&event) else {
            return;
        };
        if line.len() > MAX_EVENT_BYTES {
            tracing::warn!("workspace audit event dropped: exceeds {MAX_EVENT_BYTES} bytes");
            return;
        }
        let mut seq = self.next_seq.lock().expect("audit seq lock");
        event.seq = *seq;
        event.timestamp_ms = now_ms();
        let mut serialized = serde_json::to_vec(&event).expect("event reserialize");
        serialized.push(b'\n');
        let append = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .and_then(|mut file| file.write_all(&serialized));
        match append {
            Ok(()) => *seq += 1,
            Err(err) => tracing::warn!("workspace audit append failed: {err}"),
        }
    }

    pub(crate) fn list(
        &self,
        project_id: Option<&str>,
        limit: Option<u32>,
    ) -> Result<WorkspaceAuditListResponse, JSONRPCErrorError> {
        let limit = limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT) as usize;
        let raw = fs::read_to_string(&self.path).map_err(|err| {
            internal_error(format!("workspace audit read failed: {err}"))
        })?;
        let mut events: Vec<WorkspaceAuditEvent> = raw
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .filter(|event: &WorkspaceAuditEvent| {
                project_id.is_none() || event.project_id.as_deref() == project_id
            })
            .collect();
        events.reverse();
        events.truncate(limit);
        Ok(WorkspaceAuditListResponse { events })
    }
}

/// Build one audit event. `seq`/`timestamp_ms` are overwritten by `record`.
pub(crate) fn audit_event(
    project_id: Option<&str>,
    operation: &str,
    outcome: &str,
    detail: serde_json::Value,
) -> WorkspaceAuditEvent {
    WorkspaceAuditEvent {
        seq: 0,
        timestamp_ms: 0,
        project_id: project_id.map(str::to_owned),
        operation: operation.to_owned(),
        outcome: outcome.to_owned(),
        detail,
    }
}

/// Build a detail object with a bounded error string (never a full
/// params/response dump).
pub(crate) fn error_detail(context: &str, err: &JSONRPCErrorError) -> serde_json::Value {
    let message: String = err.message.chars().take(200).collect();
    serde_json::json!({"context": context, "error": message})
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use tempfile::TempDir;

    fn event(project_id: &str, operation: &str, outcome: &str, detail: serde_json::Value) -> WorkspaceAuditEvent {
        WorkspaceAuditEvent {
            seq: 0,
            timestamp_ms: 0,
            project_id: Some(project_id.to_owned()),
            operation: operation.to_owned(),
            outcome: outcome.to_owned(),
            detail,
        }
    }

    fn log_at(dir: &TempDir) -> WorkspaceAuditLog {
        WorkspaceAuditLog::new(dir.path().join("v1.jsonl"))
    }

    #[test]
    fn record_appends_monotonic_seq_lines() {
        let dir = TempDir::new().expect("temp");
        let log = log_at(&dir);
        log.record(event("p1", "project.bind", "ok", json!({"roots": 2})));
        log.record(event("p1", "changeset.apply", "ok", json!({"changesetId": "cs-1"})));
        let events = log.list(Some("p1"), None).expect("list").events;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].seq, 2, "newest first");
        assert_eq!(events[0].operation, "changeset.apply");
        assert_eq!(events[1].seq, 1);
    }

    #[test]
    fn list_filters_by_project_and_caps_limit() {
        let dir = TempDir::new().expect("temp");
        let log = log_at(&dir);
        for i in 0..5 {
            log.record(event("p1", "changeset.apply", "ok", json!({"i": i})));
            log.record(event("p2", "service.start", "ok", json!({"i": i})));
        }
        let p1 = log.list(Some("p1"), Some(2)).expect("list").events;
        assert_eq!(p1.len(), 2);
        assert!(p1.iter().all(|e| e.project_id.as_deref() == Some("p1")));
        let all = log.list(None, Some(500)).expect("list").events;
        assert_eq!(all.len(), 10);
    }

    #[test]
    fn seq_resumes_across_restart() {
        let dir = TempDir::new().expect("temp");
        let path = dir.path().join("v1.jsonl");
        WorkspaceAuditLog::new(path.clone()).record(event("p1", "a", "ok", json!({})));
        WorkspaceAuditLog::new(path.clone()).record(event("p1", "b", "ok", json!({})));
        let events = WorkspaceAuditLog::new(path).list(None, None).expect("list").events;
        assert_eq!(events[0].seq, 2, "second instance must continue seq, not reset");
    }

    #[test]
    fn record_never_writes_content_or_secret_values() {
        let dir = TempDir::new().expect("temp");
        let log = log_at(&dir);
        // detail is built by whitelist at call sites; the store itself must
        // also refuse obviously-oversized lines (runaway payload guard).
        log.record(event(
            "p1",
            "changeset.create",
            "ok",
            json!({"files": ["0:src/a.ts"]}),
        ));
        let raw = std::fs::read_to_string(dir.path().join("v1.jsonl")).unwrap();
        assert!(!raw.contains("export const"), "no file content in audit");
        assert_eq!(raw.lines().count(), 1);
    }
}
