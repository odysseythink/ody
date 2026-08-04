use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Manifest layout selected by its table headers.  Legacy manifests remain
/// supported so an in-flight plan is never reinterpreted as a stricter task
/// plan merely because the host was upgraded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestFormat {
    Legacy,
    Task,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PartsManifest {
    pub format: ManifestFormat,
    pub rows: Vec<ManifestRow>,
}

impl PartsManifest {
    pub fn is_task_mode(&self) -> bool {
        self.format == ManifestFormat::Task
    }
}

/// Returns contract changes that are not legal after a split-plan manifest has
/// been accepted. The manifest is the execution contract: later submissions
/// may only advance row status from `pending` to `done`.
pub fn manifest_progression_violations(
    previous: &PartsManifest,
    current: &PartsManifest,
) -> Vec<String> {
    let mut violations = Vec::new();
    if previous.format != current.format {
        violations.push("manifest format changed".to_string());
    }
    if previous.rows.len() != current.rows.len() {
        violations.push(format!(
            "row count changed from {} to {}",
            previous.rows.len(),
            current.rows.len()
        ));
    }

    for (position, (before, after)) in previous.rows.iter().zip(current.rows.iter()).enumerate() {
        let row = position + 1;
        if before.id != after.id {
            violations.push(format!(
                "row {row} ID changed from `{}` to `{}`",
                before.id, after.id
            ));
        }
        if before.file != after.file {
            violations.push(format!(
                "task `{}` File changed from `{}` to `{}`",
                before.id, before.file, after.file
            ));
        }
        if before.task != after.task {
            violations.push(format!("task `{}` title changed", before.id));
        }
        if before.scope != after.scope {
            violations.push(format!("task `{}` Scope changed", before.id));
        }
        if before.depends_on != after.depends_on {
            violations.push(format!("task `{}` dependencies changed", before.id));
        }
        if before.status == RowStatus::Done && after.status == RowStatus::Pending {
            violations.push(format!(
                "task `{}` regressed from done to pending",
                before.id
            ));
        }
    }

    violations
}

#[derive(Debug, Clone, PartialEq)]
pub struct ManifestRow {
    /// Display identifier from the `#` column. This is deliberately opaque:
    /// split refinements commonly use ids such as `4a` / `4b`, and manifest
    /// order (not numeric ordering) determines which part runs next.
    pub id: String,
    pub file: String,
    /// Stable implementation-task label from task-mode manifests.  Legacy
    /// manifests deliberately leave this absent rather than inventing a task
    /// identity from their free-form Scope cell.
    pub task: Option<String>,
    pub scope: String,
    /// Stable task IDs that must complete before this row may be accepted.
    /// Empty for legacy manifests and for task rows with no dependencies.
    pub depends_on: Vec<String>,
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
    /// Errors for individual table rows. Callers that persist a manifest must
    /// reject these rather than silently dropping rows and advancing past work.
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
struct ManifestColumns {
    id: usize,
    file: usize,
    task: Option<usize>,
    scope: usize,
    depends_on: Option<usize>,
    status: usize,
}

fn table_cells(line: &str) -> Vec<&str> {
    line.trim().split('|').skip(1).map(str::trim).collect()
}

fn header_index(headers: &[&str], names: &[&str]) -> Option<usize> {
    headers.iter().position(|header| {
        let normalized = header.trim().trim_matches('`').to_ascii_lowercase();
        names.iter().any(|name| normalized == *name)
    })
}

fn manifest_columns(headers: &[&str]) -> Result<(ManifestFormat, ManifestColumns), String> {
    let id = header_index(headers, &["#", "id", "task id"])
        .ok_or_else(|| "Parts table is missing a `#` or `ID` column".to_string())?;
    let file = header_index(headers, &["file"])
        .ok_or_else(|| "Parts table is missing a `File` column".to_string())?;
    let scope = header_index(headers, &["scope"])
        .ok_or_else(|| "Parts table is missing a `Scope` column".to_string())?;
    let status = header_index(headers, &["status"])
        .ok_or_else(|| "Parts table is missing a `Status` column".to_string())?;
    let task = header_index(headers, &["task", "task title"]);
    let depends_on = header_index(headers, &["depends on", "dependencies"]);
    let format = if task.is_some() || depends_on.is_some() {
        ManifestFormat::Task
    } else {
        ManifestFormat::Legacy
    };

    if format == ManifestFormat::Task && task.is_none() {
        return Err(
            "task-mode Parts table has `Depends on` but no required `Task` column".to_string(),
        );
    }

    Ok((
        format,
        ManifestColumns {
            id,
            file,
            task,
            scope,
            depends_on,
            status,
        },
    ))
}

fn cell<'a>(cells: &'a [&'a str], index: usize) -> &'a str {
    cells.get(index).copied().unwrap_or_default().trim()
}

fn parse_dependencies(raw: &str) -> Vec<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || matches!(trimmed, "-" | "—") || trimmed.eq_ignore_ascii_case("none")
    {
        return Vec::new();
    }
    trimmed
        .split(',')
        .map(|dependency| strip_backticks(dependency.trim()).to_string())
        .filter(|dependency| !dependency.is_empty())
        .collect()
}

pub fn parse_parts_manifest(content: &str) -> ManifestParseResult {
    let mut result = ManifestParseResult {
        manifest: None,
        warning: None,
        diagnostics: Vec::new(),
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
        let message = "## Parts found but no table".to_string();
        result.warning = Some(message.clone());
        result.diagnostics.push(message);
        return result;
    };

    let header_cells = table_cells(lines[table_start]);
    let (format, columns) = match manifest_columns(&header_cells) {
        Ok(columns) => columns,
        Err(message) => {
            result.warning = Some(message.clone());
            result.diagnostics.push(message);
            return result;
        }
    };

    let mut rows = Vec::new();
    for (row_offset, line) in lines.iter().skip(table_start + 2).enumerate() {
        let trimmed = line.trim();
        if !trimmed.starts_with('|') {
            break;
        }
        let cells = table_cells(trimmed);
        let row_number = row_offset + 1;
        let required_cells = [columns.id, columns.file, columns.scope, columns.status]
            .into_iter()
            .chain(columns.task)
            .max()
            .unwrap_or_default()
            + 1;
        if cells.len() < required_cells {
            result.diagnostics.push(format!(
                "Parts table row {row_number} has fewer cells than its header"
            ));
            continue;
        }
        let id = cell(&cells, columns.id);
        if id.is_empty() {
            result
                .diagnostics
                .push(format!("Parts table row {row_number} has an empty ID cell"));
            continue;
        }
        let file_raw = cell(&cells, columns.file);
        let scope = cell(&cells, columns.scope).to_string();
        if scope.is_empty() {
            result.diagnostics.push(format!(
                "Parts table row {row_number} has an empty Scope cell"
            ));
            continue;
        }
        let task = columns.task.map(|index| cell(&cells, index).to_string());
        if format == ManifestFormat::Task && task.as_deref().is_none_or(str::is_empty) {
            result.diagnostics.push(format!(
                "task-mode Parts table row {row_number} has an empty Task cell"
            ));
            continue;
        }
        let depends_on = columns
            .depends_on
            .map(|index| parse_dependencies(cell(&cells, index)))
            .unwrap_or_default();
        let status_raw = cell(&cells, columns.status).to_lowercase();

        let file = strip_backticks(file_raw);
        if file.is_empty() || !file.ends_with(".md") {
            result.diagnostics.push(format!(
                "Parts table row {row_number} has invalid File cell `{file_raw}`; expected a `.md` path"
            ));
            continue;
        }
        let status = match status_raw.as_str() {
            "pending" => RowStatus::Pending,
            "done" => RowStatus::Done,
            _ => {
                result.diagnostics.push(format!(
                    "Parts table row {row_number} has invalid Status cell `{}`; expected `pending` or `done`",
                    cell(&cells, columns.status)
                ));
                continue;
            }
        };

        rows.push(ManifestRow {
            id: id.to_string(),
            file,
            task,
            scope,
            depends_on,
            status,
        });
    }

    if rows.is_empty() {
        let message = "## Parts table has no valid rows".to_string();
        result.warning = Some(message.clone());
        result.diagnostics.push(message);
        return result;
    }

    let mut seen_ids = HashSet::new();
    let mut seen_files = HashSet::new();
    for row in &rows {
        if !seen_ids.insert(row.id.as_str()) {
            result
                .diagnostics
                .push(format!("Parts table has duplicate ID `{}`", row.id));
        }
        if !seen_files.insert(row.file.as_str()) {
            result
                .diagnostics
                .push(format!("Parts table has duplicate File `{}`", row.file));
        }
    }
    if format == ManifestFormat::Task {
        for row in &rows {
            for dependency in &row.depends_on {
                if dependency == &row.id {
                    result
                        .diagnostics
                        .push(format!("task `{}` cannot depend on itself", row.id));
                } else if !seen_ids.contains(dependency.as_str()) {
                    result.diagnostics.push(format!(
                        "task `{}` depends on unknown task `{dependency}`",
                        row.id
                    ));
                }
            }
        }
    }

    result.manifest = Some(PartsManifest { format, rows });
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

/// Returns human-readable hard-budget violations for a persisted part. A zero
/// limit disables its corresponding check. Missing or pending parts are not
/// violations: the caller should continue directing the model to write them.
pub fn part_budget_violations(
    stem_dir: &Path,
    row: &ManifestRow,
    max_tasks: usize,
    max_bytes: usize,
) -> Vec<String> {
    if !row_is_verified_done(stem_dir, row) {
        return Vec::new();
    }
    let Some(path) = normalize_part_path(stem_dir, &row.file) else {
        return Vec::new();
    };
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut violations = Vec::new();
    let tasks = count_task_headings(&content);
    if max_tasks > 0 && tasks > max_tasks {
        violations.push(format!("{tasks} tasks exceeds the {max_tasks}-task limit"));
    }
    if max_bytes > 0 && content.len() > max_bytes {
        violations.push(format!(
            "{} bytes exceeds the {}-byte limit",
            content.len(),
            max_bytes
        ));
    }
    violations
}

/// Validates the durable structure of a task-mode part.  This intentionally
/// checks for concrete evidence rather than byte size: a short summary cannot
/// be marked done merely because it happens to fit in a file-size budget.
pub fn task_part_structure_violations(stem_dir: &Path, row: &ManifestRow) -> Vec<String> {
    if !row_is_verified_done(stem_dir, row) {
        return Vec::new();
    }
    let Some(expected_task) = row.task.as_deref() else {
        return Vec::new();
    };
    let Some(path) = normalize_part_path(stem_dir, &row.file) else {
        return Vec::new();
    };
    let Ok(content) = std::fs::read_to_string(path) else {
        return vec!["part file could not be read for task completeness review".to_string()];
    };

    let mut violations = Vec::new();
    let task_heading_count = count_task_headings(&content);
    if task_heading_count != 1 {
        violations.push(format!(
            "task-mode part must contain exactly one Task heading, found {task_heading_count}"
        ));
    }
    if !contains_manifest_task_id(&content, &row.id) {
        violations.push(format!(
            "missing manifest task marker `{}` (write `**Manifest task:** {}`)",
            row.id, row.id
        ));
    }
    if !contains_task_label(&content, expected_task) {
        violations.push(format!(
            "its sole Task heading does not name manifest task `{expected_task}`"
        ));
    }
    for (label, alternatives) in [
        ("Files", &["files"] as &[&str]),
        ("Source evidence", &["source evidence"]),
        ("Implementation", &["implementation"]),
        (
            "Failure and edge cases",
            &[
                "failure and edge cases",
                "failure / edge cases",
                "edge cases",
            ],
        ),
        ("Tests", &["tests", "test plan"]),
    ] {
        if !has_nonempty_labeled_block(&content, alternatives) {
            violations.push(format!("missing non-empty `{label}` block"));
        }
    }
    if source_anchor_count(&content) == 0 {
        violations.push(
            "Source evidence must include at least one backticked `path:line` anchor".to_string(),
        );
    }
    let completed_review_items = completed_self_review_items(&content);
    if completed_review_items < 7 {
        violations.push(format!(
            "Self-review has {completed_review_items} completed item(s); all 7 are required"
        ));
    }
    violations
}

/// Combines the generic operational limits with task-mode completeness and
/// dependency checks.  Legacy manifests retain their existing semantics.
pub fn part_completion_violations(
    stem_dir: &Path,
    manifest: &PartsManifest,
    row: &ManifestRow,
    max_tasks: usize,
    max_bytes: usize,
) -> Vec<String> {
    let mut violations = part_budget_violations(stem_dir, row, max_tasks, max_bytes);
    if !manifest.is_task_mode() || !row_is_verified_done(stem_dir, row) {
        return violations;
    }

    violations.extend(task_part_structure_violations(stem_dir, row));
    for dependency in &row.depends_on {
        let dependency_row = manifest
            .rows
            .iter()
            .find(|candidate| candidate.id == *dependency);
        let dependency_complete = dependency_row.is_some_and(|candidate| {
            row_is_verified_done(stem_dir, candidate)
                && task_part_structure_violations(stem_dir, candidate).is_empty()
        });
        if !dependency_complete {
            violations.push(format!(
                "dependency `{dependency}` is not a verified completed task"
            ));
        }
    }
    violations
}

fn contains_manifest_task_id(content: &str, task_id: &str) -> bool {
    content.lines().any(|line| {
        let trimmed = line.trim();
        trimmed
            .get(.."**manifest task:**".len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("**manifest task:**"))
            && trimmed
                .get("**manifest task:**".len()..)
                .is_some_and(|value| value.trim().trim_matches('`').eq_ignore_ascii_case(task_id))
    })
}

fn contains_task_label(content: &str, task: &str) -> bool {
    let task_lower = task.to_ascii_lowercase();
    content.lines().any(|line| {
        let trimmed = line.trim_start();
        let hashes = trimmed
            .chars()
            .take_while(|character| *character == '#')
            .count();
        if !(2..=4).contains(&hashes) {
            return false;
        }
        let heading = trimmed[hashes..].trim_start().to_ascii_lowercase();
        heading.starts_with("task ") && heading.contains(&task_lower)
    })
}

fn has_nonempty_labeled_block(content: &str, labels: &[&str]) -> bool {
    let lines = content.lines().collect::<Vec<_>>();
    for (index, line) in lines.iter().enumerate() {
        let normalized = line.trim().trim_matches('*').trim().to_ascii_lowercase();
        let Some(label) = labels.iter().find(|label| normalized.starts_with(**label)) else {
            continue;
        };
        let remainder = normalized[label.len()..].trim_start_matches(':').trim();
        if !remainder.is_empty() {
            return true;
        }
        for following in lines.iter().skip(index + 1) {
            let trimmed = following.trim();
            if trimmed.starts_with('#') || trimmed.starts_with("**") {
                break;
            }
            if !trimmed.is_empty() {
                return true;
            }
        }
    }
    false
}

fn source_anchor_count(content: &str) -> usize {
    content
        .split('`')
        .skip(1)
        .step_by(2)
        .filter(|candidate| {
            candidate.rsplit_once(':').is_some_and(|(path, line)| {
                !path.trim().is_empty() && line.trim().parse::<usize>().is_ok()
            })
        })
        .count()
}

fn completed_self_review_items(content: &str) -> usize {
    let lower = content.to_ascii_lowercase();
    let Some(start) = lower
        .find("## self-review")
        .or_else(|| lower.find("## self review"))
    else {
        return 0;
    };
    let section = &lower[start..];
    let section_end = section.find("\n## ").unwrap_or(section.len());
    section[..section_end]
        .lines()
        .filter(|line| line.trim_start().starts_with("- [x]"))
        .count()
}

fn count_task_headings(content: &str) -> usize {
    content
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            let hashes = trimmed.chars().take_while(|c| *c == '#').count();
            if !(2..=4).contains(&hashes) {
                return false;
            }
            let rest = trimmed[hashes..].trim_start();
            let lower = rest.to_ascii_lowercase();
            lower.strip_prefix("task ").is_some_and(|suffix| {
                let suffix = suffix.trim_start();
                suffix.chars().next().is_some_and(|character| {
                    character.is_ascii_digit()
                        || (character == 't'
                            && suffix
                                .chars()
                                .nth(1)
                                .is_some_and(|next| next.is_ascii_digit()))
                })
            })
        })
        .count()
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

/// Moves top-level Markdown files in a split directory that are no longer
/// referenced by the current manifest into a local quarantine.  This is kept
/// inside the plan's own directory so it is recoverable, and deliberately does
/// not recurse or follow links.
pub fn quarantine_untracked_part_files(
    stem_dir: &Path,
    manifest: &PartsManifest,
) -> std::io::Result<Vec<PathBuf>> {
    if !stem_dir.is_dir() {
        return Ok(Vec::new());
    }
    let tracked = manifest
        .rows
        .iter()
        .filter_map(|row| normalize_part_path(stem_dir, &row.file))
        .collect::<std::collections::HashSet<_>>();
    let candidates = std::fs::read_dir(stem_dir)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let is_markdown = path.extension().and_then(|ext| ext.to_str()) == Some("md");
            entry
                .file_type()
                .ok()?
                .is_file()
                .then_some(path)
                .filter(|_| is_markdown)
        })
        .filter(|path| !tracked.contains(path))
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    let quarantine = stem_dir.join(".orphaned");
    std::fs::create_dir_all(&quarantine)?;
    let mut moved = Vec::with_capacity(candidates.len());
    for source in candidates {
        let name = source.file_name().expect("read_dir paths have a file name");
        let mut target = quarantine.join(name);
        let mut suffix = 1usize;
        while target.exists() {
            target = quarantine.join(format!("{}.{}", name.to_string_lossy(), suffix));
            suffix += 1;
        }
        std::fs::rename(&source, &target)?;
        moved.push(target);
    }
    Ok(moved)
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
    fn manifest_progression_allows_only_pending_to_done() {
        let previous = parse_parts_manifest(
            r#"## Parts
| ID | File | Task | Scope | Depends on | Status |
|---|---|---|---|---|---|
| T01 | `topic/domain.md` | Add domain model | domain | none | pending |
| T02 | `topic/api.md` | Add API | api | T01 | pending |
"#,
        )
        .manifest
        .unwrap();
        let current = parse_parts_manifest(
            r#"## Parts
| ID | File | Task | Scope | Depends on | Status |
|---|---|---|---|---|---|
| T01 | `topic/domain.md` | Add domain model | domain | none | done |
| T02 | `topic/api.md` | Add API | api | T01 | pending |
"#,
        )
        .manifest
        .unwrap();

        assert!(manifest_progression_violations(&previous, &current).is_empty());
    }

    #[test]
    fn manifest_progression_rejects_repartition_after_acceptance() {
        let previous = parse_parts_manifest(
            r#"## Parts
| ID | File | Task | Scope | Depends on | Status |
|---|---|---|---|---|---|
| T01 | `topic/domain.md` | Add domain model | domain | none | done |
| T02 | `topic/api.md` | Add API | api | T01 | pending |
"#,
        )
        .manifest
        .unwrap();
        let repartitioned = parse_parts_manifest(
            r#"## Parts
| ID | File | Task | Scope | Depends on | Status |
|---|---|---|---|---|---|
| T01a | `topic/domain-types.md` | Add domain types | domain types | none | pending |
| T01b | `topic/domain-tests.md` | Test domain model | domain tests | T01a | pending |
| T02 | `topic/api.md` | Add API | api | T01b | pending |
"#,
        )
        .manifest
        .unwrap();

        let violations = manifest_progression_violations(&previous, &repartitioned);
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("row count changed")),
            "{violations:?}"
        );
        assert!(
            violations
                .iter()
                .any(|violation| violation.contains("regressed from done to pending")),
            "{violations:?}"
        );
    }

    #[test]
    fn manifest_progression_rejects_contract_edits_and_done_regression() {
        let previous = PartsManifest {
            format: ManifestFormat::Task,
            rows: vec![ManifestRow {
                id: "T01".to_string(),
                file: "topic/domain.md".to_string(),
                task: Some("Add domain model".to_string()),
                scope: "domain".to_string(),
                depends_on: Vec::new(),
                status: RowStatus::Done,
            }],
        };
        let current = PartsManifest {
            format: ManifestFormat::Task,
            rows: vec![ManifestRow {
                id: "T01".to_string(),
                file: "topic/domain-v2.md".to_string(),
                task: Some("Replace domain model".to_string()),
                scope: "domain and storage".to_string(),
                depends_on: vec!["T00".to_string()],
                status: RowStatus::Pending,
            }],
        };

        let violations = manifest_progression_violations(&previous, &current);
        for expected in [
            "File changed",
            "title changed",
            "Scope changed",
            "dependencies changed",
            "regressed from done to pending",
        ] {
            assert!(
                violations
                    .iter()
                    .any(|violation| violation.contains(expected)),
                "missing {expected}: {violations:?}"
            );
        }
    }

    #[test]
    fn parse_parts_manifest_preserves_alphanumeric_ids_and_document_order() {
        let markdown = r#"## Parts
| # | File | Scope | Status |
|---|---|---|---|
| 4a | interaction-core.md | interaction | pending |
| 4b | interaction-tests.md | tests | pending |
| 5 | render.md | render | pending |
"#;

        let result = parse_parts_manifest(markdown);
        let manifest = result.manifest.expect("expected manifest");
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        assert_eq!(
            manifest
                .rows
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            vec!["4a", "4b", "5"]
        );
        assert_eq!(manifest.format, ManifestFormat::Legacy);
    }

    #[test]
    fn parse_task_manifest_keeps_task_identity_and_dependencies() {
        let markdown = r#"## Parts
| ID | File | Task | Scope | Depends on | Status |
|---|---|---|---|---|---|
| T01 | `topic/protocol.md` | Protocol types | protocol surface | — | done |
| T02 | `topic/config.md` | Configuration | config model | T01 | pending |
"#;

        let result = parse_parts_manifest(markdown);
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let manifest = result.manifest.expect("task manifest");
        assert_eq!(manifest.format, ManifestFormat::Task);
        assert!(manifest.is_task_mode());
        assert_eq!(manifest.rows[1].task.as_deref(), Some("Configuration"));
        assert_eq!(manifest.rows[1].depends_on, vec!["T01"]);
    }

    #[test]
    fn task_manifest_rejects_duplicate_and_unknown_dependencies() {
        let markdown = r#"## Parts
| ID | File | Task | Scope | Depends on | Status |
|---|---|---|---|---|---|
| T01 | `topic/a.md` | A | alpha | T99 | pending |
| T01 | `topic/b.md` | B | beta | — | pending |
"#;

        let result = parse_parts_manifest(markdown);
        assert!(
            result.manifest.is_some(),
            "valid rows remain visible to callers"
        );
        assert!(
            result
                .diagnostics
                .iter()
                .any(|item| item.contains("duplicate ID `T01`")),
            "{:?}",
            result.diagnostics
        );
        assert!(
            result
                .diagnostics
                .iter()
                .any(|item| item.contains("unknown task `T99`")),
            "{:?}",
            result.diagnostics
        );
    }

    #[test]
    fn parse_parts_manifest_reports_invalid_rows_instead_of_silently_skipping_them() {
        let markdown = r#"## Parts
| # | File | Scope | Status |
|---|---|---|---|
| 1 | core.md | core | pending |
|  | missing-id.md | invalid | pending |
| 3 | tests.md | tests | later |
"#;

        let result = parse_parts_manifest(markdown);
        assert_eq!(result.manifest.expect("valid row remains").rows.len(), 1);
        assert_eq!(result.diagnostics.len(), 2, "{:?}", result.diagnostics);
        assert!(result.diagnostics[0].contains("empty ID cell"));
        assert!(result.diagnostics[1].contains("invalid Status"));
    }

    #[test]
    fn part_budget_flags_excess_tasks_and_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let row = ManifestRow {
            id: "1".to_string(),
            file: "core.md".to_string(),
            task: None,
            scope: "core".to_string(),
            depends_on: Vec::new(),
            status: RowStatus::Done,
        };
        std::fs::write(
            tmp.path().join("core.md"),
            "### Task 1\n\n### Task 2\n\n### Task 3\n\n### Task 4\n",
        )
        .unwrap();

        let violations = part_budget_violations(tmp.path(), &row, 3, 10);
        assert!(violations.iter().any(|v| v.contains("4 tasks")));
        assert!(violations.iter().any(|v| v.contains("bytes")));
    }

    fn complete_task_part(task_id: &str, task: &str) -> String {
        format!(
            "# {task}\n\n**Manifest task:** {task_id}\n\n### Task {task_id}: {task}\n\n**Files**\n- `core/src/example.rs`\n\n**Source evidence**\n- `core/src/example.rs:42` — existing entry point\n\n**Implementation**\n1. Make the concrete change.\n\n**Failure and edge cases**\n- Preserve the existing error path.\n\n**Tests**\n- Add a behavioral regression test.\n\n## Self-review\n- [x] 1. evidence\n- [x] 2. evidence\n- [x] 3. evidence\n- [x] 4. evidence\n- [x] 5. evidence\n- [x] 6. evidence\n- [x] 7. evidence\n"
        )
    }

    #[test]
    fn task_part_requires_concrete_structure_and_completed_dependencies() {
        let tmp = tempfile::tempdir().unwrap();
        let stem = tmp.path().join("topic");
        std::fs::create_dir_all(&stem).unwrap();
        std::fs::write(
            stem.join("protocol.md"),
            complete_task_part("T01", "Protocol types"),
        )
        .unwrap();
        std::fs::write(
            stem.join("config.md"),
            complete_task_part("T02", "Configuration model"),
        )
        .unwrap();
        let manifest = parse_parts_manifest(
            "## Parts\n| ID | File | Task | Scope | Depends on | Status |\n|---|---|---|---|---|---|\n| T01 | `topic/protocol.md` | Protocol types | protocol | — | done |\n| T02 | `topic/config.md` | Configuration model | config | T01 | done |\n",
        )
        .manifest
        .unwrap();

        let violations = part_completion_violations(&stem, &manifest, &manifest.rows[1], 1, 0);
        assert!(violations.is_empty(), "{violations:?}");
    }

    #[test]
    fn task_part_rejects_short_summary_and_unfinished_dependency() {
        let tmp = tempfile::tempdir().unwrap();
        let stem = tmp.path().join("topic");
        std::fs::create_dir_all(&stem).unwrap();
        std::fs::write(stem.join("protocol.md"), "# Protocol\n").unwrap();
        std::fs::write(
            stem.join("config.md"),
            complete_task_part("T02", "Configuration model"),
        )
        .unwrap();
        let manifest = parse_parts_manifest(
            "## Parts\n| ID | File | Task | Scope | Depends on | Status |\n|---|---|---|---|---|---|\n| T01 | `topic/protocol.md` | Protocol types | protocol | — | done |\n| T02 | `topic/config.md` | Configuration model | config | T01 | done |\n",
        )
        .manifest
        .unwrap();

        let violations = part_completion_violations(&stem, &manifest, &manifest.rows[1], 1, 0);
        assert!(
            violations
                .iter()
                .any(|item| item.contains("dependency `T01` is not a verified completed task")),
            "{violations:?}"
        );
        let protocol_violations =
            part_completion_violations(&stem, &manifest, &manifest.rows[0], 1, 0);
        assert!(
            protocol_violations
                .iter()
                .any(|item| item.contains("exactly one Task heading")),
            "{protocol_violations:?}"
        );
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
    fn quarantine_moves_only_untracked_top_level_markdown_parts() {
        let tmp = tempfile::tempdir().unwrap();
        let stem = tmp.path().join("plan");
        std::fs::create_dir_all(&stem).unwrap();
        std::fs::write(stem.join("active.md"), "active").unwrap();
        std::fs::write(stem.join("stale.md"), "stale").unwrap();
        std::fs::write(stem.join("notes.txt"), "keep").unwrap();
        let manifest = PartsManifest {
            format: ManifestFormat::Legacy,
            rows: vec![ManifestRow {
                id: "1".to_string(),
                file: "active.md".to_string(),
                task: None,
                scope: "active".to_string(),
                depends_on: Vec::new(),
                status: RowStatus::Pending,
            }],
        };

        let moved = quarantine_untracked_part_files(&stem, &manifest).unwrap();

        assert_eq!(moved, vec![stem.join(".orphaned").join("stale.md")]);
        assert!(stem.join("active.md").exists());
        assert!(stem.join("notes.txt").exists());
        assert!(!stem.join("stale.md").exists());
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
            id: "1".to_string(),
            file: "<stem>/core.md".to_string(),
            task: None,
            scope: "core".to_string(),
            depends_on: Vec::new(),
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
