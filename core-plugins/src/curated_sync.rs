//! Git-only startup sync for the curated plugin marketplace.
//!
//! Mirrors the upstream openai/plugins repository (the source of the
//! `openai-curated` marketplace) into `$ODY_HOME/.tmp/plugins` so plugin
//! listing/discovery can surface it without any manual marketplace setup.
//! The sync is git-only: a depth-1 fetch of the remote HEAD, staged in a
//! temporary directory and activated atomically. There is deliberately no
//! HTTP fallback or export-archive path (those upstream fallbacks depend on
//! the ChatGPT backend, which ody does not use).

use std::fs::File;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Output;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tempfile::TempDir;
use tracing::warn;

use crate::loader::curated_plugins_repo_path;

/// Upstream curated plugins repository mirrored into the local curated snapshot.
pub const CURATED_PLUGINS_GIT_URL: &str = "https://github.com/openai/plugins.git";

const GIT: &str = "git";
const CURATED_PLUGINS_FETCH_REF: &str = "refs/ody/curated-sync";
const CURATED_PLUGINS_SHA_FILE: &str = ".tmp/plugins.sha";
const CURATED_PLUGINS_SYNC_LOCK_FILE: &str = ".tmp/plugins.sync.lock";
const CURATED_PLUGINS_GIT_TIMEOUT: Duration = Duration::from_secs(30);

/// Guards against spawning more than one curated repo sync thread per process.
static CURATED_REPO_SYNC_SPAWNED: AtomicBool = AtomicBool::new(false);

/// Spawn a background thread that syncs the curated plugins repo snapshot and
/// runs `on_success` afterwards so hosts can invalidate stale plugin caches.
/// Runs at most once per process; failure resets the guard so a later call
/// can retry. Intended for long-lived hosts; short-lived CLI processes exit
/// before the sync can finish and should not call this.
pub(crate) fn spawn_curated_repo_sync_with_hook(
    ody_home: PathBuf,
    on_success: Option<Arc<dyn Fn() + Send + Sync>>,
) {
    if CURATED_REPO_SYNC_SPAWNED.swap(true, Ordering::SeqCst) {
        return;
    }
    let spawn_result = std::thread::Builder::new()
        .name("plugins-curated-repo-sync".to_string())
        .spawn(move || match sync_curated_plugins_repo(ody_home.as_path()) {
            Ok(_version) => {
                if let Some(on_success) = on_success {
                    on_success();
                }
            }
            Err(err) => {
                CURATED_REPO_SYNC_SPAWNED.store(false, Ordering::SeqCst);
                warn!("failed to sync curated plugins repo: {err}");
            }
        });
    if let Err(err) = spawn_result {
        CURATED_REPO_SYNC_SPAWNED.store(false, Ordering::SeqCst);
        warn!("failed to start curated plugins repo sync task: {err}");
    }
}

/// Sync the curated plugins repository into `$ODY_HOME/.tmp/plugins`.
///
/// Returns the remote HEAD sha used as the snapshot version. Syncing is a
/// no-op when the local snapshot already matches the remote HEAD.
pub fn sync_curated_plugins_repo(ody_home: &Path) -> Result<String, String> {
    sync_curated_plugins_repo_from_url(ody_home, CURATED_PLUGINS_GIT_URL)
}

pub(crate) fn sync_curated_plugins_repo_from_url(
    ody_home: &Path,
    git_url: &str,
) -> Result<String, String> {
    let _lock = lock_curated_plugins_sync(ody_home)?;
    let repo_path = curated_plugins_repo_path(ody_home);
    let sha_path = ody_home.join(CURATED_PLUGINS_SHA_FILE);
    let remote_sha = git_ls_remote_head_sha(git_url)?;
    let local_sha = read_local_git_or_sha_file(&repo_path, &sha_path);

    if local_sha.as_deref() == Some(remote_sha.as_str()) && repo_path.join(".git").is_dir() {
        return Ok(remote_sha);
    }

    let staged_repo_dir = prepare_staged_repo(&repo_path)?;
    run_git_in_repo(
        staged_repo_dir.path(),
        &["init"],
        "git init curated plugins repo",
    )?;

    if repo_path.join(".git").is_dir() {
        // Refresh the FETCH_REF in the existing snapshot as well so a failed
        // activation never strands it without the matching ref.
        fetch_curated_plugins_commit(&repo_path, git_url, &remote_sha)?;
        fetch_curated_plugins_commit_from_source(staged_repo_dir.path(), &repo_path)?;
    } else {
        fetch_curated_plugins_commit(staged_repo_dir.path(), git_url, &remote_sha)?;
    }

    run_git_in_repo(
        staged_repo_dir.path(),
        &["reset", "--hard", CURATED_PLUGINS_FETCH_REF],
        "git reset curated plugins repo",
    )?;
    let fetched_sha = git_head_sha(staged_repo_dir.path())?;
    if fetched_sha != remote_sha {
        return Err(format!(
            "curated plugins fetch HEAD mismatch: expected {remote_sha}, got {fetched_sha}"
        ));
    }

    ensure_marketplace_manifest_exists(staged_repo_dir.path())?;
    activate_curated_repo(&repo_path, staged_repo_dir)?;
    write_curated_plugins_sha(&sha_path, &remote_sha)?;
    Ok(remote_sha)
}

fn lock_curated_plugins_sync(ody_home: &Path) -> Result<File, String> {
    let lock_path = ody_home.join(CURATED_PLUGINS_SYNC_LOCK_FILE);
    std::fs::create_dir_all(ody_home.join(".tmp"))
        .map_err(|err| format!("failed to create curated plugins sync directory: {err}"))?;
    let lock_file = File::options()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|err| format!("failed to open curated plugins sync lock: {err}"))?;
    lock_file
        .lock()
        .map_err(|err| format!("failed to lock curated plugins sync: {err}"))?;
    Ok(lock_file)
}

fn prepare_staged_repo(repo_path: &Path) -> Result<TempDir, String> {
    let Some(parent) = repo_path.parent() else {
        return Err(format!(
            "failed to determine curated plugins parent directory for {}",
            repo_path.display()
        ));
    };
    std::fs::create_dir_all(parent).map_err(|err| {
        format!(
            "failed to create curated plugins parent directory {}: {err}",
            parent.display()
        )
    })?;
    tempfile::Builder::new()
        .prefix("plugins-clone-")
        .tempdir_in(parent)
        .map_err(|err| {
            format!(
                "failed to create temporary curated plugins directory in {}: {err}",
                parent.display()
            )
        })
}

fn fetch_curated_plugins_commit(
    repo_path: &Path,
    git_url: &str,
    remote_sha: &str,
) -> Result<(), String> {
    let fetch_refspec = format!("+{remote_sha}:{CURATED_PLUGINS_FETCH_REF}");
    let output = run_git_command_with_timeout(
        Command::new(GIT)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .arg("-C")
            .arg(repo_path)
            .args(["fetch", "--depth", "1", "--no-tags"])
            .arg(git_url)
            .arg(fetch_refspec),
        "git fetch curated plugins repo",
        CURATED_PLUGINS_GIT_TIMEOUT,
    )?;
    ensure_git_success(&output, "git fetch curated plugins repo")
}

fn fetch_curated_plugins_commit_from_source(
    repo_path: &Path,
    source_repo_path: &Path,
) -> Result<(), String> {
    let fetch_refspec = format!("+{CURATED_PLUGINS_FETCH_REF}:{CURATED_PLUGINS_FETCH_REF}");
    let output = run_git_command_with_timeout(
        Command::new(GIT)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .arg("-C")
            .arg(repo_path)
            .args(["fetch", "--depth", "1", "--no-tags"])
            .arg(source_repo_path)
            .arg(fetch_refspec),
        "git copy fetched curated plugins commit",
        CURATED_PLUGINS_GIT_TIMEOUT,
    )?;
    ensure_git_success(&output, "git copy fetched curated plugins commit")
}

fn ensure_marketplace_manifest_exists(repo_path: &Path) -> Result<(), String> {
    if repo_path.join(".agents/plugins/marketplace.json").is_file() {
        return Ok(());
    }
    Err(format!(
        "curated plugins snapshot missing marketplace manifest at {}",
        repo_path.join(".agents/plugins/marketplace.json").display()
    ))
}

/// Move the staged snapshot into place, keeping the previous snapshot as a
/// rollback source if the move fails midway.
fn activate_curated_repo(repo_path: &Path, staged_repo_dir: TempDir) -> Result<(), String> {
    let staged_repo_path = staged_repo_dir.path();
    if repo_path.exists() {
        let parent = repo_path.parent().ok_or_else(|| {
            format!(
                "failed to determine curated plugins parent directory for {}",
                repo_path.display()
            )
        })?;
        let backup_dir = tempfile::Builder::new()
            .prefix("plugins-backup-")
            .tempdir_in(parent)
            .map_err(|err| {
                format!(
                    "failed to create curated plugins backup directory in {}: {err}",
                    parent.display()
                )
            })?;
        let backup_repo_path = backup_dir.path().join("repo");

        std::fs::rename(repo_path, &backup_repo_path).map_err(|err| {
            format!(
                "failed to move previous curated plugins repo out of the way at {}: {err}",
                repo_path.display()
            )
        })?;

        if let Err(err) = std::fs::rename(staged_repo_path, repo_path) {
            let rollback_result = std::fs::rename(&backup_repo_path, repo_path);
            return match rollback_result {
                Ok(()) => Err(format!(
                    "failed to activate new curated plugins repo at {}: {err}",
                    repo_path.display()
                )),
                Err(rollback_err) => {
                    let backup_path = backup_dir.keep().join("repo");
                    Err(format!(
                        "failed to activate new curated plugins repo at {}: {err}; failed to restore previous repo (left at {}): {rollback_err}",
                        repo_path.display(),
                        backup_path.display()
                    ))
                }
            };
        }
    } else {
        std::fs::rename(staged_repo_path, repo_path).map_err(|err| {
            format!(
                "failed to activate curated plugins repo at {}: {err}",
                repo_path.display()
            )
        })?;
    }

    Ok(())
}

fn write_curated_plugins_sha(sha_path: &Path, remote_sha: &str) -> Result<(), String> {
    if let Some(parent) = sha_path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| {
            format!(
                "failed to create curated plugins sha directory {}: {err}",
                parent.display()
            )
        })?;
    }
    std::fs::write(sha_path, format!("{remote_sha}\n")).map_err(|err| {
        format!(
            "failed to write curated plugins sha file {}: {err}",
            sha_path.display()
        )
    })
}

fn read_sha_file(sha_path: &Path) -> Option<String> {
    let contents = std::fs::read_to_string(sha_path).ok()?;
    let sha = contents.trim();
    if sha.is_empty() {
        None
    } else {
        Some(sha.to_string())
    }
}

fn read_local_git_or_sha_file(repo_path: &Path, sha_path: &Path) -> Option<String> {
    if repo_path.join(".git").is_dir()
        && let Ok(sha) = git_head_sha(repo_path)
    {
        return Some(sha);
    }

    read_sha_file(sha_path)
}

fn git_ls_remote_head_sha(git_url: &str) -> Result<String, String> {
    let output = run_git_command_with_timeout(
        Command::new(GIT)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .arg("ls-remote")
            .arg(git_url)
            .arg("HEAD"),
        "git ls-remote curated plugins repo",
        CURATED_PLUGINS_GIT_TIMEOUT,
    )?;
    ensure_git_success(&output, "git ls-remote curated plugins repo")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let Some(first_line) = stdout.lines().next() else {
        return Err("git ls-remote returned empty output for curated plugins repo".to_string());
    };
    let Some((sha, _)) = first_line.split_once('\t') else {
        return Err(format!(
            "unexpected git ls-remote output for curated plugins repo: {first_line}"
        ));
    };
    if sha.is_empty() {
        return Err("git ls-remote returned empty sha for curated plugins repo".to_string());
    }
    Ok(sha.to_string())
}

fn git_head_sha(repo_path: &Path) -> Result<String, String> {
    let output = Command::new(GIT)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(repo_path)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .map_err(|err| {
            format!(
                "failed to run git rev-parse HEAD in {}: {err}",
                repo_path.display()
            )
        })?;
    ensure_git_success(&output, "git rev-parse HEAD")?;

    let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if sha.is_empty() {
        return Err(format!(
            "git rev-parse HEAD returned empty output in {}",
            repo_path.display()
        ));
    }
    Ok(sha)
}

fn run_git_in_repo(repo_path: &Path, args: &[&str], context: &str) -> Result<(), String> {
    let output = run_git_command_with_timeout(
        Command::new(GIT)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .arg("-C")
            .arg(repo_path)
            .args(args),
        context,
        CURATED_PLUGINS_GIT_TIMEOUT,
    )?;
    ensure_git_success(&output, context)
}

fn run_git_command_with_timeout(
    command: &mut Command,
    context: &str,
    timeout: Duration,
) -> Result<Output, String> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("failed to run {context}: {err}"))?;

    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return child
                    .wait_with_output()
                    .map_err(|err| format!("failed to wait for {context}: {err}"));
            }
            Ok(None) => {}
            Err(err) => return Err(format!("failed to poll {context}: {err}")),
        }

        if start.elapsed() >= timeout {
            match child.try_wait() {
                Ok(Some(_)) => {
                    return child
                        .wait_with_output()
                        .map_err(|err| format!("failed to wait for {context}: {err}"));
                }
                Ok(None) => {}
                Err(err) => return Err(format!("failed to poll {context}: {err}")),
            }

            let _ = child.kill();
            let output = child
                .wait_with_output()
                .map_err(|err| format!("failed to wait for {context} after timeout: {err}"))?;
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return if stderr.is_empty() {
                Err(format!("{context} timed out after {}s", timeout.as_secs()))
            } else {
                Err(format!(
                    "{context} timed out after {}s: {stderr}",
                    timeout.as_secs()
                ))
            };
        }

        std::thread::sleep(Duration::from_millis(100));
    }
}

fn ensure_git_success(output: &Output, context: &str) -> Result<(), String> {
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        Err(format!("{context} failed with status {}", output.status))
    } else {
        Err(format!(
            "{context} failed with status {}: {stderr}",
            output.status
        ))
    }
}

#[cfg(test)]
#[path = "curated_sync_tests.rs"]
mod tests;
