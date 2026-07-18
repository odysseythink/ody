use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct PartsManifest {
    pub rows: Vec<ManifestRow>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ManifestRow {
    pub number: usize,
    pub file: String,
    pub scope: String,
    pub status: RowStatus,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RowStatus {
    Pending,
    Done,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ManifestParseResult {
    pub manifest: Option<PartsManifest>,
    pub warning: Option<String>,
}

pub fn parse_parts_manifest(content: &str) -> ManifestParseResult {
    let mut result = ManifestParseResult {
        manifest: None,
        warning: None,
    };

    let Some(heading_pos) = content.find("## Parts") else {
        return result;
    };

    let remainder = &content[heading_pos..];
    let section_end = remainder
        .find("\n## ")
        .map(|i| i + 1)
        .unwrap_or(remainder.len());
    let section = &remainder[..section_end];

    let lines: Vec<&str> = section.lines().collect();
    let mut table_start = None;
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('|') && i + 1 < lines.len() && lines[i + 1].contains("---") {
            table_start = Some(i);
            break;
        }
    }

    let Some(table_start) = table_start else {
        result.warning = Some("## Parts found but no table".to_string());
        return result;
    };

    let mut rows = Vec::new();
    for line in lines.iter().skip(table_start + 2) {
        let trimmed = line.trim();
        if !trimmed.starts_with('|') {
            break;
        }
        let cells: Vec<&str> = trimmed
            .split('|')
            .skip(1)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if cells.len() < 4 {
            continue;
        }
        let Ok(number) = cells[0].trim().parse::<usize>() else {
            continue;
        };
        let file_raw = cells[1].trim();
        let scope = cells[2].trim().to_string();
        let status_raw = cells[3].trim().to_lowercase();

        let file = strip_backticks(file_raw);
        if file.is_empty() || !file.ends_with(".md") {
            continue;
        }
        let status = match status_raw.as_str() {
            "pending" => RowStatus::Pending,
            "done" => RowStatus::Done,
            _ => continue,
        };

        rows.push(ManifestRow {
            number,
            file,
            scope,
            status,
        });
    }

    if rows.is_empty() {
        result.warning = Some("## Parts table has no valid rows".to_string());
        return result;
    }

    result.manifest = Some(PartsManifest { rows });
    result
}

/// Returns true only if `row` is marked `done` in the table AND its
/// normalized part file actually exists on disk. A `done` row whose file is
/// missing (e.g. the model never wrote it, or wrote it to the wrong place)
/// is treated as not actually finished, so callers keep directing the model
/// back to it instead of silently accepting a false completion.
pub fn row_is_verified_done(stem_dir: &Path, row: &ManifestRow) -> bool {
    row.status == RowStatus::Done
        && normalize_part_path(stem_dir, &row.file).is_some_and(|path| path.exists())
}

/// True if a `File` cell still carries template placeholder notation (`<stem>/core.md`,
/// `<id>/api.md`) instead of a real path.
pub fn part_file_cell_has_placeholder(file_cell: &str) -> bool {
    file_cell.contains('<') || file_cell.contains('>')
}

/// Explains why a `File` cell is unusable, or `None` if it is fine.
///
/// Only the basename is needed to resolve the file, so every one of these defects used to pass
/// silently: `<stem>/core.md` resolved to the real file, verified `done`, and shipped in an index
/// whose rows no reader could follow. The cell is not just a lookup key — an index is routinely
/// handed to a downstream reader as text and nothing else, so it must be openable as written.
pub fn part_file_cell_problem(stem_dir: &Path, file_cell: &str) -> Option<String> {
    if part_file_cell_has_placeholder(file_cell) {
        return Some(format!(
            "`{file_cell}` still has an unsubstituted placeholder — write the real path"
        ));
    }
    if file_cell.starts_with('/') || file_cell.contains("..") {
        return Some(format!(
            "`{file_cell}` must be relative to the index and stay inside the plan's own directory"
        ));
    }
    if Path::new(file_cell).extension().and_then(|e| e.to_str()) != Some("md") {
        return Some(format!("`{file_cell}` is not a `.md` file"));
    }
    let expected_dir = stem_dir.file_name().and_then(|n| n.to_str())?;
    match part_cell_directory(file_cell) {
        Some(dir) if dir != expected_dir => Some(format!(
            "`{file_cell}` points at `{dir}/`, but this plan's parts live in `{expected_dir}/`"
        )),
        Some(_) => None,
        None => Some(format!(
            "`{file_cell}` has no directory — write it as `{expected_dir}/{file_cell}` so a reader \
             can find it"
        )),
    }
}

/// The single directory component of a `File` cell, or `None` if it is a bare file name.
/// A cell with nested directories returns the first component, which will not match the expected
/// stem and is reported as a mismatch.
fn part_cell_directory(file_cell: &str) -> Option<&str> {
    let (dir, _) = file_cell.rsplit_once('/')?;
    Some(dir)
}

pub fn normalize_part_path(stem_dir: &Path, file_cell: &str) -> Option<PathBuf> {
    if file_cell.starts_with('/') || file_cell.contains("..") {
        return None;
    }
    if part_file_cell_has_placeholder(file_cell) {
        return None;
    }
    // A cell naming some other directory is wrong even though the basename would resolve: readers
    // follow the cell as written. A bare name still resolves — existing plans use it, and it is
    // merely under-specified rather than misleading.
    if let Some(dir) = part_cell_directory(file_cell)
        && stem_dir.file_name().and_then(|n| n.to_str()) != Some(dir)
    {
        return None;
    }
    let path = Path::new(file_cell);
    let basename = path.file_name()?;
    let normalized = stem_dir.join(basename);
    if !normalized.starts_with(stem_dir) {
        return None;
    }
    if normalized.extension()? != "md" {
        return None;
    }
    Some(normalized)
}

fn strip_backticks(value: &str) -> String {
    value.trim().trim_matches('`').trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_parts_manifest_finds_table() {
        let markdown = r#"# Plan

## Parts
| # | File | Scope | Status |
|---|---|---|---|
| 1 | core.md | models + persistence | pending |
| 2 | api.md | endpoints + wiring | pending |
| 3 | ui.md | rendering | pending |
"#;
        let result = parse_parts_manifest(markdown);
        let manifest = result.manifest.expect("expected manifest");
        assert_eq!(manifest.rows.len(), 3);
        assert_eq!(manifest.rows[0].file, "core.md");
        assert_eq!(manifest.rows[0].status, RowStatus::Pending);
    }

    #[test]
    fn parse_parts_manifest_ignores_separator() {
        let markdown = r#"## Parts
| # | File | Scope | Status |
|---|---|---|---|
| 1 | core.md | models | pending |
"#;
        let result = parse_parts_manifest(markdown);
        let manifest = result.manifest.expect("expected manifest");
        assert_eq!(manifest.rows.len(), 1);
    }

    #[test]
    fn parse_parts_manifest_strips_backticks() {
        let markdown = r#"## Parts
| # | File | Scope | Status |
|---|---|---|---|
| 1 | `api.md` | endpoints | pending |
"#;
        let result = parse_parts_manifest(markdown);
        let manifest = result.manifest.expect("expected manifest");
        assert_eq!(manifest.rows[0].file, "api.md");
    }

    #[test]
    fn parse_parts_manifest_rejects_invalid_status() {
        let markdown = r#"## Parts
| # | File | Scope | Status |
|---|---|---|---|
| 1 | core.md | models | in-progress |
"#;
        let result = parse_parts_manifest(markdown);
        assert!(result.manifest.is_none());
        assert!(result.warning.is_some());
    }

    #[test]
    fn parse_parts_manifest_returns_none_without_heading() {
        let markdown = r#"| # | File | Scope | Status |
|---|---|---|---|
| 1 | core.md | models | pending |
"#;
        let result = parse_parts_manifest(markdown);
        assert!(result.manifest.is_none());
        assert!(result.warning.is_none());
    }

    #[test]
    fn normalize_part_path_accepts_basename() {
        let stem = Path::new("/plans/2026-07-05-topic");
        let normalized = normalize_part_path(stem, "core.md").unwrap();
        assert_eq!(normalized, Path::new("/plans/2026-07-05-topic/core.md"));
    }

    #[test]
    fn normalize_part_path_rejects_traversal() {
        let stem = Path::new("/plans/2026-07-05-topic");
        assert!(normalize_part_path(stem, "../escape.md").is_none());
    }

    #[test]
    fn normalize_part_path_rejects_absolute() {
        let stem = Path::new("/plans/2026-07-05-topic");
        assert!(normalize_part_path(stem, "/etc/passwd.md").is_none());
    }

    #[test]
    fn normalize_part_path_rejects_non_md() {
        let stem = Path::new("/plans/2026-07-05-topic");
        assert!(normalize_part_path(stem, "core.txt").is_none());
    }

    /// Regression: a real shipped plan carried `<stem>/core-widget.md` rows. Only the basename is
    /// kept, so the cell resolved to the real file, existed, and verified `done` — the plan
    /// finalized with rows no reader could follow, and the executing agent went hunting for a
    /// literal `<stem>` directory. The cell is part of the artifact, not just a lookup key.
    #[test]
    fn normalize_part_path_rejects_unsubstituted_placeholders() {
        let stem = Path::new("/plans/2026-07-05-topic");
        for cell in [
            "<stem>/core.md",
            "<id>/core.md",
            "<plan-stem>/core.md",
            "<part-name>.md",
        ] {
            assert!(
                normalize_part_path(stem, cell).is_none(),
                "{cell:?} is a placeholder, not a file name, and must not resolve"
            );
        }
    }

    #[test]
    fn part_file_cell_has_placeholder_only_flags_placeholder_notation() {
        assert!(part_file_cell_has_placeholder("<stem>/core.md"));
        assert!(part_file_cell_has_placeholder("<id>/api.md"));
        assert!(!part_file_cell_has_placeholder("core.md"));
        assert!(!part_file_cell_has_placeholder("2026-07-05-topic/core.md"));
    }

    /// A cell naming the plan's real directory is the only fully usable form. Both shipped failure
    /// modes — an unsubstituted placeholder, and a bare name that says nothing about location —
    /// must be reported, and the report has to name the directory the model should have used, or it
    /// cannot act on it.
    #[test]
    fn part_file_cell_problem_explains_each_unusable_form() {
        let stem = Path::new("/plans/2026-07-05-topic");

        assert_eq!(
            part_file_cell_problem(stem, "2026-07-05-topic/core.md"),
            None
        );

        let placeholder = part_file_cell_problem(stem, "<stem>/core.md").unwrap();
        assert!(placeholder.contains("placeholder"), "{placeholder}");

        let bare = part_file_cell_problem(stem, "core.md").unwrap();
        assert!(
            bare.contains("2026-07-05-topic/core.md"),
            "a bare cell's report must show the path to use, got: {bare}"
        );

        let wrong_dir = part_file_cell_problem(stem, "2026-07-10-design-mode/core.md").unwrap();
        assert!(
            wrong_dir.contains("2026-07-05-topic/"),
            "a wrong-directory report must name the real directory, got: {wrong_dir}"
        );

        assert!(part_file_cell_problem(stem, "core.txt").is_some());
        assert!(part_file_cell_problem(stem, "/etc/passwd.md").is_some());
    }

    /// A cell pointing at someone else's directory must not resolve. Only the basename is used, so
    /// it would otherwise silently find the right file while telling readers to look elsewhere —
    /// exactly the tolerance that let `<stem>/core.md` ship.
    #[test]
    fn normalize_part_path_rejects_a_foreign_directory() {
        let stem = Path::new("/plans/2026-07-05-topic");
        assert!(normalize_part_path(stem, "2026-07-10-design-mode/core.md").is_none());
        assert_eq!(
            normalize_part_path(stem, "2026-07-05-topic/core.md").unwrap(),
            Path::new("/plans/2026-07-05-topic/core.md")
        );
    }

    /// A `done` row whose cell is a placeholder must not count as finished, even though the
    /// basename would resolve to a file that really exists.
    #[test]
    fn row_with_placeholder_cell_is_not_verified_done() {
        let tmp = tempfile::tempdir().unwrap();
        let stem = tmp.path();
        std::fs::write(stem.join("core.md"), "# Part 1\n").unwrap();

        let placeholder = ManifestRow {
            number: 1,
            file: "<stem>/core.md".to_string(),
            scope: "core".to_string(),
            status: RowStatus::Done,
        };
        assert!(!row_is_verified_done(stem, &placeholder));

        let real = ManifestRow {
            file: "core.md".to_string(),
            ..placeholder
        };
        assert!(row_is_verified_done(stem, &real));
    }
}
