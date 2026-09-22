//! Pending user operations: what the user asked to forget, keep, or unpin.
//!
//! The settings panel never touches the store. It writes one small JSON file per
//! request under `user_ops/`, and the runtime applies them on its next pass and
//! deletes each file it handled. One file per operation is what keeps the two
//! writers — the UI and the runtime — from racing over a shared file, and a
//! deleted file is what keeps an application idempotent.

use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;

use crate::MemoryDb;
use crate::MemoryDbError;

/// Directory holding one file per pending user operation.
pub const USER_OPS_SUBDIR: &str = "user_ops";

/// What the user asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserOpKind {
    /// Forget this claim. The row is kept for a short audit window but stops
    /// being retrieved, injected, or shown.
    Delete,
    /// Keep this claim in front of the budget.
    Pin,
    /// Stop favouring this claim.
    Unpin,
}

/// One pending request, as written by odyBox.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserOp {
    pub op: UserOpKind,
    pub claim_id: String,
}

/// Tally of one application pass.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct UserOpsOutcome {
    pub applied: usize,
    /// Requests that could not be applied — an unknown claim, or an unreadable
    /// file. Counted so a silent failure is still visible in the log.
    pub failed: usize,
}

pub fn user_ops_dir(memory_root: &Path) -> PathBuf {
    memory_root.join(USER_OPS_SUBDIR)
}

/// Applies every pending operation, clearing each file it handled.
///
/// A request that cannot be applied is dropped rather than retried forever: the
/// panel shows the current view, so the user can simply ask again.
pub async fn apply_pending(
    db: &MemoryDb,
    memory_root: &Path,
) -> Result<UserOpsOutcome, MemoryDbError> {
    let dir = user_ops_dir(memory_root);
    let mut reader = match tokio::fs::read_dir(&dir).await {
        Ok(reader) => reader,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(UserOpsOutcome::default());
        }
        Err(err) => return Err(err.into()),
    };

    let mut outcome = UserOpsOutcome::default();
    while let Some(entry) = reader.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }

        let Some(op) = read_op(&path).await else {
            tracing::warn!(path = %path.display(), "assistant memory: unreadable user operation");
            outcome.failed += 1;
            let _ = tokio::fs::remove_file(&path).await;
            continue;
        };

        let applied = match op.op {
            UserOpKind::Delete => db.delete_claim(&op.claim_id).await,
            UserOpKind::Pin => db.set_pinned(&op.claim_id, true).await,
            UserOpKind::Unpin => db.set_pinned(&op.claim_id, false).await,
        };

        match applied {
            Ok(()) => outcome.applied += 1,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    claim_id = %op.claim_id,
                    "assistant memory: could not apply user operation"
                );
                outcome.failed += 1;
            }
        }

        let _ = tokio::fs::remove_file(&path).await;
    }

    Ok(outcome)
}

async fn read_op(path: &Path) -> Option<UserOp> {
    let raw = tokio::fs::read_to_string(path).await.ok()?;
    let op: UserOp = serde_json::from_str(&raw).ok()?;
    if op.claim_id.trim().is_empty() {
        return None;
    }
    Some(op)
}

#[cfg(test)]
#[path = "user_ops_tests.rs"]
mod tests;
