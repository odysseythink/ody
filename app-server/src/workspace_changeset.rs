//! ChangeSet engine: path-safety validation, content hashing, unified diff
//! rendering, and the file operations behind changeset apply/restore.
//!
//! Write discipline: every target path is resolved once at create time
//! (canonical deepest existing ancestor must stay inside the root; writing
//! through symlinks is rejected) and the resolved absolute path is stored
//! with the changeset. apply/restore reuse the stored path after
//! re-validating it, never re-interpreting the client-supplied string.
//! All writes are tmp + rename; deletes are single-file only.

use std::collections::BTreeSet;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;

use ody_app_server_protocol::JSONRPCErrorError;
use ody_app_server_protocol::WorkspaceFileChange;
use ody_app_server_protocol::WorkspaceFileChangeKind;
use ody_app_server_protocol::WorkspaceProjectRef;
use sha2::Digest;
use sha2::Sha256;

use crate::error_code::invalid_params;

/// Max files per changeset.
pub(crate) const MAX_CHANGESET_FILES: usize = 50;
/// Per-file cap for declared content and captured base content.
pub(crate) const MAX_CHANGE_FILE_BYTES: u64 = 1024 * 1024;
/// Total declared-content cap per changeset.
pub(crate) const MAX_CHANGESET_TOTAL_BYTES: u64 = 8 * 1024 * 1024;

/// A change whose path has been validated and resolved to an absolute
/// target inside its root.
#[derive(Debug, Clone)]
pub(crate) struct PreparedChange {
    pub key: String,
    pub root_index: usize,
    pub root_path: PathBuf,
    pub relative: String,
    pub absolute: PathBuf,
    pub kind: WorkspaceFileChangeKind,
    pub base_hash: Option<String>,
    pub content: Option<String>,
    /// Captured from disk at create time; drives restore and the diff.
    pub base_content: Option<String>,
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// Normalize a root-relative path: reject absolute paths, `.`/`..`
/// segments, empty segments, and NULs. Returns `/`-joined segments.
pub(crate) fn normalize_relative(path: &str) -> Result<String, JSONRPCErrorError> {
    if path.is_empty() || path.starts_with('/') || path.starts_with('\\') || path.contains('\0') {
        return Err(invalid_params(format!(
            "change path {path:?} must be a non-empty root-relative path"
        )));
    }
    let mut segments = Vec::new();
    for segment in path.split(['/', '\\']) {
        match segment {
            "" | "." => continue,
            ".." => {
                return Err(invalid_params(format!(
                    "change path {path:?} must not escape the workspace root"
                )))
            }
            other => segments.push(other),
        }
    }
    if segments.is_empty() {
        return Err(invalid_params(format!(
            "change path {path:?} must name a file inside the workspace root"
        )));
    }
    Ok(segments.join("/"))
}

/// Resolve the normalized relative path to an absolute target guaranteed
/// inside `root` (itself canonicalized at bind time). Canonicalizes the
/// deepest existing ancestor; any symlink traversal that escapes the root
/// is rejected. Existing symlink targets are never written through.
pub(crate) fn resolve_target(
    root: &Path,
    normalized: &str,
) -> Result<PathBuf, JSONRPCErrorError> {
    if !root.is_dir() {
        return Err(invalid_params(format!(
            "root {} is not a readable directory",
            root.display()
        )));
    }
    let candidate = root.join(normalized);
    // Reject writing through an existing symlink target outright.
    if let Ok(meta) = fs::symlink_metadata(&candidate) {
        if meta.file_type().is_symlink() {
            return Err(invalid_params(format!(
                "change path {normalized:?} is a symbolic link; refusing to write through it"
            )));
        }
    }
    let mut probe = candidate.clone();
    let mut missing: Vec<std::ffi::OsString> = Vec::new();
    loop {
        match fs::canonicalize(&probe) {
            Ok(resolved) => {
                if !resolved.starts_with(root) {
                    return Err(invalid_params(format!(
                        "change path {normalized:?} resolves outside the workspace root ({})",
                        resolved.display()
                    )));
                }
                let mut target = resolved;
                for component in missing.iter().rev() {
                    target.push(component);
                }
                return Ok(target);
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {
                match probe.file_name() {
                    Some(name) => missing.push(name.to_os_string()),
                    None => {
                        return Err(invalid_params(format!(
                            "change path {normalized:?} cannot be resolved under root {}",
                            root.display()
                        )))
                    }
                }
                probe = match probe.parent() {
                    Some(parent) => parent.to_path_buf(),
                    None => {
                        return Err(invalid_params(format!(
                            "change path {normalized:?} cannot be resolved under root {}",
                            root.display()
                        )))
                    }
                };
            }
            Err(err) => {
                return Err(invalid_params(format!(
                    "change path {normalized:?} cannot be canonicalized: {err}"
                )))
            }
        }
    }
}

/// Validate limits, paths, and (for Update/Delete) that the declared
/// base_hash matches disk. Captures base content for the diff and restore.
pub(crate) fn prepare_changes(
    project: &WorkspaceProjectRef,
    changes: &[WorkspaceFileChange],
) -> Result<Vec<PreparedChange>, JSONRPCErrorError> {
    if changes.is_empty() {
        return Err(invalid_params("changeset must contain at least one change"));
    }
    if changes.len() > MAX_CHANGESET_FILES {
        return Err(invalid_params(format!(
            "changeset has {} changes; max is {MAX_CHANGESET_FILES}",
            changes.len()
        )));
    }
    let mut total: u64 = 0;
    let mut prepared = Vec::with_capacity(changes.len());
    let mut seen = BTreeSet::new();
    for change in changes {
        let root = project
            .roots
            .get(change.root_index as usize)
            .ok_or_else(|| invalid_params(format!("unknown root index {}", change.root_index)))?;
        let normalized = normalize_relative(&change.path)?;
        let key = format!("{}:{normalized}", change.root_index);
        if !seen.insert(key.clone()) {
            return Err(invalid_params(format!("duplicate change path {key:?}")));
        }
        let absolute = resolve_target(Path::new(&root.path), &normalized)?;
        match change.kind {
            WorkspaceFileChangeKind::Add => {
                let content = change.content.clone().ok_or_else(|| {
                    invalid_params(format!("add change {key} requires content"))
                })?;
                if content.len() as u64 > MAX_CHANGE_FILE_BYTES {
                    return Err(invalid_params(format!(
                        "change {key} content exceeds {MAX_CHANGE_FILE_BYTES} bytes"
                    )));
                }
                if absolute.exists() {
                    return Err(invalid_params(format!(
                        "add change {key} target already exists"
                    )));
                }
                total += content.len() as u64;
                prepared.push(PreparedChange {
                    key,
                    root_index: change.root_index as usize,
                    root_path: PathBuf::from(&root.path),
                    relative: normalized,
                    absolute,
                    kind: change.kind,
                    base_hash: None,
                    content: Some(content),
                    base_content: None,
                });
            }
            WorkspaceFileChangeKind::Update | WorkspaceFileChangeKind::Delete => {
                let declared = change.base_hash.clone().ok_or_else(|| {
                    invalid_params(format!("{:?} change {key} requires baseHash", change.kind))
                })?;
                let content = change.content.clone();
                if change.kind == WorkspaceFileChangeKind::Update {
                    let content = content.clone().ok_or_else(|| {
                        invalid_params(format!("update change {key} requires content"))
                    })?;
                    if content.len() as u64 > MAX_CHANGE_FILE_BYTES {
                        return Err(invalid_params(format!(
                            "change {key} content exceeds {MAX_CHANGE_FILE_BYTES} bytes"
                        )));
                    }
                    total += content.len() as u64;
                }
                let bytes = fs::read(&absolute).map_err(|err| {
                    invalid_params(format!("change {key} cannot read current file: {err}"))
                })?;
                if bytes.len() as u64 > MAX_CHANGE_FILE_BYTES {
                    return Err(invalid_params(format!(
                        "change {key} file exceeds {MAX_CHANGE_FILE_BYTES} bytes"
                    )));
                }
                let actual = sha256_hex(&bytes);
                if actual != declared {
                    return Err(invalid_params(format!(
                        "change {key} baseHash mismatch: file changed on disk; re-index and recreate the changeset"
                    )));
                }
                let base_content = String::from_utf8(bytes).map_err(|_| {
                    invalid_params(format!("change {key} target is not UTF-8 text"))
                })?;
                prepared.push(PreparedChange {
                    key,
                    root_index: change.root_index as usize,
                    root_path: PathBuf::from(&root.path),
                    relative: normalized,
                    absolute,
                    kind: change.kind,
                    base_hash: Some(declared),
                    content,
                    base_content: Some(base_content),
                });
            }
        }
    }
    if total > MAX_CHANGESET_TOTAL_BYTES {
        return Err(invalid_params(format!(
            "changeset content exceeds {MAX_CHANGESET_TOTAL_BYTES} bytes total"
        )));
    }
    Ok(prepared)
}

/// Unified diff (git-style headers + similar hunks) for one file.
pub(crate) fn render_file_diff(relative: &str, base: Option<&str>, new: Option<&str>) -> String {
    let mut out = format!("diff --git a/{relative} b/{relative}\n");
    let old_text = base.unwrap_or("");
    let new_text = new.unwrap_or("");
    match (base, new) {
        (None, Some(_)) => {
            out.push_str("new file mode 100644\n");
            out.push_str(&format!("--- /dev/null\n+++ b/{relative}\n"));
        }
        (Some(_), None) => {
            out.push_str("deleted file mode 100644\n");
            out.push_str(&format!("--- a/{relative}\n+++ /dev/null\n"));
        }
        _ => {
            out.push_str(&format!("--- a/{relative}\n+++ b/{relative}\n"));
        }
    }
    let diff = similar::TextDiff::from_lines(old_text, new_text);
    out.push_str(
        &diff
            .unified_diff()
            .context_radius(3)
            .to_string(),
    );
    out
}

pub(crate) fn render_changeset_diff(prepared: &[PreparedChange]) -> String {
    prepared
        .iter()
        .map(|change| {
            render_file_diff(
                &change.relative,
                change.base_content.as_deref(),
                change.content.as_deref(),
            )
        })
        .collect::<Vec<_>>()
        .join("")
}

/// Pre-apply verification shared by apply (against base_hash) and idempotent
/// re-apply after a partial failure (base_hash OR final content accepted).
pub(crate) fn verify_apply_state(
    change: &PreparedChange,
    accept_applied: bool,
) -> Result<(), String> {
    match change.kind {
        WorkspaceFileChangeKind::Add => {
            if change.absolute.exists() {
                let current = fs::read(&change.absolute).map_err(|e| e.to_string())?;
                let expected = change.content.as_deref().unwrap_or_default();
                if accept_applied && current == expected.as_bytes() {
                    return Ok(());
                }
                return Err(format!("add target {} already exists", change.key));
            }
            Ok(())
        }
        WorkspaceFileChangeKind::Update | WorkspaceFileChangeKind::Delete => {
            let current = fs::read(&change.absolute).map_err(|e| e.to_string())?;
            let actual = sha256_hex(&current);
            let base = change.base_hash.as_deref().unwrap_or("");
            if actual == base {
                return Ok(());
            }
            if accept_applied {
                if let Some(content) = &change.content {
                    if actual == sha256_hex(content.as_bytes()) {
                        return Ok(());
                    }
                }
            }
            Err(format!(
                "file {} changed on disk (hash mismatch); re-index and recreate the changeset",
                change.key
            ))
        }
    }
}

/// Restore verification: files must be exactly as apply left them.
pub(crate) fn verify_restore_state(change: &PreparedChange) -> Result<(), String> {
    match change.kind {
        WorkspaceFileChangeKind::Add => {
            if !change.absolute.exists() {
                return Err(format!(
                    "added file {} is missing; cannot restore safely",
                    change.key
                ));
            }
            let current = fs::read(&change.absolute).map_err(|e| e.to_string())?;
            let applied = change
                .content
                .as_deref()
                .map(sha256_hex_of_str)
                .unwrap_or_default();
            if sha256_hex(&current) == applied {
                Ok(())
            } else {
                Err(format!(
                    "added file {} was modified after apply",
                    change.key
                ))
            }
        }
        WorkspaceFileChangeKind::Update | WorkspaceFileChangeKind::Delete => {
            if !change.absolute.exists() {
                return Err(format!(
                    "file {} is missing; cannot restore safely",
                    change.key
                ));
            }
            let current = fs::read(&change.absolute).map_err(|e| e.to_string())?;
            let actual = sha256_hex(&current);
            let applied = change
                .content
                .as_deref()
                .map(sha256_hex_of_str)
                .unwrap_or_default();
            // Delete changes have no content; existence is the check above.
            if change.kind == WorkspaceFileChangeKind::Delete || actual == applied {
                Ok(())
            } else {
                Err(format!(
                    "file {} changed after apply; refusing to overwrite user edits",
                    change.key
                ))
            }
        }
    }
}

fn sha256_hex_of_str(s: &str) -> String {
    sha256_hex(s.as_bytes())
}

/// Atomic tmp + rename write; creates parent directories for new files.
pub(crate) fn atomic_write(path: &Path, content: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("ody-tmp-{}", std::process::id()));
    fs::write(&tmp, content)?;
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> PathBuf {
        tempfile::tempdir()
            .expect("create root")
            .keep()
            .canonicalize()
            .expect("canonicalize")
    }

    #[test]
    fn normalize_rejects_escape_and_absolute() {
        assert!(normalize_relative("../escape.txt").is_err());
        assert!(normalize_relative("a/../../escape.txt").is_err());
        assert!(normalize_relative("/etc/passwd").is_err());
        assert!(normalize_relative("").is_err());
        assert_eq!(normalize_relative("a//b/./c.tsx").unwrap(), "a/b/c.tsx");
    }

    #[cfg(unix)]
    #[test]
    fn resolve_target_rejects_symlink_escape() {
        let root = temp_root();
        let outside = temp_root();
        std::os::unix::fs::symlink(&outside, root.join("link")).expect("create symlink");
        let err = resolve_target(&root, "link/evil.txt").expect_err("escape rejected");
        assert!(err.message.contains("outside"), "{}", err.message);
    }

    #[cfg(unix)]
    #[test]
    fn resolve_target_rejects_existing_symlink_file() {
        let root = temp_root();
        let target = temp_root().join("real.txt");
        fs::write(&target, "real").expect("write target");
        std::os::unix::fs::symlink(&target, root.join("linked.tsx")).expect("create symlink");
        let err = resolve_target(&root, "linked.tsx").expect_err("symlink write rejected");
        assert!(err.message.contains("symbolic link"), "{}", err.message);
    }

    #[cfg(unix)]
    #[test]
    fn resolve_target_accepts_missing_nested_path_inside_root() {
        let root = temp_root();
        let resolved = resolve_target(&root, "src/pages/NewPage.tsx").expect("resolve");
        assert!(resolved.starts_with(&root));
        assert!(resolved.ends_with("src/pages/NewPage.tsx"));
    }

    #[test]
    fn render_file_diff_add_update_delete() {
        let add = render_file_diff("src/New.tsx", None, Some("line1\nline2\n"));
        assert!(add.contains("new file mode 100644"));
        assert!(add.contains("--- /dev/null"));
        assert!(add.contains("+line1"));

        let update = render_file_diff("a.ts", Some("old\n"), Some("new\n"));
        assert!(update.contains("--- a/a.ts"));
        assert!(update.contains("-old"));
        assert!(update.contains("+new"));

        let delete = render_file_diff("a.ts", Some("gone\n"), None);
        assert!(delete.contains("deleted file mode 100644"));
        assert!(delete.contains("-gone"));
    }

    #[test]
    fn atomic_write_creates_parents_and_replaces() {
        let root = temp_root();
        let path = root.join("nested/dir/file.txt");
        atomic_write(&path, "v1").expect("write");
        assert_eq!(fs::read_to_string(&path).unwrap(), "v1");
        atomic_write(&path, "v2").expect("rewrite");
        assert_eq!(fs::read_to_string(&path).unwrap(), "v2");
    }
}
