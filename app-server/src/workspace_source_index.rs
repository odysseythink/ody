//! Zero-AST SourceRef indexing over bound workspace roots.
//!
//! Symbol/range localization is heuristic (export declaration line scan +
//! brace balancing) and degrades to file-level (`range: None`, whole-file
//! content hash) whenever the heuristic fails. The index is computed fresh
//! on every call; nothing is persisted. Hashing is deterministic per
//! content, so repeated indexes of unchanged files agree.

use std::fs;
use std::path::Path;
use std::path::PathBuf;

use ody_app_server_protocol::WorkspaceProjectRef;
use ody_app_server_protocol::WorkspaceSourceArtifact;
use ody_app_server_protocol::WorkspaceSourceIndex;
use ody_app_server_protocol::WorkspaceSourceKind;
use ody_app_server_protocol::WorkspaceSourceRange;
use ody_app_server_protocol::WorkspaceSourceRef;
use sha2::Digest;
use sha2::Sha256;

use crate::workspace_discovery::classify_source;
use crate::workspace_discovery::detect_framework;
use crate::workspace_discovery::MAX_DEPTH;
use crate::workspace_discovery::MAX_FILES_VISITED;
use crate::workspace_discovery::MAX_SOURCES_PER_KIND;
use crate::workspace_discovery::SKIP_DIRS;

/// Source files larger than this are skipped with a diagnosable error entry.
const MAX_SOURCE_FILE_BYTES: u64 = 1024 * 1024;

pub(crate) async fn index_project(project: &WorkspaceProjectRef) -> WorkspaceSourceIndex {
    let mut artifacts = Vec::new();
    let mut refs = Vec::new();
    let mut truncated = false;
    let mut errors = Vec::new();
    for (root_index, root) in project.roots.iter().enumerate() {
        let root_path = Path::new(&root.path);
        if !root_path.is_dir() {
            errors.push(format!("root {} is not a readable directory", root.path));
            continue;
        }
        let framework = detect_framework(root_path);
        index_root(
            &root.path,
            root_index,
            framework,
            &mut artifacts,
            &mut refs,
            &mut truncated,
            &mut errors,
        );
    }
    WorkspaceSourceIndex {
        project_id: project.id.clone(),
        artifacts,
        refs,
        indexed_at_ms: now_ms(),
        truncated,
        errors,
    }
}

#[allow(clippy::too_many_arguments)]
fn index_root(
    root_path: &str,
    root_index: usize,
    framework: Option<String>,
    artifacts: &mut Vec<WorkspaceSourceArtifact>,
    refs: &mut Vec<WorkspaceSourceRef>,
    truncated: &mut bool,
    errors: &mut Vec<String>,
) {
    let root = Path::new(root_path);
    // Per-kind counters indexed by kind slot (Page/Component/Route).
    let mut counts = [0usize; 3];
    let mut stack: Vec<(PathBuf, u32)> = vec![(root.to_path_buf(), 0)];
    let mut visited: u64 = 0;
    while let Some((dir, depth)) = stack.pop() {
        if depth > MAX_DEPTH {
            *truncated = true;
            continue;
        }
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) => {
                errors.push(format!("read {}: {err}", dir.display()));
                continue;
            }
        };
        for entry in entries.flatten() {
            // DirEntry::file_type does not follow symlinks: escaping
            // symlinks are never traversed or read.
            let Ok(file_type) = entry.file_type() else { continue };
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if file_type.is_symlink() {
                continue;
            }
            let path = dir.join(name.as_ref());
            if file_type.is_dir() {
                if SKIP_DIRS.contains(&name.as_ref()) {
                    continue;
                }
                stack.push((path, depth + 1));
            } else if file_type.is_file() {
                visited += 1;
                if visited > MAX_FILES_VISITED {
                    *truncated = true;
                    break;
                }
                let Ok(relative) = path.strip_prefix(root) else {
                    continue;
                };
                let Some(source_entry) = classify_source(relative) else {
                    continue;
                };
                let kind = source_entry.kind;
                let slot = match kind {
                    WorkspaceSourceKind::Page => 0,
                    WorkspaceSourceKind::Component => 1,
                    WorkspaceSourceKind::Route => 2,
                };
                if counts[slot] >= MAX_SOURCES_PER_KIND {
                    *truncated = true;
                    continue;
                }
                counts[slot] += 1;
                let Ok(meta) = fs::metadata(&path) else {
                    continue;
                };
                if meta.len() > MAX_SOURCE_FILE_BYTES {
                    errors.push(format!("skip {}: file exceeds 1 MiB", path.display()));
                    continue;
                }
                let Ok(content) = fs::read_to_string(&path) else {
                    continue; // binary or non-UTF8
                };
                let file_hash = sha256_hex(content.as_bytes());
                let (symbol, range) = extract_symbol_and_range(&content, &source_entry.name);
                let content_hash = range
                    .as_ref()
                    .map(|r| range_hash(&content, r))
                    .unwrap_or_else(|| file_hash.clone());
                let rel_path = relative.to_string_lossy().replace('\\', "/");
                let kind_label = match kind {
                    WorkspaceSourceKind::Page => "page",
                    WorkspaceSourceKind::Component => "component",
                    WorkspaceSourceKind::Route => "route",
                };
                let artifact_id = format!("{kind_label}:{root_index}:{rel_path}");
                let route_path = source_entry.route_path.clone();
                artifacts.push(WorkspaceSourceArtifact {
                    id: artifact_id.clone(),
                    kind,
                    name: source_entry.name.clone(),
                    path: rel_path.clone(),
                    route_path: route_path.clone(),
                    framework: framework.clone(),
                });
                refs.push(WorkspaceSourceRef {
                    artifact_id,
                    root_path: root_path.to_owned(),
                    file_path: rel_path,
                    range,
                    symbol,
                    route_path,
                    content_hash,
                    file_hash,
                });
            }
        }
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn range_hash(content: &str, range: &WorkspaceSourceRange) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let start = range.start_line as usize;
    let end = (range.end_line as usize).min(lines.len());
    let slice = lines.get(start.saturating_sub(1)..end).unwrap_or(&[]);
    sha256_hex(slice.join("\n").as_bytes())
}

/// Heuristic export localization. Deterministic per content; every failure
/// mode degrades to (file stem, None) = file-level ref.
fn extract_symbol_and_range(
    content: &str,
    file_stem: &str,
) -> (String, Option<WorkspaceSourceRange>) {
    // SFCs (Vue/Svelte) stay file-level in v1: script blocks have no
    // reliable brace semantics for range purposes.
    if content.contains("<script") {
        return (file_stem.to_owned(), None);
    }
    let lines: Vec<&str> = content.lines().collect();
    // 1. `export default function/class Name` (also `export default async function Name`).
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("export default ") {
            let rest = rest.strip_prefix("async ").unwrap_or(rest);
            if let Some(name) = rest
                .strip_prefix("function ")
                .or_else(|| rest.strip_prefix("class "))
                .and_then(identifier_name)
            {
                return (name, brace_range(&lines, i));
            }
            // 2. `export default Identifier;` → locate the local declaration.
            if let Some(name) = identifier_name(rest) {
                let range =
                    find_declaration(&lines, &name).and_then(|decl| brace_range(&lines, decl));
                return (name, range);
            }
        }
    }
    // 3. Named exports; prefer the one matching the file stem.
    let mut first_named: Option<(String, usize)> = None;
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("export ") {
            continue;
        }
        let rest = trimmed.strip_prefix("export ").unwrap_or(trimmed);
        let rest = rest.strip_prefix("async ").unwrap_or(rest);
        for prefix in ["function ", "class ", "const "] {
            if let Some(candidate) = rest.strip_prefix(prefix).and_then(identifier_name) {
                if candidate == file_stem {
                    return (candidate, brace_range(&lines, i));
                }
                if first_named.is_none() {
                    first_named = Some((candidate, i));
                }
            }
        }
    }
    if let Some((name, i)) = first_named {
        return (name, brace_range(&lines, i));
    }
    (file_stem.to_owned(), None)
}

fn identifier_name(s: &str) -> Option<String> {
    let mut chars = s.trim_start().chars();
    let first = chars.next()?;
    if !(first.is_ascii_alphabetic() || first == '_' || first == '$') {
        return None;
    }
    let mut name = String::from(first);
    for c in chars {
        if c.is_ascii_alphanumeric() || c == '_' || c == '$' {
            name.push(c);
        } else {
            break;
        }
    }
    Some(name)
}

fn find_declaration(lines: &[&str], name: &str) -> Option<usize> {
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        let rest = trimmed.strip_prefix("export ").unwrap_or(trimmed);
        let rest = rest.strip_prefix("async ").unwrap_or(rest);
        for prefix in ["function ", "class ", "const "] {
            if let Some(candidate) = rest.strip_prefix(prefix).and_then(identifier_name) {
                if candidate == name {
                    return Some(i);
                }
            }
        }
    }
    None
}

/// Brace-balanced block range starting at `start`, or None when unbalanced.
/// Braces inside strings/comments can skew the end line; the hash stays
/// self-consistent because the same heuristic runs on every index.
fn brace_range(lines: &[&str], start: usize) -> Option<WorkspaceSourceRange> {
    let mut depth: i32 = 0;
    let mut opened = false;
    for (i, line) in lines.iter().enumerate().skip(start) {
        for c in line.chars() {
            match c {
                '{' => {
                    depth += 1;
                    opened = true;
                }
                '}' => {
                    depth -= 1;
                    if opened && depth == 0 {
                        return Some(WorkspaceSourceRange {
                            start_line: (start + 1) as u32,
                            end_line: (i + 1) as u32,
                        });
                    }
                }
                _ => {}
            }
        }
    }
    None
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

    const HOME_PAGE: &str = "export default function HomePage() {\n  return <main>home</main>;\n}\n";

    #[test]
    fn extracts_default_function_symbol_and_range() {
        let (symbol, range) = extract_symbol_and_range(HOME_PAGE, "HomePage");
        assert_eq!(symbol, "HomePage");
        let range = range.expect("balanced block gives a range");
        assert_eq!((range.start_line, range.end_line), (1, 3));
    }

    #[test]
    fn export_default_identifier_resolves_declaration_range() {
        let content = "export function Button() {\n  return <button>ok</button>;\n}\nexport default Button;\n";
        let (symbol, range) = extract_symbol_and_range(content, "Button");
        assert_eq!(symbol, "Button");
        let range = range.expect("declaration line found");
        assert_eq!((range.start_line, range.end_line), (1, 3));
    }

    #[test]
    fn named_export_matching_stem_is_preferred() {
        let content = "export function helper() {\n  return 1;\n}\nexport function Button() {\n  return 2;\n}\n";
        let (symbol, range) = extract_symbol_and_range(content, "Button");
        assert_eq!(symbol, "Button");
        assert_eq!(range.expect("range").start_line, 4);
    }

    #[test]
    fn module_exports_falls_back_to_file_level() {
        let content = "module.exports = { foo: 1 };\n";
        let (symbol, range) = extract_symbol_and_range(content, "util");
        assert_eq!(symbol, "util");
        assert!(range.is_none());
    }

    #[test]
    fn vue_sfc_falls_back_to_file_level() {
        let content = "<template><div/></template>\n<script setup>\nconst x = 1;\n</script>\n";
        let (symbol, range) = extract_symbol_and_range(content, "HomeView");
        assert_eq!(symbol, "HomeView");
        assert!(range.is_none());
    }

    #[test]
    fn unbalanced_braces_degrade_to_none() {
        let content = "export function Broken() {\n  return 1;\n";
        let (_, range) = extract_symbol_and_range(content, "Broken");
        assert!(range.is_none());
    }

    #[test]
    fn range_hash_is_deterministic_and_differs_from_file() {
        let content = "export default function HomePage() {\n  return <main>home</main>;\n}\nexport const x = 1;\n";
        let file_hash = sha256_hex(content.as_bytes());
        let range = WorkspaceSourceRange {
            start_line: 1,
            end_line: 3,
        };
        let ranged = range_hash(content, &range);
        assert_ne!(ranged, file_hash);
        assert_eq!(ranged, range_hash(content, &range));
        assert_eq!(ranged.len(), 64);
    }
}
