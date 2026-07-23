use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use crate::function_tool::FunctionCallError;
use crate::sandboxing::SandboxPermissions;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use crate::tools::handlers::apply_granted_turn_permissions;
use crate::tools::handlers::apply_patch::write_permissions_for_paths;
use crate::tools::handlers::file_tools::PathAccessMode;
use crate::tools::handlers::file_tools::local_search_root;
use crate::tools::handlers::resolve_tool_environment;
use ody_protocol::permissions::FileSystemSandboxPolicy;
use ody_protocol::protocol::FileChange;
use ody_sandboxing::policy_transforms::effective_file_system_sandbox_policy;
use ody_sandboxing::policy_transforms::merge_permission_profiles;
use ody_utils_absolute_path::AbsolutePathBuf;

/// Maximum file size for which a unified diff is computed after a write/edit.
pub(crate) const MAX_FILE_SIZE_FOR_DIFF: usize = 1024 * 1024;

/// Resolves the local path a write/edit tool should operate on.
///
/// Write/edit tools are confined to the workspace by default. Absolute paths
/// outside the working directory are rejected unless they point to an
/// Ody-managed skill resource (project skill assets or system skills under
/// $ODY_HOME/skills/.system).
pub(crate) async fn resolve_write_path(
    turn: &TurnContext,
    environment_id: Option<&str>,
    path: &str,
) -> Result<AbsolutePathBuf, FunctionCallError> {
    // Try the normal workspace-relative resolution first.
    if let Ok(resolved) = local_search_root(
        turn,
        environment_id,
        Some(path),
        PathAccessMode::AbsoluteOutsideAllowed,
    ) {
        return Ok(resolved);
    }

    // If that fails, allow absolute paths that target Ody-managed skill
    // directories to resolve outside the workspace.
    let resolved = local_search_root(
        turn,
        environment_id,
        Some(path),
        PathAccessMode::AbsoluteOutsideAllowed,
    )?;
    let cwd = resolve_write_cwd(turn, environment_id).await?;
    if is_ody_managed_skill_path(&resolved, turn, &cwd) {
        Ok(resolved)
    } else {
        // Re-run the original workspace-only resolution to produce the
        // correct "escapes the working directory" error.
        local_search_root(
            turn,
            environment_id,
            Some(path),
            PathAccessMode::AbsoluteOutsideAllowed,
        )
    }
}

/// Returns the absolute cwd of the selected environment, or an error if the
/// tool is unavailable for this session.
pub(crate) async fn resolve_write_cwd(
    turn: &TurnContext,
    environment_id: Option<&str>,
) -> Result<AbsolutePathBuf, FunctionCallError> {
    let Some(turn_environment) = resolve_tool_environment(turn, environment_id)? else {
        return Err(FunctionCallError::RespondToModel(
            "write_file and edit_file are unavailable in this session".to_string(),
        ));
    };
    turn_environment.cwd().to_abs_path().map_err(|err| {
        FunctionCallError::RespondToModel(format!(
            "environment cwd `{}` is not native to the Ody host: {err}",
            turn_environment.cwd()
        ))
    })
}

/// Ensures the requested path is writable under the effective sandbox policy.
///
/// Merges any granted turn/session permissions with additional permissions that
/// would be required for this path, then checks whether the effective policy
/// allows the write. Returns the effective policy on success.
pub(crate) async fn ensure_write_allowed(
    session: &Session,
    turn: &TurnContext,
    environment_id: &str,
    path: &AbsolutePathBuf,
    cwd: &AbsolutePathBuf,
) -> Result<FileSystemSandboxPolicy, FunctionCallError> {
    let granted_permissions = merge_permission_profiles(
        session
            .granted_session_permissions(environment_id)
            .await
            .as_ref(),
        session
            .granted_turn_permissions(environment_id)
            .await
            .as_ref(),
    );
    let base_policy = turn.file_system_sandbox_policy();
    let policy_with_granted =
        effective_file_system_sandbox_policy(&base_policy, granted_permissions.as_ref());

    let effective = apply_granted_turn_permissions(
        session,
        environment_id,
        cwd.as_path(),
        SandboxPermissions::UseDefault,
        write_permissions_for_paths(&[path.clone()], &policy_with_granted, cwd),
    )
    .await;

    let final_policy = effective_file_system_sandbox_policy(
        &base_policy,
        effective.additional_permissions.as_ref(),
    );

    if !final_policy.can_write_path_with_cwd(path.as_path(), cwd.as_path()) {
        if is_ody_managed_skill_path(path, turn, cwd) {
            return Ok(final_policy);
        }
        return Err(FunctionCallError::RespondToModel(format!(
            "write to `{}` is not permitted by the current sandbox policy. Use `apply_patch` or \
             `shell_command` if you need to write outside the allowed paths.",
            path.as_path().display()
        )));
    }

    Ok(final_policy)
}

/// Checks whether `path` is an Ody-managed skill resource that should be writable
/// even when it falls outside the workspace root.
///
/// This covers:
/// - project-embedded / builtin skill assets under `<cwd>/skills/src/assets`
/// - user-scope system skills installed under `<ody_home>/skills/.system`
fn is_ody_managed_skill_path(
    path: &AbsolutePathBuf,
    turn: &TurnContext,
    cwd: &AbsolutePathBuf,
) -> bool {
    // 1. Project-local skill assets (embedded or builtin).
    let project_skills_root = cwd.join("skills").join("src").join("assets");
    if path.starts_with(project_skills_root.as_path()) {
        return true;
    }

    // 2. User-level system skills installed under $ODY_HOME/skills/.system.
    let system_skills_root = turn.config.ody_home.join("skills").join(".system");
    if path.starts_with(system_skills_root.as_path()) {
        return true;
    }

    false
}

/// Writes `contents` to `path` atomically using a temporary file and rename.
pub(crate) async fn atomic_write(
    path: &AbsolutePathBuf,
    contents: &[u8],
) -> Result<(), FunctionCallError> {
    let parent = path.parent().ok_or_else(|| {
        FunctionCallError::RespondToModel(format!(
            "path `{}` has no parent directory",
            path.as_path().display()
        ))
    })?;

    tokio::fs::create_dir_all(parent.as_path())
        .await
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!(
                "failed to create parent directory for `{}`: {err}",
                path.as_path().display()
            ))
        })?;

    let temp_suffix = format!(".ody-write.{}.tmp", uuid::Uuid::new_v4());
    let temp_path = path.with_extension(&temp_suffix);

    let write_result = async {
        tokio::fs::write(&temp_path, contents).await?;
        tokio::fs::rename(&temp_path, path.as_path()).await?;
        Ok::<(), std::io::Error>(())
    }
    .await;

    if write_result.is_err() {
        let _ = tokio::fs::remove_file(&temp_path).await;
    }

    write_result.map_err(|err| {
        FunctionCallError::RespondToModel(format!(
            "failed to write `{}`: {err}",
            path.as_path().display()
        ))
    })
}

/// Computes a unified diff between `old` and `new` for `path`.
pub(crate) fn compute_unified_diff(old: &str, new: &str, path: &Path) -> String {
    let diff = similar::TextDiff::from_lines(old, new);
    diff.unified_diff()
        .context_radius(3)
        .header(
            &format!("a/{}", path.display()),
            &format!("b/{}", path.display()),
        )
        .to_string()
}

/// Builds a protocol [`FileChange`] for a write/edit operation.
///
/// If the file did not previously exist, the change is reported as an `Add`.
/// Otherwise an `Update` with a unified diff is produced, capped to
/// [`MAX_FILE_SIZE_FOR_DIFF`] to avoid stalling on huge files.
pub(crate) fn file_change_for_write(
    path: &Path,
    old_content: Option<&str>,
    new_content: &str,
) -> FileChange {
    match old_content {
        None => FileChange::Add {
            content: new_content.to_string(),
        },
        Some(old) => {
            let unified_diff = if old.len() > MAX_FILE_SIZE_FOR_DIFF
                || new_content.len() > MAX_FILE_SIZE_FOR_DIFF
            {
                format!(
                    "diff --git a/{path} b/{path}\n\
                     # File content exceeds diff size limit; changes not shown\n",
                    path = path.display()
                )
            } else {
                compute_unified_diff(old, new_content, path)
            };
            FileChange::Update {
                unified_diff,
                move_path: None,
            }
        }
    }
}

/// Builds a `HashMap` containing a single file change for event emission.
pub(crate) fn single_file_change(
    path: PathBuf,
    change: FileChange,
) -> HashMap<PathBuf, FileChange> {
    HashMap::from([(path, change)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use ody_utils_absolute_path::AbsolutePathBuf;
    use std::path::Path;

    #[test]
    fn compute_unified_diff_produces_valid_unified_diff() {
        let diff = compute_unified_diff("foo\n", "bar\n", Path::new("src.txt"));
        assert!(
            diff.contains("--- a/src.txt"),
            "diff must contain old header: {diff}"
        );
        assert!(
            diff.contains("+++ b/src.txt"),
            "diff must contain new header: {diff}"
        );
        assert!(
            diff.contains("-foo"),
            "diff must contain removed line: {diff}"
        );
        assert!(
            diff.contains("+bar"),
            "diff must contain added line: {diff}"
        );
    }

    #[test]
    fn file_change_for_new_file_is_add() {
        let change = file_change_for_write(Path::new("new.txt"), None, "hello\n");
        assert!(
            matches!(change, FileChange::Add { ref content } if content == "hello\n"),
            "expected Add change for a new file: {change:?}"
        );
    }

    #[test]
    fn file_change_for_existing_file_is_update_with_diff() {
        let change = file_change_for_write(Path::new("existing.txt"), Some("old\n"), "new\n");
        match change {
            FileChange::Update {
                unified_diff,
                move_path,
            } => {
                assert!(
                    unified_diff.contains("-old"),
                    "diff must show old content: {unified_diff}"
                );
                assert!(
                    unified_diff.contains("+new"),
                    "diff must show new content: {unified_diff}"
                );
                assert!(move_path.is_none(), "move_path must be None");
            }
            other => panic!("expected Update change for existing file: {other:?}"),
        }
    }

    #[test]
    fn file_change_skips_diff_for_oversized_content() {
        let oversized = "x".repeat(MAX_FILE_SIZE_FOR_DIFF + 1);
        let change = file_change_for_write(Path::new("huge.txt"), Some(&oversized), "y\n");
        match change {
            FileChange::Update { unified_diff, .. } => {
                assert!(
                    unified_diff.contains("exceeds diff size limit"),
                    "expected placeholder for oversized diff: {unified_diff}"
                );
            }
            other => panic!("expected Update change: {other:?}"),
        }
    }

    #[tokio::test]
    async fn atomic_write_creates_file_and_parent_dirs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = AbsolutePathBuf::from_absolute_path(dir.path().join("nested/file.txt"))
            .expect("absolute path");
        atomic_write(&path, b"hello\n").await.expect("write");
        assert_eq!(
            std::fs::read_to_string(path.as_path()).expect("read"),
            "hello\n"
        );
    }

    #[tokio::test]
    async fn atomic_write_overwrites_existing_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = AbsolutePathBuf::from_absolute_path(dir.path().join("file.txt"))
            .expect("absolute path");
        std::fs::write(path.as_path(), "old\n").unwrap();
        atomic_write(&path, b"new\n").await.expect("write");
        assert_eq!(
            std::fs::read_to_string(path.as_path()).expect("read"),
            "new\n"
        );
    }
}
