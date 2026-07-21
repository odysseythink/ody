
//! Schemas for the structured file-exploration tools: `read_file`, `grep`, `glob`, `jq`.
//!
//! These exist to keep codebase exploration out of the raw-shell path. A model
//! that explores with `rg`/`cat` through `shell_command` dumps unshaped stdout
//! into the conversation; these tools shape the output *before* it reaches the
//! context.
//!
//! The defaults are deliberately frugal, and the descriptions state them,
//! because the default is what the model actually gets:
//!   - `grep` returns matching **file paths**, not their contents.
//!   - `read_file` pages: 1000 lines, 2000 chars/line, 100 KiB.
//!   - both cap result counts at 250 and offer `offset` to page further.

use ody_tools::JsonSchema;
use ody_tools::ResponsesApiTool;
use ody_tools::ToolSpec;
use serde_json::Value;
use serde_json::json;
use std::collections::BTreeMap;

pub const READ_FILE_TOOL_NAME: &str = "read_file";
pub const GREP_TOOL_NAME: &str = "grep";
pub const GLOB_TOOL_NAME: &str = "glob";
 pub const JQ_TOOL_NAME: &str = "jq";
 pub const WRITE_FILE_TOOL_NAME: &str = "write_file";
 pub const EDIT_FILE_TOOL_NAME: &str = "edit_file";

/// Maximum lines returned by a single `read_file` call.
pub const MAX_LINES: usize = 1000;
/// Maximum characters retained per line; longer lines are truncated in place.
pub const MAX_LINE_LENGTH: usize = 2000;
/// Maximum bytes read from a file in a single call.
pub const MAX_BYTES: usize = 100 * 1024 * 1024;
/// Maximum result rows returned by `grep`/`glob` before pagination kicks in.
pub const DEFAULT_HEAD_LIMIT: usize = 250;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FileToolOptions {
    pub include_environment_id: bool,
}

fn environment_id_property(
    properties: &mut BTreeMap<String, JsonSchema>,
    options: FileToolOptions,
) {
    if options.include_environment_id {
        properties.insert(
            "environment_id".to_string(),
            JsonSchema::string(Some(
                "Environment id from <environment_context>. Omit to use the primary environment."
                    .to_string(),
            )),
        );
    }
}

pub fn create_read_file_tool(options: FileToolOptions) -> ToolSpec {
    let mut properties = BTreeMap::from([
        (
            "path".to_string(),
            JsonSchema::string(Some(
                "Path to the file to read. Absolute paths may point outside the working directory; \
                 relative paths are resolved against it."
                    .to_string(),
            )),
        ),
        (
            "offset".to_string(),
            JsonSchema::integer(Some(format!(
                "1-based line to start reading from. Omit to start at line 1. Negative values read \
                 from the end of the file (e.g. -50 reads the last 50 lines); the absolute value \
                 cannot exceed {MAX_LINES}."
            ))),
        ),
        (
            "limit".to_string(),
            JsonSchema::integer(Some(format!(
                "Number of lines to read. Defaults to the internal cap of {MAX_LINES}; values above \
                 it are clamped. Page through a large file with `offset` rather than raising this."
            ))),
        ),
    ]);
    environment_id_property(&mut properties, options);

    ToolSpec::Function(ResponsesApiTool {
        name: READ_FILE_TOOL_NAME.to_string(),
        description: format!(
            "Read a file from the filesystem, returned as numbered lines. Prefer this over shell \
             `cat`/`sed`: it caps output at {MAX_LINES} lines, {MAX_LINE_LENGTH} characters per \
             line, and {} KiB, so a large file cannot flood the conversation. Use `offset`/`limit` \
             to read only the region you care about — locate it with `grep` first.",
            MAX_BYTES / 1024
        ),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            Some(vec!["path".to_string()]),
            Some(false.into()),
        ),
        output_schema: Some(read_file_output_schema()),
    })
}

pub fn create_grep_tool(options: FileToolOptions) -> ToolSpec {
    let mut properties = BTreeMap::from([
        (
            "pattern".to_string(),
            JsonSchema::string(Some(
                "Regular expression to search for (Rust regex syntax).".to_string(),
            )),
        ),
        (
            "path".to_string(),
            JsonSchema::string(Some(
                "File or directory to search in. Defaults to the working directory.".to_string(),
            )),
        ),
        (
            "glob".to_string(),
            JsonSchema::string(Some(
                "Restrict the search to files matching this glob (e.g. `**/*.rs`).".to_string(),
            )),
        ),
        (
            "output_mode".to_string(),
            JsonSchema::string_enum(
                vec![
                    json!("files_with_matches"),
                    json!("content"),
                    json!("count_matches"),
                ],
                Some(
                    "Shape of the result. `files_with_matches` (the DEFAULT) returns only the paths \
                     of files that contain a match — start here, it is by far the cheapest. \
                     `content` returns the matching lines themselves (honors `-n`/`-A`/`-B`/`-C`); \
                     ask for it only once you know which files matter. `count_matches` returns just \
                     the number of matches."
                        .to_string(),
                ),
            ),
        ),
        (
            "head_limit".to_string(),
            JsonSchema::integer(Some(format!(
                "Maximum rows to return. Defaults to {DEFAULT_HEAD_LIMIT}. Use `offset` to page \
                 through the rest rather than raising this."
            ))),
        ),
        (
            "offset".to_string(),
            JsonSchema::integer(Some(
                "Rows to skip before applying `head_limit`. Defaults to 0. Use with `head_limit` to \
                 page through a large result set."
                    .to_string(),
            )),
        ),
        (
            "-n".to_string(),
            JsonSchema::boolean(Some(
                "Prefix each matching line with its line number. Only applies when `output_mode` is \
                 `content`. Defaults to true."
                    .to_string(),
            )),
        ),
        (
            "-A".to_string(),
            JsonSchema::integer(Some(
                "Lines of context to show after each match. Only applies when `output_mode` is \
                 `content`."
                    .to_string(),
            )),
        ),
        (
            "-B".to_string(),
            JsonSchema::integer(Some(
                "Lines of context to show before each match. Only applies when `output_mode` is \
                 `content`."
                    .to_string(),
            )),
        ),
        (
            "-C".to_string(),
            JsonSchema::integer(Some(
                "Lines of context to show before and after each match. Only applies when \
                 `output_mode` is `content`; takes precedence over `-A` and `-B`."
                    .to_string(),
            )),
        ),
        (
            "-i".to_string(),
            JsonSchema::boolean(Some("Case-insensitive search.".to_string())),
        ),
    ]);
    environment_id_property(&mut properties, options);

    ToolSpec::Function(ResponsesApiTool {
        name: GREP_TOOL_NAME.to_string(),
        description:
            "Search file contents with a regular expression. Prefer this over shelling out to `rg`: \
             it returns matching FILE PATHS by default instead of every matching line, which is what \
             keeps a broad search from flooding the conversation. Typical flow: `grep` to find the \
             files, then `read_file` on the few that matter. Respects .gitignore."
                .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            Some(vec!["pattern".to_string()]),
            Some(false.into()),
        ),
        output_schema: Some(grep_output_schema()),
    })
}

pub fn create_glob_tool(options: FileToolOptions) -> ToolSpec {
    let mut properties = BTreeMap::from([
        (
            "pattern".to_string(),
            JsonSchema::string(Some(
                "Glob pattern to match files against (e.g. `**/*.rs`, `core/src/**/handlers/*.rs`). \
                 Must contain a literal anchor — an extension or a path segment. A pure wildcard \
                 (`*` or `**/*`) is rejected, because enumerating the whole tree is never the \
                 cheapest way to answer a question."
                    .to_string(),
            )),
        ),
        (
            "path".to_string(),
            JsonSchema::string(Some(
                "Directory to search in. Defaults to the working directory.".to_string(),
            )),
        ),
        (
            "head_limit".to_string(),
            JsonSchema::integer(Some(format!(
                "Maximum paths to return. Defaults to {DEFAULT_HEAD_LIMIT}."
            ))),
        ),
    ]);
    environment_id_property(&mut properties, options);

    ToolSpec::Function(ResponsesApiTool {
        name: GLOB_TOOL_NAME.to_string(),
        description:
            "Find files by glob pattern, newest first. Prefer this over shell `find`/`ls -R`. \
             Respects .gitignore."
                .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            Some(vec!["pattern".to_string()]),
            Some(false.into()),
        ),
        output_schema: Some(glob_output_schema()),
    })
}

pub fn create_jq_tool(options: FileToolOptions) -> ToolSpec {
    let mut properties = BTreeMap::from([
        (
            "path".to_string(),
            JsonSchema::string(Some(
                "Path to a JSON or JSONL file. Absolute paths may point outside the working \
                 directory; relative paths are resolved against it."
                    .to_string(),
            )),
        ),
        (
            "filter".to_string(),
            JsonSchema::string(Some(
                "jq filter expression applied to each JSON value in the file. For JSONL files, the filter runs once per line.".to_string(),
            )),
        ),
        (
            "output_mode".to_string(),
            JsonSchema::string_enum(
                vec![json!("lines"), json!("array")],
                Some(
                    "Shape of the result. `lines` (the DEFAULT) returns one JSON value per line. \
                     `array` returns all results wrapped in a JSON array."
                        .to_string(),
                ),
            ),
        ),
        (
            "offset".to_string(),
            JsonSchema::integer(Some(
                "1-based result to start from. Omit to start at 1. Negative values read from the \
                 end of the result set."
                    .to_string(),
            )),
        ),
        (
            "limit".to_string(),
            JsonSchema::integer(Some(
                "Maximum number of results to return. Defaults to an internal cap. Page through \
                 large results with `offset` rather than raising this."
                    .to_string(),
            )),
        ),
        (
            "count".to_string(),
            JsonSchema::boolean(Some(
                "Return the number of input values in the file instead of running the filter. \
                 For JSONL files this equals the line count. The filter is ignored when this is true."
                    .to_string(),
            )),
        ),
    ]);
    environment_id_property(&mut properties, options);

    ToolSpec::Function(ResponsesApiTool {
        name: JQ_TOOL_NAME.to_string(),
        description:
            "Run a jq filter against a JSON or JSONL file. Prefer this over `read_file` for \
             JSON/JSONL files when you want to filter, count, or page through structured data. \
             Example filters: `.` to pretty-print, `select(.type == \"function_call\")` to filter \
             objects, `.field` to extract a field, `length` to count. Output is capped and shaped \
             before it reaches the conversation."
                .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            Some(vec!["path".to_string(), "filter".to_string()]),
            Some(false.into()),
        ),
        output_schema: Some(jq_output_schema()),
    })
}

fn jq_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "content": {
                "type": "string",
                "description": "The jq results, either newline-separated JSON values (`lines` mode) or a JSON array (`array` mode)."
            },
            "truncated": {
                "type": "boolean",
                "description": "True when more results were available than returned; page on with `offset`."
            }
        },
        "required": ["content", "truncated"],
        "additionalProperties": false
    })
}

fn read_file_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "content": {
                "type": "string",
                "description": "The requested lines, each prefixed with its 1-based line number."
            },
            "truncated": {
                "type": "boolean",
                "description": "True when the file had more lines than were returned; page on with `offset`."
            }
        },
        "required": ["content", "truncated"],
        "additionalProperties": false
    })
}

fn grep_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "matches": {
                "type": "string",
                "description": "File paths (default), matching lines (`content`), or a count (`count_matches`)."
            },
            "truncated": {
                "type": "boolean",
                "description": "True when more rows matched than were returned; page on with `offset`."
            }
        },
        "required": ["matches", "truncated"],
        "additionalProperties": false
    })
}

fn glob_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "paths": {
                "type": "string",
                "description": "Newline-separated matching paths, newest first."
            },
            "truncated": {
                "type": "boolean",
                "description": "True when more files matched than were returned."
            }
        },
        "required": ["paths", "truncated"],
        "additionalProperties": false
    })
}


fn write_file_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "success": {
                "type": "boolean",
                "description": "True when the file was written successfully."
            },
            "bytes_written": {
                "type": "integer",
                "description": "Number of bytes written to the file."
            }
        },
        "required": ["success", "bytes_written"],
        "additionalProperties": false
    })
}

fn edit_file_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "success": {
                "type": "boolean",
                "description": "True when the edit was applied successfully."
            },
            "replacements": {
                "type": "integer",
                "description": "Number of occurrences of old_string that were replaced."
            }
        },
        "required": ["success", "replacements"],
        "additionalProperties": false
    })
}

pub fn create_write_file_tool(options: FileToolOptions) -> ToolSpec {
    let mut properties = BTreeMap::from([
        (
            "path".to_string(),
            JsonSchema::string(Some(
                "Path to the file to write. Relative paths are resolved against the working \
                 directory; absolute paths may point outside it but must be inside the workspace."
                    .to_string(),
            )),
        ),
        (
            "content".to_string(),
            JsonSchema::string(Some(
                "The full content to write to the file. Existing content is overwritten unless \
                 `append` is true.".to_string(),
            )),
        ),
        (
            "append".to_string(),
            JsonSchema::boolean(Some(
                "If true, append `content` to the end of the file instead of overwriting it. \
                 Defaults to false.".to_string(),
            )),
        ),
    ]);
    environment_id_property(&mut properties, options);

    ToolSpec::Function(ResponsesApiTool {
        name: WRITE_FILE_TOOL_NAME.to_string(),
        description:
            "Write content to a file. Use this to create a new file or overwrite an existing \
             file. For surgical edits to an existing file, prefer `edit_file` or `apply_patch`. \
             The parent directory is created automatically if it does not exist."
                .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            Some(vec!["path".to_string(), "content".to_string()]),
            Some(false.into()),
        ),
        output_schema: Some(write_file_output_schema()),
    })
}

pub fn create_edit_file_tool(options: FileToolOptions) -> ToolSpec {
    let mut properties = BTreeMap::from([
        (
            "path".to_string(),
            JsonSchema::string(Some(
                "Path to the file to edit. Relative paths are resolved against the working \
                 directory; absolute paths may point outside it but must be inside the workspace."
                    .to_string(),
            )),
        ),
        (
            "old_string".to_string(),
            JsonSchema::string(Some(
                "The exact existing string to replace. Must match at least once.".to_string(),
            )),
        ),
        (
            "new_string".to_string(),
            JsonSchema::string(Some("The string to put in place of `old_string`.".to_string())),
        ),
        (
            "replace_all".to_string(),
            JsonSchema::boolean(Some(
                "If true, replace every occurrence of `old_string`; otherwise replace only the \
                 first. Defaults to false.".to_string(),
            )),
        ),
    ]);
    environment_id_property(&mut properties, options);

    ToolSpec::Function(ResponsesApiTool {
        name: EDIT_FILE_TOOL_NAME.to_string(),
        description:
            "Edit a file by replacing a specific string. Use this for small, localized changes \
             where `old_string` is unique enough to locate unambiguously. For multi-hunk or \
             complex edits, use `apply_patch`."
                .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            Some(vec![
                "path".to_string(),
                "old_string".to_string(),
                "new_string".to_string(),
            ]),
            Some(false.into()),
        ),
        output_schema: Some(edit_file_output_schema()),
    })
}
#[cfg(test)]
mod tests {
    use super::*;

    fn spec_json(spec: &ToolSpec) -> String {
        serde_json::to_string(spec).expect("tool spec serializes")
    }

    /// The single most important property of this whole change: a `grep` that
    /// returns matching *lines* by default is just `rg` with extra steps, and
    /// costs the same context. The cheap shape has to be what the model gets
    /// when it does not think about it.
    #[test]
    fn grep_defaults_to_paths_not_contents() {
        let json = spec_json(&create_grep_tool(FileToolOptions::default()));
        assert!(
            json.contains("`files_with_matches` (the DEFAULT)"),
            "grep must advertise files_with_matches as its default output mode: {json}"
        );
        assert!(
            json.contains("matching FILE PATHS by default"),
            "grep's description must tell the model it gets paths, not contents: {json}"
        );
    }

    /// `read_file` is only cheaper than `cat` because of these caps. If the
    /// numbers stop appearing in the description, the model has no reason to
    /// page instead of asking for the whole file.
    #[test]
    fn read_file_states_its_caps() {
        let json = spec_json(&create_read_file_tool(FileToolOptions::default()));
        for expected in ["1000", "2000", "100 KiB"] {
            assert!(
                json.contains(expected),
                "read_file must state its {expected} cap so the model pages instead of dumping: {json}"
            );
        }
    }

    #[test]
    fn glob_rejects_pure_wildcards_in_its_contract() {
        let json = spec_json(&create_glob_tool(FileToolOptions::default()));
        assert!(
            json.contains("pure wildcard"),
            "glob must tell the model that `*`/`**/*` is rejected: {json}"
        );
    }

    #[test]
    fn environment_id_is_opt_in() {
        let without = spec_json(&create_read_file_tool(FileToolOptions::default()));
        assert!(!without.contains("environment_id"));
        let with = spec_json(&create_read_file_tool(FileToolOptions {
            include_environment_id: true,
        }));
        assert!(with.contains("environment_id"));
    }

    #[test]
    fn jq_prefers_itself_for_json_files() {
        let json = spec_json(&create_jq_tool(FileToolOptions::default()));
        assert!(
            !json.contains("Use `read_file` first"),
            "jq should not tell models to use read_file first; it should be the default for JSON/JSONL: {json}"
        );
        assert!(
            json.contains("Prefer this over `read_file`"),
            "jq description should encourage using jq over read_file for JSON/JSONL: {json}"
        );
        for example in ["`select(.type == \\\"function_call\\\")`", "`.field`", "`length`"] {
            assert!(
                json.contains(example),
                "jq description should include concrete example filter `{example}`: {json}"
            );
        }
    }

    #[test]
    fn tools_require_their_primary_argument() {
        for (spec, required) in [
            (create_read_file_tool(FileToolOptions::default()), "path"),
            (create_grep_tool(FileToolOptions::default()), "pattern"),
            (create_glob_tool(FileToolOptions::default()), "pattern"),
        ] {
            let json = spec_json(&spec);
            assert!(
                json.contains(&format!("\"required\":[\"{required}\"]")),
                "expected `{required}` to be the required argument: {json}"
            );
        }
        let jq_json = spec_json(&create_jq_tool(FileToolOptions::default()));
        assert!(
            jq_json.contains("\"required\":[\"path\",\"filter\"]")
                || jq_json.contains("\"required\":[\"filter\",\"path\"]"),
            "expected jq to require both path and filter: {jq_json}"
        );
    }

    #[test]
    fn write_file_requires_path_and_content() {
        let json = spec_json(&create_write_file_tool(FileToolOptions::default()));
        assert!(
            json.contains("\"required\":[\"path\",\"content\"]"),
            "write_file must require path and content: {json}"
        );
        assert!(json.contains("append"), "write_file must expose append: {json}");
    }

    #[test]
    fn edit_file_requires_path_old_and_new_string() {
        let json = spec_json(&create_edit_file_tool(FileToolOptions::default()));
        assert!(
            json.contains("\"required\":[\"old_string\",\"new_string\",\"path\"]")
                || json.contains("\"required\":[\"path\",\"old_string\",\"new_string\"]")
                || json.contains("\"required\":[\"path\",\"new_string\",\"old_string\"]")
                || json.contains("\"required\":[\"old_string\",\"path\",\"new_string\"]")
                || json.contains("\"required\":[\"new_string\",\"path\",\"old_string\"]")
                || json.contains("\"required\":[\"new_string\",\"old_string\",\"path\"]"),
            "edit_file must require path, old_string and new_string: {json}"
        );
        assert!(json.contains("replace_all"), "edit_file must expose replace_all: {json}");
    }

    #[test]
    fn write_file_and_edit_file_opt_in_environment_id() {
        let without = spec_json(&create_write_file_tool(FileToolOptions::default()));
        assert!(!without.contains("environment_id"));
        let with = spec_json(&create_write_file_tool(FileToolOptions {
            include_environment_id: true,
        }));
        assert!(with.contains("environment_id"));

        let without = spec_json(&create_edit_file_tool(FileToolOptions::default()));
        assert!(!without.contains("environment_id"));
        let with = spec_json(&create_edit_file_tool(FileToolOptions {
            include_environment_id: true,
        }));
        assert!(with.contains("environment_id"));
    }

    #[test]
    fn write_file_prefers_surgical_tools_in_description() {
        let json = spec_json(&create_write_file_tool(FileToolOptions::default()));
        assert!(
            json.contains("prefer `edit_file` or `apply_patch`"),
            "write_file description should guide models toward surgical tools: {json}"
        );
    }

    #[test]
    fn edit_file_prefers_apply_patch_for_complex_edits() {
        let json = spec_json(&create_edit_file_tool(FileToolOptions::default()));
        assert!(
            json.contains("use `apply_patch`"),
            "edit_file description should guide models toward apply_patch for complex edits: {json}"
        );
    }
}
