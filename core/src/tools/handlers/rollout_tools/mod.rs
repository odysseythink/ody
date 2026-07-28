//! Rollout-aware tools for exploring Ody session files.
//!
//! These tools wrap `ody_rollout` line readers, the reverse scanner, and the
//! session-file search so models can inspect large `.jsonl` / `.jsonl.zst`
//! session files without raw shell commands.

mod read;
mod search;
mod spec;
mod tail;

pub use read::RolloutReadHandler;
pub use search::RolloutSearchHandler;
pub use spec::RolloutToolOptions;
pub use tail::RolloutTailHandler;

use crate::function_tool::FunctionCallError;
use crate::session::turn_context::TurnContext;
use std::path::Path;
use std::path::PathBuf;

/// Resolves a rollout path from either an explicit `path` argument or a `thread_id`.
///
/// Absolute paths are used as-is. Relative paths are joined against the Ody home
/// directory (from the turn config). `thread_id` triggers a filesystem scan
/// under `<ody_home>/sessions`.
async fn resolve_rollout_path(
    turn: &TurnContext,
    path: Option<&str>,
    thread_id: Option<&str>,
) -> Result<PathBuf, FunctionCallError> {
    let ody_home = turn.config.ody_home.to_path_buf();

    match (path, thread_id) {
        (Some(path), None) => Ok(resolve_path(&ody_home, path)),
        (None, Some(thread_id)) => find_by_thread_id(ody_home, thread_id).await,
        (Some(path), Some(_thread_id)) => {
            // Prefer the explicit path; ignore the thread_id to avoid ambiguity.
            Ok(resolve_path(&ody_home, path))
        }
        (None, None) => Err(FunctionCallError::RespondToModel(
            "rollout tools require either `path` or `thread_id`".to_string(),
        )),
    }
}

fn resolve_path(ody_home: &Path, path: &str) -> PathBuf {
    let candidate = Path::new(path);
    if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        ody_home.join(path)
    }
}

async fn find_by_thread_id(
    ody_home: PathBuf,
    thread_id: &str,
) -> Result<PathBuf, FunctionCallError> {
    let found = ody_rollout::find_thread_path_by_id_str(ody_home.as_path(), thread_id, None)
        .await
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!(
                "failed to locate rollout for thread_id `{thread_id}`: {err}"
            ))
        })?;
    match found {
        Some(path) => Ok(path),
        None => Err(FunctionCallError::RespondToModel(format!(
            "no rollout file found for thread_id `{thread_id}` under {}",
            ody_home.display()
        ))),
    }
}

/// Truncates a long record line for display, matching the file-tools convention.
fn clip_record_line(line: &str) -> String {
    if line.chars().count() <= spec::MAX_RECORD_LINE_LENGTH {
        return line.to_string();
    }
    let kept: String = line.chars().take(spec::MAX_RECORD_LINE_LENGTH).collect();
    format!("{kept}… [line truncated]")
}

/// Formats a collection of numbered records as `cat -n`-style output.
fn render_numbered_records(start_index: usize, records: &[String]) -> String {
    let mut out = String::new();
    for (index, record) in records.iter().enumerate() {
        let number = start_index + index + 1;
        out.push_str(&format!("{number:6}\t{record}\n"));
    }
    out
}

/// Appends a pagination notice when a read was capped.
fn append_truncation_notice(content: &mut String, truncated: bool) {
    if truncated {
        content.push_str("\n[truncated; use offset/limit or rollout_tail to see more]\n");
    }
}

#[cfg(test)]
mod resolve_tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn absolute_path_is_used_as_is() {
        let dir = tempdir().expect("tempdir");
        let ody_home = dir.path().join("ody");
        let resolved = resolve_path(&ody_home, "/tmp/rollout.jsonl");
        assert_eq!(resolved, PathBuf::from("/tmp/rollout.jsonl"));
    }

    #[test]
    fn relative_path_is_resolved_against_ody_home() {
        let dir = tempdir().expect("tempdir");
        let ody_home = dir.path().join("ody");
        let resolved = resolve_path(&ody_home, "sessions/rollout.jsonl");
        assert_eq!(resolved, ody_home.join("sessions/rollout.jsonl"));
    }
}
