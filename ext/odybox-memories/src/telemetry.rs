//! Run observability: what the memory system actually did, on this machine.
//!
//! Two artefacts, both local and both plain JSON — nothing here leaves the
//! device, because memory is personal by definition:
//!
//! * `metrics.jsonl` — append-only detail, one line per pipeline pass. This is
//!   the file to analyse later with `jq` when asking "is it working, and where
//!   does it lose things?".
//! * `metrics_summary.json` — the rolled-up counters the settings panel shows,
//!   so the plain question "has memory done anything at all?" is answerable
//!   without opening a log.
//!
//! The counters are chosen around the failure modes that matter: nothing
//! extracted, candidates dropped for having no evidence, claims superseded,
//! injected claims, and failed writes.

use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

/// Append-only detail, one line per pipeline pass.
pub const EVENTS_FILENAME: &str = "metrics.jsonl";
/// Rolled-up counters, rewritten after every pass.
pub const SUMMARY_FILENAME: &str = "metrics_summary.json";

/// What one pipeline pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PassEvent {
    /// RFC 3339 timestamp of when the pass finished.
    pub at: String,
    pub duration_ms: u64,
    pub sessions_seen: usize,
    pub extracted: usize,
    pub empty: usize,
    pub failed: usize,
    /// Claims the extraction model proposed.
    pub candidates: usize,
    /// Claims thrown away because they cited no transcript line, or a line that
    /// is not in that transcript. A high number points at the prompt or model.
    pub dropped_ungrounded: usize,
    pub added: usize,
    pub skipped: usize,
    pub superseded: usize,
    pub store_failed: usize,
    pub consolidated_subjects: usize,
    pub subsumed: usize,
    pub ops_applied: usize,
    pub ops_failed: usize,
    /// Claims shown to the model as "what you already hold".
    pub known_claims_shown: usize,
    /// Claims in the injected memory block.
    pub injected_claims: usize,
}

/// Counters carried across passes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemorySummary {
    pub passes: u64,
    pub last_pass_at: Option<String>,
    pub last_duration_ms: u64,
    pub claims_added: u64,
    pub claims_superseded: u64,
    pub claims_dropped_ungrounded: u64,
    pub store_failures: u64,
    pub claims_in_store: u64,
    pub last_injected_claims: usize,
}

pub fn events_path(memory_root: &Path) -> PathBuf {
    memory_root.join(EVENTS_FILENAME)
}

pub fn summary_path(memory_root: &Path) -> PathBuf {
    memory_root.join(SUMMARY_FILENAME)
}

/// Appends one pass to the detail log.
pub async fn append_event(memory_root: &Path, event: &PassEvent) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;

    tokio::fs::create_dir_all(memory_root).await?;
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(events_path(memory_root))
        .await?;
    let mut line = serde_json::to_string(event).unwrap_or_else(|_| "{}".to_string());
    line.push('\n');
    file.write_all(line.as_bytes()).await
}

/// Rolls one pass into the summary the panel reads.
pub async fn update_summary(
    memory_root: &Path,
    event: &PassEvent,
    claims_in_store: u64,
) -> std::io::Result<MemorySummary> {
    let mut summary = read_summary(memory_root).await;
    summary.passes += 1;
    summary.last_pass_at = Some(event.at.clone());
    summary.last_duration_ms = event.duration_ms;
    summary.claims_added += event.added as u64;
    summary.claims_superseded += (event.superseded + event.subsumed) as u64;
    summary.claims_dropped_ungrounded += event.dropped_ungrounded as u64;
    summary.store_failures += (event.store_failed + event.failed) as u64;
    summary.claims_in_store = claims_in_store;
    summary.last_injected_claims = event.injected_claims;

    tokio::fs::create_dir_all(memory_root).await?;
    let body = serde_json::to_string_pretty(&summary).unwrap_or_else(|_| "{}".to_string());
    tokio::fs::write(summary_path(memory_root), format!("{body}\n")).await?;
    Ok(summary)
}

/// Reads the summary; a missing or damaged file reads as "nothing yet".
pub async fn read_summary(memory_root: &Path) -> MemorySummary {
    tokio::fs::read_to_string(summary_path(memory_root))
        .await
        .ok()
        .and_then(|raw| serde_json::from_str::<MemorySummary>(&raw).ok())
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "telemetry_tests.rs"]
mod tests;
