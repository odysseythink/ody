//! Structured file-exploration tools: `read_file`, `grep`, `glob`, `jq`.
//!
//! These replace the raw-shell exploration path (`rg`, `cat`, `find` through
//! `shell_command`), whose unshaped stdout lands in the conversation verbatim.
//! Every result here is capped and paginated before it reaches the model.
//!
//! ## Local-filesystem only, by design
//!
//! The traversal is a local [`ignore::WalkBuilder`] walk, so `.gitignore` is
//! honoured for free and a search costs no sandbox IPC per directory. The price
//! is that these tools cannot see a *remote* turn environment's filesystem —
//! they would silently search the host's disk instead, and answer a question
//! about the wrong machine. [`local_search_root`] therefore refuses to run in a
//! remote environment and points the model at `shell_command`, which does
//! execute remotely. Do not remove that guard without switching the traversal
//! to the `ExecutorFileSystem` trait.

mod edit;
mod glob;
mod grep;
mod jq;
mod read;
mod write;
mod write_edit;

#[cfg(test)]
mod tests;

pub use edit::EditFileHandler;
pub use glob::GlobHandler;
pub use grep::GrepHandler;
pub use jq::JqHandler;
pub use read::ReadFileHandler;
pub use write::WriteFileHandler;

use crate::function_tool::FunctionCallError;
use crate::session::turn_context::TurnContext;
use crate::tools::handlers::resolve_tool_environment;
use ody_utils_absolute_path::AbsolutePathBuf;

/// Whether a file tool path may leave the workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum PathAccessMode {
    /// Relative paths are resolved inside the workspace; absolute paths are also
    /// accepted but must fall inside the workspace.
    #[default]
    WorkspaceRelativeOnly,
    /// Absolute paths may point outside the workspace. Relative paths are still
    /// resolved inside the workspace and are checked against the workspace root.
    AbsoluteOutsideAllowed,
}

/// Resolves the local path a file tool should operate on.
///
/// `path` is the tool's optional `path` argument: absolute paths are taken as
/// given, relative ones are joined onto the environment's cwd. The result is
/// confined to the environment cwd depending on `access_mode`. See
/// [`PathAccessMode`] for the two available policies.
pub(crate) fn local_search_root(
    turn: &TurnContext,
    environment_id: Option<&str>,
    path: Option<&str>,
    access_mode: PathAccessMode,
) -> Result<AbsolutePathBuf, FunctionCallError> {
    let Some(turn_environment) = resolve_tool_environment(turn, environment_id)? else {
        return Err(FunctionCallError::RespondToModel(
            "file tools are unavailable in this session".to_string(),
        ));
    };

    // See the module docs: a local walk of a remote environment would search the
    // wrong machine and quietly return a confident, wrong answer.
    if turn_environment.environment.is_remote() {
        return Err(FunctionCallError::RespondToModel(format!(
            "file tools only work on the local filesystem, but environment `{}` is remote. \
              Use shell_command (rg / find / sed / jq) for that environment instead.",
            turn_environment.environment_id
        )));
    }

    let cwd = turn_environment.cwd().to_abs_path().map_err(|err| {
        FunctionCallError::RespondToModel(format!(
            "environment cwd `{}` is not native to the Ody host: {err}",
            turn_environment.cwd()
        ))
    })?;

    let Some(path) = path else {
        return Ok(cwd);
    };

    resolve_tool_path(&cwd, path, access_mode)
}

/// Resolves and validates a raw tool path against the workspace root.
///
/// * `WorkspaceRelativeOnly` behaves like the original confinement: every path
///   must resolve to a location inside `root`.
/// * `AbsoluteOutsideAllowed` accepts fully absolute paths outside `root` for
///   read-only tools, while keeping relative paths confined to the workspace.
fn resolve_tool_path(
    root: &AbsolutePathBuf,
    raw_path: &str,
    access_mode: PathAccessMode,
) -> Result<AbsolutePathBuf, FunctionCallError> {
    let joined = root.join(raw_path);
    match access_mode {
        PathAccessMode::AbsoluteOutsideAllowed if std::path::Path::new(raw_path).is_absolute() => {
            let normalized = lexically_normalize(joined.as_path());
            check_sensitive_path(&normalized).map_err(FunctionCallError::RespondToModel)?;
            AbsolutePathBuf::from_absolute_path(&normalized).map_err(|err| {
                FunctionCallError::RespondToModel(format!(
                    "path `{}` is not usable: {err}",
                    normalized.display()
                ))
            })
        }
        PathAccessMode::WorkspaceRelativeOnly | PathAccessMode::AbsoluteOutsideAllowed => {
            confine_to_root(root, joined)
        }
    }
}

/// Rejects a resolved path that escapes `root`.
///
/// Normalizes `..` lexically rather than via `canonicalize`, so the check does
/// not depend on the path existing yet and cannot be defeated by a symlink race
/// between the check and the read.
fn confine_to_root(
    root: &AbsolutePathBuf,
    candidate: AbsolutePathBuf,
) -> Result<AbsolutePathBuf, FunctionCallError> {
    let normalized = lexically_normalize(candidate.as_path());
    if !normalized.starts_with(root.as_path()) {
        return Err(FunctionCallError::RespondToModel(format!(
            "path `{}` escapes the working directory `{}`. File tools are confined to the \
              workspace; use shell_command if you genuinely need to read outside it.",
            normalized.display(),
            root.as_path().display()
        )));
    }
    AbsolutePathBuf::from_absolute_path(&normalized).map_err(|err| {
        FunctionCallError::RespondToModel(format!(
            "path `{}` is not usable: {err}",
            normalized.display()
        ))
    })
}

/// Rejects well-known sensitive files that should not be pulled into the model
/// context without explicit escalation.
fn check_sensitive_path(path: &std::path::Path) -> Result<(), String> {
    let lower = path.to_string_lossy().to_lowercase();

    // SSH private keys.
    if lower.contains(".ssh") {
        let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if [
            "id_rsa",
            "id_dsa",
            "id_ecdsa",
            "id_ed25519",
            "id_ed25519_sk",
            "id_xmss",
        ]
        .iter()
        .any(|name| file_name.eq_ignore_ascii_case(name))
        {
            return Err(format!(
                "path `{}` is a private SSH key; use shell_command if you genuinely need to read it",
                path.display()
            ));
        }
    }

    // Unix system credential databases.
    for sensitive in [
        "/etc/shadow",
        "/etc/shadow-",
        "/etc/gshadow",
        "/etc/gshadow-",
        "/etc/master.passwd",
        "/etc/spwd.db",
        "/etc/security/opasswd",
    ] {
        if lower == sensitive {
            return Err(format!(
                "path `{}` is a sensitive system credential file; use shell_command if you genuinely need to read it",
                path.display()
            ));
        }
    }

    // Windows credential databases.
    if lower.contains("system32") && lower.contains("config") {
        let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if ["sam", "security", "system", "software"]
            .iter()
            .any(|name| file_name.eq_ignore_ascii_case(name))
        {
            return Err(format!(
                "path `{}` is a sensitive Windows credential database; use shell_command if you genuinely need to read it",
                path.display()
            ));
        }
    }

    Ok(())
}

fn lexically_normalize(path: &std::path::Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut out = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Renders the trailing pagination notice shared by `grep` and `glob`.
pub(crate) fn pagination_notice(total: usize, shown: usize, offset: usize) -> Option<String> {
    if shown >= total {
        return None;
    }
    let next = offset + shown;
    Some(format!(
        "\n\n[showing {shown} of {total}; use offset={next} to see more]"
    ))
}
