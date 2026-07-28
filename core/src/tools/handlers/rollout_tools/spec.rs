//! Tool schemas for rollout-aware session-file exploration.
//!
//! These tools wrap `ody_rollout` primitives so the model can read, tail, and
//! search Ody's own `.jsonl` / `.jsonl.zst` session files without falling back
//! to raw shell commands. They are gated by the `rollout_tools` feature flag.

use ody_tools::JsonSchema;
use ody_tools::ResponsesApiTool;
use ody_tools::ToolSpec;
use serde_json::Value;
use serde_json::json;
use std::collections::BTreeMap;

pub const ROLLOUT_READ_TOOL_NAME: &str = "rollout_read";
pub const ROLLOUT_TAIL_TOOL_NAME: &str = "rollout_tail";
pub const ROLLOUT_SEARCH_TOOL_NAME: &str = "rollout_search";

/// Maximum records returned by a single `rollout_read` call.
pub const MAX_READ_RECORDS: usize = 10_000;
/// Maximum records returned by a single `rollout_tail` call.
pub const MAX_TAIL_RECORDS: usize = 1_000;
/// Default number of records returned by `rollout_tail`.
pub const DEFAULT_TAIL_RECORDS: usize = 50;
/// Maximum result rows returned by `rollout_search` before pagination kicks in.
pub const DEFAULT_SEARCH_LIMIT: usize = 250;
/// Maximum characters retained per record line; longer lines are truncated in place.
pub const MAX_RECORD_LINE_LENGTH: usize = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RolloutToolOptions {
    pub include_environment_id: bool,
}

fn environment_id_property(
    properties: &mut BTreeMap<String, JsonSchema>,
    options: RolloutToolOptions,
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

pub fn create_rollout_read_tool(options: RolloutToolOptions) -> ToolSpec {
    let mut properties = BTreeMap::from([
        (
            "path".to_string(),
            JsonSchema::string(Some(
                "Path to the rollout file to read. Absolute paths may point outside the working \
                 directory; relative paths are resolved against the Ody home directory (usually \
                 ~/.ody-code). Either `path` or `thread_id` must be provided."
                    .to_string(),
            )),
        ),
        (
            "thread_id".to_string(),
            JsonSchema::string(Some(
                "Thread/session UUID whose rollout file should be read. The tool searches the \
                 sessions directory under the Ody home. Either `path` or `thread_id` must be provided."
                    .to_string(),
            )),
        ),
        (
            "offset".to_string(),
            JsonSchema::integer(Some(format!(
                "1-based record (line) to start reading from. Omit to start at record 1. Negative \
                 values read from the end of the file (e.g. -50 reads the last 50 records); the \
                 absolute value cannot exceed {MAX_READ_RECORDS}."
            ))),
        ),
        (
            "limit".to_string(),
            JsonSchema::integer(Some(format!(
                "Number of records to read. Defaults to the internal cap of {MAX_READ_RECORDS}; \
                 values above it are clamped. Page through a large file with `offset` rather than \
                 raising this."
            ))),
        ),
    ]);
    environment_id_property(&mut properties, options);

    ToolSpec::Function(ResponsesApiTool {
        name: ROLLOUT_READ_TOOL_NAME.to_string(),
        description: format!(
            "Read Ody session rollout files (`.jsonl` or `.jsonl.zst`), returned as numbered \
             records. Unlike `read_file`, this tool is not capped at 100 KiB and can read \
             arbitrarily large session files line by line. Use it to inspect the full content of a \
             session file when `read_file` returns truncated or empty results. One of `path` or \
             `thread_id` is required."
        ),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, Some(vec![]), Some(false.into())),
        output_schema: Some(read_tool_output_schema()),
    })
}

pub fn create_rollout_tail_tool(options: RolloutToolOptions) -> ToolSpec {
    let mut properties = BTreeMap::from([
        (
            "path".to_string(),
            JsonSchema::string(Some(
                "Path to the rollout file to tail. Absolute paths may point outside the working \
                 directory; relative paths are resolved against the Ody home directory. Either \
                 `path` or `thread_id` must be provided."
                    .to_string(),
            )),
        ),
        (
            "thread_id".to_string(),
            JsonSchema::string(Some(
                "Thread/session UUID whose rollout file should be tailed. The tool searches the \
                 sessions directory under the Ody home. Either `path` or `thread_id` must be provided."
                    .to_string(),
            )),
        ),
        (
            "limit".to_string(),
            JsonSchema::integer(Some(format!(
                "Number of most recent records to return. Defaults to {DEFAULT_TAIL_RECORDS}; \
                 values above {MAX_TAIL_RECORDS} are clamped."
            ))),
        ),
    ]);
    environment_id_property(&mut properties, options);

    ToolSpec::Function(ResponsesApiTool {
        name: ROLLOUT_TAIL_TOOL_NAME.to_string(),
        description: format!(
            "Read the most recent records from an Ody session rollout file (`.jsonl` or \
             `.jsonl.zst`), returning them newest-first. This is the safe way to tail large or \
             compressed session files without resorting to `shell_command`. One of `path` or \
             `thread_id` is required."
        ),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(properties, Some(vec![]), Some(false.into())),
        output_schema: Some(read_tool_output_schema()),
    })
}

pub fn create_rollout_search_tool(options: RolloutToolOptions) -> ToolSpec {
    let mut properties = BTreeMap::from([
        (
            "search_term".to_string(),
            JsonSchema::string(Some(
                "Literal text to search for in session rollout files. Case-insensitive."
                    .to_string(),
            )),
        ),
        (
            "archived".to_string(),
            JsonSchema::boolean(Some(
                "Search archived sessions instead of active sessions. Defaults to false."
                    .to_string(),
            )),
        ),
        (
            "head_limit".to_string(),
            JsonSchema::integer(Some(format!(
                "Maximum number of matching rollout files to return. Defaults to \
                 {DEFAULT_SEARCH_LIMIT}; values above it are clamped."
            ))),
        ),
    ]);
    environment_id_property(&mut properties, options);

    ToolSpec::Function(ResponsesApiTool {
        name: ROLLOUT_SEARCH_TOOL_NAME.to_string(),
        description: format!(
            "Search Ody session rollout files (`.jsonl` or `.jsonl.zst`) for a literal text string. \
             Returns matching rollout paths and, for compressed files, a short content snippet. \
             This is the safe way to grep across session files without using `shell_command`."
        ),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            Some(vec!["search_term".to_string()]),
            Some(false.into()),
        ),
        output_schema: Some(search_tool_output_schema()),
    })
}

fn read_tool_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "content": {
                "type": "string",
                "description": "Numbered records read from the rollout file."
            },
            "truncated": {
                "type": "boolean",
                "description": "Whether additional records were omitted due to the limit."
            }
        },
        "required": ["content", "truncated"],
        "additionalProperties": false
    })
}

fn search_tool_output_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "matches": {
                "type": "string",
                "description": "Matching rollout paths and snippets, one per line."
            },
            "truncated": {
                "type": "boolean",
                "description": "Whether additional matches were omitted due to the limit."
            }
        },
        "required": ["matches", "truncated"],
        "additionalProperties": false
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec_json(spec: &ToolSpec) -> String {
        serde_json::to_string(spec).expect("ToolSpec serializes")
    }

    #[test]
    fn read_tool_requires_no_arguments_but_requires_path_or_thread_id_in_description() {
        let json = spec_json(&create_rollout_read_tool(RolloutToolOptions::default()));
        assert!(
            json.contains("path\""),
            "rollout_read schema should include path: {json}"
        );
        assert!(
            json.contains("thread_id\""),
            "rollout_read schema should include thread_id: {json}"
        );
    }

    #[test]
    fn tail_tool_has_limit_default() {
        let json = spec_json(&create_rollout_tail_tool(RolloutToolOptions::default()));
        assert!(
            json.contains("limit"),
            "rollout_tail schema should include limit: {json}"
        );
    }

    #[test]
    fn search_tool_requires_search_term() {
        let json = spec_json(&create_rollout_search_tool(RolloutToolOptions::default()));
        assert!(
            json.contains("\"required\":[\"search_term\"]"),
            "rollout_search should require search_term: {json}"
        );
    }
}
