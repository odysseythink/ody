//! Validation engine: runs project scripts (`{package_manager} run <script>`)
//! inside the primary root and reports structured per-check results.
//!
//! Execution is synchronous per request with per-check timeouts and
//! `kill_on_drop`: no background process outlives the call (strategy 8.2).
//! Checks run sequentially in the order given; `overall` is Succeeded only
//! when every run succeeds.

use std::path::Path;
use std::process::Output;
use std::time::Duration;

use ody_app_server_protocol::WorkspaceValidationCheck;
use ody_app_server_protocol::WorkspaceValidationKind;
use ody_app_server_protocol::WorkspaceValidationOverall;
use ody_app_server_protocol::WorkspaceValidationReport;
use ody_app_server_protocol::WorkspaceValidationRun;
use ody_app_server_protocol::WorkspaceValidationStatus;

use crate::workspace_discovery::detect_package_manager;

/// Trailing stdout/stderr bytes kept per run.
pub(crate) const VALIDATE_OUTPUT_TAIL_BYTES: usize = 64 * 1024;
pub(crate) const VALIDATE_DEFAULT_TIMEOUT_MS: i64 = 120_000;
pub(crate) const VALIDATE_MAX_TIMEOUT_MS: i64 = 600_000;
pub(crate) const MAX_VALIDATION_CHECKS: usize = 8;

pub(crate) enum ScriptCommand {
    Spawn(String, Vec<String>),
    Unresolvable(String),
}

/// `{pm} run <script>` when a package.json exists. The package manager
/// follows lockfile priority (E0 rule); npm is the default when a
/// package.json has no lockfile. Without a package.json the script cannot
/// be resolved to a command and the check reports SpawnError with a
/// diagnosable reason.
pub(crate) fn resolve_command(root: &Path, script: &str) -> ScriptCommand {
    if !root.join("package.json").is_file() {
        return ScriptCommand::Unresolvable(format!(
            "root {} has no package.json; cannot run script {script:?}",
            root.display()
        ));
    }
    let pm = detect_package_manager(root).unwrap_or_else(|| "npm".to_owned());
    ScriptCommand::Spawn(pm, vec!["run".to_owned(), script.to_owned()])
}

pub(crate) async fn run_checks(
    root: &Path,
    checks: &[WorkspaceValidationCheck],
    timeout_ms: i64,
) -> WorkspaceValidationReport {
    let mut runs = Vec::with_capacity(checks.len());
    for check in checks {
        runs.push(run_check(root, check, timeout_ms).await);
    }
    let overall = if runs
        .iter()
        .all(|run| run.status == WorkspaceValidationStatus::Succeeded)
    {
        WorkspaceValidationOverall::Succeeded
    } else {
        WorkspaceValidationOverall::Failed
    };
    WorkspaceValidationReport { runs, overall }
}

async fn run_check(
    root: &Path,
    check: &WorkspaceValidationCheck,
    timeout_ms: i64,
) -> WorkspaceValidationRun {
    let started_at_ms = now_ms();
    let started = tokio::time::Instant::now();
    let base = WorkspaceValidationRun {
        kind: check.kind,
        script: check.script.clone(),
        status: WorkspaceValidationStatus::SpawnError,
        exit_code: None,
        stdout_tail: String::new(),
        stderr_tail: String::new(),
        started_at_ms,
        duration_ms: 0,
    };
    let (program, args) = match resolve_command(root, &check.script) {
        ScriptCommand::Spawn(program, args) => (program, args),
        ScriptCommand::Unresolvable(reason) => {
            return WorkspaceValidationRun {
                stderr_tail: reason,
                ..base
            };
        }
    };
    let child = tokio::process::Command::new(&program)
        .args(&args)
        .current_dir(root)
        .kill_on_drop(true)
        .output();
    let output = match tokio::time::timeout(Duration::from_millis(timeout_ms as u64), child).await
    {
        Ok(output) => output,
        Err(_) => {
            // Timeout: the future drop kills the child via kill_on_drop.
            return WorkspaceValidationRun {
                status: WorkspaceValidationStatus::TimedOut,
                duration_ms: timeout_ms,
                ..base
            };
        }
    };
    let output: Output = match output {
        Ok(output) => output,
        Err(err) => {
            return WorkspaceValidationRun {
                stderr_tail: format!("failed to spawn {program}: {err}"),
                duration_ms: started.elapsed().as_millis() as i64,
                ..base
            };
        }
    };
    let status = if output.status.success() {
        WorkspaceValidationStatus::Succeeded
    } else {
        WorkspaceValidationStatus::Failed
    };
    WorkspaceValidationRun {
        status,
        exit_code: output.status.code(),
        stdout_tail: tail(&output.stdout),
        stderr_tail: tail(&output.stderr),
        duration_ms: started.elapsed().as_millis() as i64,
        ..base
    }
}

fn tail(bytes: &[u8]) -> String {
    let start = bytes.len().saturating_sub(VALIDATE_OUTPUT_TAIL_BYTES);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root_with_package_json(lockfile: Option<&str>) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("create root");
        std::fs::write(dir.path().join("package.json"), "{}").expect("write package.json");
        if let Some(lockfile) = lockfile {
            std::fs::write(dir.path().join(lockfile), "").expect("write lockfile");
        }
        dir
    }

    #[test]
    fn resolve_command_requires_package_json() {
        let dir = tempfile::tempdir().unwrap();
        match resolve_command(dir.path(), "build") {
            ScriptCommand::Unresolvable(reason) => {
                assert!(reason.contains("no package.json"), "{reason}");
            }
            ScriptCommand::Spawn(..) => panic!("expected Unresolvable"),
        }
    }

    #[test]
    fn resolve_command_uses_lockfile_priority_pm() {
        let pnpm = temp_root_with_package_json(Some("pnpm-lock.yaml"));
        match resolve_command(pnpm.path(), "build") {
            ScriptCommand::Spawn(pm, args) => {
                assert_eq!(pm, "pnpm");
                assert_eq!(args, vec!["run", "build"]);
            }
            ScriptCommand::Unresolvable(_) => panic!("expected Spawn"),
        }
        let npm = temp_root_with_package_json(None);
        match resolve_command(npm.path(), "build") {
            ScriptCommand::Spawn(pm, _) => assert_eq!(pm, "npm"),
            ScriptCommand::Unresolvable(_) => panic!("expected Spawn"),
        }
    }

    #[test]
    fn tail_keeps_trailing_bytes_only() {
        let big = vec![b'x'; VALIDATE_OUTPUT_TAIL_BYTES + 100];
        let tailed = tail(&big);
        assert_eq!(tailed.len(), VALIDATE_OUTPUT_TAIL_BYTES);
        let small = b"hello";
        assert_eq!(tail(small), "hello");
    }
}
