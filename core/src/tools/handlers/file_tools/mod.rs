//! Structured file-exploration tools: `read_file`, `grep`, `glob`.
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

mod glob;
mod grep;
mod read;

#[cfg(test)]
mod tests;

pub use glob::GlobHandler;
pub use grep::GrepHandler;
pub use read::ReadFileHandler;

use crate::function_tool::FunctionCallError;
use crate::session::turn_context::TurnContext;
use crate::tools::handlers::resolve_tool_environment;
use ody_utils_absolute_path::AbsolutePathBuf;

/// Resolves the local directory a file tool should operate under.
///
/// `path` is the tool's optional `path` argument: absolute paths are taken as
/// given, relative ones are joined onto the environment's cwd. The result is
/// always confined to the environment cwd — a file tool is a workspace
/// exploration tool, not a general filesystem reader.
pub(crate) fn local_search_root(
    turn: &TurnContext,
    environment_id: Option<&str>,
    path: Option<&str>,
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
            "read_file/grep/glob only work on the local filesystem, but environment `{}` is \
             remote. Use shell_command (rg / find / sed) for that environment instead.",
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

    let joined = cwd.join(path);
    confine_to_root(&cwd, joined)
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
