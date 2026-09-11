//! Native session-report gate, a Rust port of the open-steps `stop-report.sh`
//! idea: when a turn is about to stop and real work has landed in the working
//! repositories since this session's baseline, block the stop once and ask the
//! model for a short wrap-up report. Reports live outside the repositories, so
//! producing one never changes the fingerprint — the cooldown is what prevents
//! a report loop.
//!
//! Cross-platform by construction: everything runs in-process via the `git`
//! CLI (the same mechanism `ody-git-utils` uses everywhere); when git is
//! unavailable or a command fails, the gate fails open and stays silent.
//!
//! Enabled via the `session_report_gate` feature flag. Tunables are constants
//! for now; promote them to a feature config if real usage asks for it.

use std::collections::hash_map::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use ody_features::Feature;
use ody_hooks::StopOutcome;
use ody_protocol::items::HookPromptFragment;

use crate::session::session::Session;
use crate::session::turn_context::TurnContext;

/// Quiet period between report requests. After the gate fires once, stops
/// inside this window are allowed so the stop always terminates.
const DEFAULT_COOLDOWN: Duration = Duration::from_secs(300);
/// Ignore changes smaller than this when no commit landed.
const DEFAULT_MIN_FILES_CHANGED: usize = 1;
/// At most this many sibling repositories are scanned from a repo-hub cwd.
const DEFAULT_MAX_REPOS: usize = 25;

/// Applies the session-report gate to a root-turn stop outcome. No-op unless
/// the feature flag is on, a user-configured stop hook already blocked, or the
/// workspace scan finds nothing (no git, not a repo).
pub(crate) async fn apply_session_report_gate(
    sess: &std::sync::Arc<Session>,
    turn_context: &std::sync::Arc<TurnContext>,
    outcome: &mut StopOutcome,
) {
    if outcome.should_block || !turn_context.config.features.enabled(Feature::SessionReportGate) {
        return;
    }

    let session_id = sess.session_id().to_string();
    #[allow(deprecated)]
    let cwd = turn_context.cwd.clone();
    let ody_home = turn_context.config.ody_home.clone();
    let settings = GateSettings::default();
    let hook_run_id = format!("session-report-gate:{}", turn_context.sub_id);

    let decision = match tokio::task::spawn_blocking(move || {
        evaluate(cwd.as_path(), &ody_home, &session_id, &settings)
    })
    .await
    {
        Ok(decision) => decision,
        Err(err) => {
            tracing::warn!(error = %err, "session report gate task failed; allowing stop");
            return;
        }
    };

    let Decision::Block(report) = decision else {
        return;
    };

    tracing::info!(
        session_id = %sess.session_id(),
        changed = ?report.changed,
        "session report gate blocking stop for wrap-up report"
    );

    outcome.should_block = true;
    outcome.block_reason = Some(report.block_reason.clone());
    let fragment = HookPromptFragment::from_single_hook(report.block_reason, hook_run_id);
    outcome.continuation_fragments.push(fragment);
}

#[derive(Debug, Clone, Copy)]
struct GateSettings {
    cooldown: Duration,
    min_files_changed: usize,
    max_repos: usize,
}

impl Default for GateSettings {
    fn default() -> Self {
        Self {
            cooldown: DEFAULT_COOLDOWN,
            min_files_changed: DEFAULT_MIN_FILES_CHANGED,
            max_repos: DEFAULT_MAX_REPOS,
        }
    }
}

/// Persistent per-workspace baseline, keyed by workspace scope under
/// `<ody_home>/state/session-report-gate/`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct GateState {
    session_id: String,
    fingerprint: u64,
    heads: String,
    /// Unix seconds of the last report request; 0 = none yet.
    fired_at_unix: u64,
}

/// Snapshot of every working repository found under the cwd.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct WorkspaceScan {
    fingerprint: u64,
    heads: String,
    dirty_files: usize,
    changed: Vec<String>,
}

/// What the gate decided after comparing the fresh scan against the baseline.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Decision {
    Allow,
    Block(BlockReport),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BlockReport {
    block_reason: String,
    changed: Vec<String>,
}

/// Full gate evaluation: scan, load the stored baseline, decide, persist.
fn evaluate(
    cwd: &Path,
    ody_home: &Path,
    session_id: &str,
    settings: &GateSettings,
) -> Decision {
    let repos = find_repos(cwd, settings.max_repos);
    if repos.is_empty() {
        return Decision::Allow;
    }
    let scan = scan_workspace(&repos);
    let scope = workspace_scope(cwd, &repos);
    let state_path = state_file_path(ody_home, &scope);

    let previous = read_state(&state_path);
    let decision = decide(previous.as_ref(), &scan, session_id, now_unix(), settings);

    match &decision {
        Decision::Allow => {
            // First stop of a new session records the baseline so pre-existing
            // changes are never blamed on this session.
            if previous.as_ref().is_none_or(|s| s.session_id != session_id) {
                write_state(
                    &state_path,
                    &GateState {
                        session_id: session_id.to_string(),
                        fingerprint: scan.fingerprint,
                        heads: scan.heads.clone(),
                        fired_at_unix: previous.as_ref().map_or(0, |s| s.fired_at_unix),
                    },
                );
            }
        }
        Decision::Block(_) => {
            // Record the fired time and advance the baseline to the current
            // fingerprint: each change-set triggers at most one report.
            write_state(
                &state_path,
                &GateState {
                    session_id: session_id.to_string(),
                    fingerprint: scan.fingerprint,
                    heads: scan.heads.clone(),
                    fired_at_unix: now_unix(),
                },
            );
        }
    }
    decision
}

/// Pure decision logic, separated from IO so every branch is unit-testable.
fn decide(
    previous: Option<&GateState>,
    scan: &WorkspaceScan,
    session_id: &str,
    now_unix: u64,
    settings: &GateSettings,
) -> Decision {
    let Some(previous) = previous else {
        // No baseline yet: record one and never blame pre-existing changes.
        return Decision::Allow;
    };
    if previous.session_id != session_id {
        return Decision::Allow;
    }
    if scan.fingerprint == previous.fingerprint {
        return Decision::Allow;
    }
    // Too small to be worth a report when no commit landed.
    if scan.dirty_files < settings.min_files_changed && scan.heads == previous.heads {
        return Decision::Allow;
    }
    if previous.fired_at_unix > 0
        && now_unix.saturating_sub(previous.fired_at_unix) < settings.cooldown.as_secs()
    {
        return Decision::Allow;
    }

    let mut changed = scan.changed.clone();
    changed.sort();
    changed.dedup();
    let changed_list = if changed.is_empty() {
        "working repositories".to_string()
    } else {
        changed.join(", ")
    };
    Decision::Block(BlockReport {
        block_reason: format!(
            "Work landed during this session in {changed_list} and has not been wrapped up. \
             Before stopping, give the user a short plain-language summary of what changed: \
             files touched, commits made, and verification status. Then stop. \
             Keep the summary under ten lines."
        ),
        changed,
    })
}

/// The cwd's repository, or every repository one level below it (a hub of
/// checkouts). Deeper nesting is not scanned, mirroring the reference hook.
fn find_repos(cwd: &Path, max_repos: usize) -> Vec<PathBuf> {
    if let Some(root) = git_stdout(cwd, &["rev-parse", "--show-toplevel"]) {
        if let Some(root) = canonical_repo_root(root.trim()) {
            return vec![root];
        }
    }
    let mut repos = Vec::new();
    let Ok(entries) = std::fs::read_dir(cwd) else {
        return repos;
    };
    for entry in entries.flatten() {
        if repos.len() >= max_repos {
            break;
        }
        let path = entry.path();
        // Dir or file: worktrees use a `.git` file.
        if !path.join(".git").exists() {
            continue;
        }
        if let Some(root) = git_stdout(&path, &["rev-parse", "--show-toplevel"]) {
            if let Some(root) = canonical_repo_root(root.trim()) {
                if !repos.contains(&root) {
                    repos.push(root);
                }
            }
        }
    }
    repos
}

/// Canonicalize a repo root so comparisons and state-file scopes are stable
/// across platforms (on Windows, `git` returns `C:/...` while
/// `canonicalize` returns `\\?\C:\...`).
fn canonical_repo_root(root: &str) -> Option<PathBuf> {
    let root = PathBuf::from(root);
    if !root.is_dir() {
        return None;
    }
    Some(std::fs::canonicalize(&root).unwrap_or(root))
}

fn scan_workspace(repos: &[PathBuf]) -> WorkspaceScan {
    let mut scan = WorkspaceScan::default();
    let mut hasher = DefaultHasher::new();
    for repo in repos {
        let Some(head) = git_stdout(&repo, &["rev-parse", "HEAD"]) else {
            continue;
        };
        let head = head.trim().to_string();
        let status = git_stdout(&repo, &["status", "--porcelain"]).unwrap_or_default();
        let dirty: Vec<&str> = status.lines().filter(|line| !line.trim().is_empty()).collect();
        // Content, not just names: `status --porcelain` alone cannot see a
        // file edited twice.
        let diff = git_bytes(&repo, &["diff", "HEAD"]).unwrap_or_default();
        let mut repo_hasher = DefaultHasher::new();
        head.hash(&mut repo_hasher);
        status.hash(&mut repo_hasher);
        diff.hash(&mut repo_hasher);
        let repo_hash = repo_hasher.finish();

        scan.dirty_files += dirty.len();
        if !dirty.is_empty() {
            scan.changed.push(
                repo
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| repo.display().to_string()),
            );
        }
        repo_hash.hash(&mut hasher);
        scan.heads.push_str(&head);
        scan.heads.push('|');
    }
    scan.fingerprint = hasher.finish();
    scan
}

/// Workspace scope for the state file: the single repo's name, or the cwd's
/// name for a repo hub. Sanitized so it is always a safe file name.
fn workspace_scope(cwd: &Path, repos: &[PathBuf]) -> String {
    let raw = if repos.len() == 1 {
        repos[0]
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "workspace".to_string())
    } else {
        cwd.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "workspace".to_string())
    };
    sanitize_scope(&raw)
}

fn sanitize_scope(raw: &str) -> String {
    let sanitized: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "workspace".to_string()
    } else {
        sanitized
    }
}

fn state_file_path(ody_home: &Path, scope: &str) -> PathBuf {
    ody_home
        .join("state")
        .join("session-report-gate")
        .join(format!("{scope}.json"))
}

fn read_state(path: &Path) -> Option<GateState> {
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write_state(path: &Path, state: &GateState) {
    let Some(parent) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(state) {
        let _ = std::fs::write(path, bytes);
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

/// Run a git command, returning stdout on success. Any failure (git missing,
/// not a repo, non-zero exit) becomes `None` so the gate fails open.
fn git_stdout(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

fn git_bytes(cwd: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(output.stdout)
}

#[cfg(test)]
#[path = "session_report_gate_tests.rs"]
mod tests;
