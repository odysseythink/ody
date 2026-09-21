//! Filesystem-backed store for odyBox assistant memory.
//!
//! Deliberately self-contained: it does **not** reuse the `pub(crate)` types in
//! `ody-memories-extension`, so nothing in the Ody memory path has to be opened
//! up or modified to support assistant memory.
//!
//! Every path is resolved against the assistant memory root and validated
//! before any I/O, so a model-supplied path can never escape the root.

use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use schemars::JsonSchema;
use serde::Serialize;

/// Default number of characters returned by a single `read`.
pub const DEFAULT_READ_MAX_CHARS: usize = 20_000;
/// Default cap on `search` results per call.
pub const DEFAULT_SEARCH_MAX_RESULTS: usize = 100;
/// Hard cap on `search` results per call.
pub const MAX_SEARCH_MAX_RESULTS: usize = 200;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("path '{path}' {reason}")]
    InvalidPath { path: String, reason: String },
    #[error("path '{path}' was not found")]
    NotFound { path: String },
    #[error("path '{path}' is not a file")]
    NotFile { path: String },
    #[error("line_offset exceeds file length")]
    LineOffsetExceedsFileLength,
    #[error("filename '{filename}' {reason}")]
    InvalidFilename { filename: String, reason: String },
    #[error("note must not be empty")]
    EmptyNote,
    #[error("note '{filename}' already exists")]
    NoteAlreadyExists { filename: String },
    #[error("queries must not be empty or contain empty strings")]
    EmptyQuery,
    #[error("I/O error while accessing assistant memories: {0}")]
    Io(#[from] std::io::Error),
}

impl StoreError {
    fn invalid_path(path: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::InvalidPath {
            path: path.into(),
            reason: reason.into(),
        }
    }

    fn invalid_filename(filename: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::InvalidFilename {
            filename: filename.into(),
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ReadResponse {
    /// Path relative to the assistant memory root.
    pub path: String,
    /// 1-indexed line number of the first returned line.
    pub start_line_number: usize,
    pub content: String,
    /// Whether `content` was cut short by the character budget.
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct SearchMatch {
    /// Path relative to the assistant memory root.
    pub path: String,
    /// 1-indexed line number of the first matching line in the window.
    pub match_line_number: usize,
    /// Matching lines plus `context_lines` of surrounding context.
    pub content: String,
    /// Which of the requested queries matched inside the window.
    pub matched_queries: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct SearchResponse {
    pub queries: Vec<String>,
    pub path: Option<String>,
    pub matches: Vec<SearchMatch>,
    /// Whether results were cut off by `max_results`.
    pub truncated: bool,
}

/// Assistant memory store rooted at `$ODY_HOME/odybox_memories`.
#[derive(Clone, Debug)]
pub struct AssistantMemoryStore {
    root: PathBuf,
}

impl AssistantMemoryStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolves an optional caller-supplied relative path inside the root.
    ///
    /// Rejects absolute paths, parent traversal, and hidden components so the
    /// store can never read outside its own root.
    fn resolve_scoped_path(&self, relative: Option<&str>) -> Result<PathBuf, StoreError> {
        let Some(relative) = relative else {
            return Ok(self.root.clone());
        };
        if relative.trim().is_empty() {
            return Err(StoreError::invalid_path(relative, "must not be empty"));
        }

        let candidate = Path::new(relative);
        if candidate.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            return Err(StoreError::invalid_path(
                relative,
                "must stay within the assistant memory root",
            ));
        }
        if candidate.components().any(|component| match component {
            Component::Normal(name) => name.to_string_lossy().starts_with('.'),
            _ => false,
        }) {
            return Err(StoreError::NotFound {
                path: relative.to_string(),
            });
        }

        Ok(self.root.join(candidate))
    }

    fn relative_display(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    /// Reads a memory file, optionally from a 1-indexed line offset.
    pub async fn read(
        &self,
        path: Option<&str>,
        line_offset: usize,
        max_lines: Option<usize>,
        max_chars: usize,
    ) -> Result<ReadResponse, StoreError> {
        let target = self.resolve_scoped_path(path)?;
        let display = self.relative_display(&target);

        let metadata = match tokio::fs::metadata(&target).await {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err(StoreError::NotFound { path: display });
            }
            Err(err) => return Err(StoreError::Io(err)),
        };
        if metadata.file_type().is_symlink() {
            return Err(StoreError::NotFound { path: display });
        }
        if !metadata.is_file() {
            return Err(StoreError::NotFile { path: display });
        }

        let content = tokio::fs::read_to_string(&target).await?;
        let lines: Vec<&str> = content.lines().collect();
        let start_index = line_offset.saturating_sub(1);
        if start_index > lines.len() {
            return Err(StoreError::LineOffsetExceedsFileLength);
        }
        let end_index = max_lines
            .map(|count| start_index.saturating_add(count))
            .unwrap_or(lines.len())
            .min(lines.len());
        let selected = lines[start_index..end_index].join("\n");
        let (content, truncated) = truncate_chars(selected, max_chars);

        Ok(ReadResponse {
            path: display,
            start_line_number: start_index + 1,
            content,
            truncated,
        })
    }

    /// Substring-searches memory files under an optional scoped path.
    ///
    /// Matching is literal (never regex). A line matches when it contains any
    /// requested query.
    pub async fn search(
        &self,
        queries: &[String],
        path: Option<&str>,
        context_lines: usize,
        case_sensitive: bool,
        max_results: usize,
    ) -> Result<SearchResponse, StoreError> {
        let trimmed: Vec<String> = queries.iter().map(|q| q.trim().to_string()).collect();
        if trimmed.is_empty() || trimmed.iter().any(String::is_empty) {
            return Err(StoreError::EmptyQuery);
        }

        let start = self.resolve_scoped_path(path)?;
        let metadata = match tokio::fs::metadata(&start).await {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err(StoreError::NotFound {
                    path: path.unwrap_or_default().to_string(),
                });
            }
            Err(err) => return Err(StoreError::Io(err)),
        };

        let budget = max_results.clamp(1, MAX_SEARCH_MAX_RESULTS);
        let mut matches = Vec::new();
        let mut truncated = false;

        if metadata.is_file() {
            self.search_file(
                &start,
                &trimmed,
                context_lines,
                case_sensitive,
                &mut matches,
            )
            .await?;
        } else if metadata.is_dir() {
            // Iterative walk: avoids async recursion and never follows symlinks.
            let mut pending = vec![start];
            while let Some(dir) = pending.pop() {
                let mut entries = match tokio::fs::read_dir(&dir).await {
                    Ok(entries) => entries,
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(err) => return Err(StoreError::Io(err)),
                };
                while let Some(entry) = entries.next_entry().await? {
                    let entry_path = entry.path();
                    let Some(name) = entry_path.file_name().and_then(|name| name.to_str()) else {
                        continue;
                    };
                    if name.starts_with('.') {
                        continue;
                    }
                    let Ok(entry_metadata) = entry.metadata().await else {
                        continue;
                    };
                    if entry_metadata.file_type().is_symlink() {
                        continue;
                    }
                    if entry_metadata.is_dir() {
                        pending.push(entry_path);
                    } else if entry_metadata.is_file() {
                        self.search_file(
                            &entry_path,
                            &trimmed,
                            context_lines,
                            case_sensitive,
                            &mut matches,
                        )
                        .await?;
                    }
                }
            }
        }

        matches.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then(left.match_line_number.cmp(&right.match_line_number))
        });
        if matches.len() > budget {
            matches.truncate(budget);
            truncated = true;
        }

        Ok(SearchResponse {
            queries: trimmed,
            path: path.map(str::to_string),
            matches,
            truncated,
        })
    }

    async fn search_file(
        &self,
        path: &Path,
        queries: &[String],
        context_lines: usize,
        case_sensitive: bool,
        matches: &mut Vec<SearchMatch>,
    ) -> Result<(), StoreError> {
        let Ok(content) = tokio::fs::read_to_string(path).await else {
            // Unreadable / non-UTF-8 files are skipped rather than failing the
            // whole search.
            return Ok(());
        };
        let lines: Vec<&str> = content.lines().collect();
        let display = self.relative_display(path);
        let prepared: Vec<String> = queries
            .iter()
            .map(|query| prepare(query, case_sensitive))
            .collect();

        for (index, line) in lines.iter().enumerate() {
            let haystack = prepare(line, case_sensitive);
            let matched_queries: Vec<String> = prepared
                .iter()
                .zip(queries)
                .filter(|(needle, _)| haystack.contains(needle.as_str()))
                .map(|(_, original)| original.clone())
                .collect();
            if matched_queries.is_empty() {
                continue;
            }

            let content_start = index.saturating_sub(context_lines);
            let content_end = index
                .saturating_add(context_lines)
                .saturating_add(1)
                .min(lines.len());
            matches.push(SearchMatch {
                path: display.clone(),
                match_line_number: index + 1,
                content: lines[content_start..content_end].join("\n"),
                matched_queries,
            });
        }

        Ok(())
    }

    /// Appends a user-requested memory update note.
    ///
    /// Notes are the only write path: the tool never edits memory files
    /// directly, mirroring the Ody memory contract.
    pub async fn add_note(&self, filename: &str, note: &str) -> Result<(), StoreError> {
        validate_note_filename(filename)?;
        if note.trim().is_empty() {
            return Err(StoreError::EmptyNote);
        }

        let notes_dir = self.root.join("extensions").join("ad_hoc").join("notes");
        tokio::fs::create_dir_all(&notes_dir).await?;
        let path = notes_dir.join(filename);

        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await
        {
            Ok(mut file) => {
                use tokio::io::AsyncWriteExt;
                file.write_all(note.as_bytes()).await?;
                file.flush().await?;
                Ok(())
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(StoreError::NoteAlreadyExists {
                    filename: filename.to_string(),
                })
            }
            Err(err) => Err(StoreError::Io(err)),
        }
    }
}

/// Note filenames must be `<YYYY-MM-DDTHH-MM-SS>-<slug>.md`.
const NOTE_TIMESTAMP_PREFIX_LEN: usize = "YYYY-MM-DDTHH-MM-SS-".len();
const NOTE_SLUG_MAX_LEN: usize = 80;

fn validate_note_filename(filename: &str) -> Result<(), StoreError> {
    let Some(stem) = filename.strip_suffix(".md") else {
        return Err(StoreError::invalid_filename(filename, "must end with .md"));
    };
    let Some(slug) = stem.get(NOTE_TIMESTAMP_PREFIX_LEN..) else {
        return Err(StoreError::invalid_filename(
            filename,
            "must use YYYY-MM-DDTHH-MM-SS-<slug>.md",
        ));
    };
    if !has_valid_timestamp_prefix(stem) {
        return Err(StoreError::invalid_filename(
            filename,
            "must use YYYY-MM-DDTHH-MM-SS-<slug>.md",
        ));
    }
    if slug.is_empty() || slug.len() > NOTE_SLUG_MAX_LEN {
        return Err(StoreError::invalid_filename(
            filename,
            "slug must be 1 to 80 bytes",
        ));
    }
    if !slug
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(StoreError::invalid_filename(
            filename,
            "slug must contain only lowercase ASCII letters, digits, or hyphens",
        ));
    }
    if stem.contains('/') || stem.contains('\\') {
        return Err(StoreError::invalid_filename(
            filename,
            "must not contain path separators",
        ));
    }
    Ok(())
}

fn has_valid_timestamp_prefix(stem: &str) -> bool {
    let bytes = stem.as_bytes();
    bytes.len() > NOTE_TIMESTAMP_PREFIX_LEN
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b'-'
        && bytes[16] == b'-'
        && bytes[19] == b'-'
        && bytes[0..4].iter().all(u8::is_ascii_digit)
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[8..10].iter().all(u8::is_ascii_digit)
        && bytes[11..13].iter().all(u8::is_ascii_digit)
        && bytes[14..16].iter().all(u8::is_ascii_digit)
        && bytes[17..19].iter().all(u8::is_ascii_digit)
}

fn prepare(value: &str, case_sensitive: bool) -> String {
    if case_sensitive {
        value.to_string()
    } else {
        value.to_lowercase()
    }
}

fn truncate_chars(text: String, max_chars: usize) -> (String, bool) {
    if text.chars().count() <= max_chars {
        return (text, false);
    }
    (text.chars().take(max_chars).collect(), true)
}
