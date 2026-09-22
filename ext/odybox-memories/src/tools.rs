//! Assistant memory tools exposed to odyBox threads.
//!
//! Modelled on `ody_memories_extension::tools`, but self-contained: these
//! tools read and write the assistant memory root only, and share no types with
//! the Ody memory tool set.

use std::sync::Arc;

use ody_extension_api::FunctionCallError;
use ody_extension_api::JsonToolOutput;
use ody_extension_api::ResponsesApiTool;
use ody_extension_api::ToolCall;
use ody_extension_api::ToolExecutor;
use ody_extension_api::ToolExecutorFuture;
use ody_extension_api::ToolName;
use ody_extension_api::ToolSpec;
use ody_extension_api::parse_tool_input_schema;
use ody_tools::ResponsesApiNamespace;
use ody_tools::ResponsesApiNamespaceTool;
use ody_tools::default_namespace_description;
use schemars::JsonSchema;
use schemars::r#gen::SchemaSettings;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Map;
use serde_json::Value;
use serde_json::json;

use crate::backend::AssistantMemoryStore;
use crate::backend::DEFAULT_READ_MAX_CHARS;
use crate::backend::DEFAULT_SEARCH_MAX_RESULTS;
use crate::backend::ReadResponse;
use crate::backend::SearchResponse;
use crate::backend::StoreError;

/// Tool namespace. Deliberately distinct from the Ody memory namespace so the
/// two tool sets can never collide, even if both are mounted on one thread.
pub const TOOLS_NAMESPACE: &str = "odybox_memories";

pub const READ_TOOL_NAME: &str = "read";
pub const SEARCH_TOOL_NAME: &str = "search";
pub const ADD_NOTE_TOOL_NAME: &str = "add_note";

/// Empty payload for write-only tools.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct EmptyResponse {}

fn tool_name(name: &str) -> ToolName {
    ToolName::namespaced(TOOLS_NAMESPACE, name)
}

fn function_tool<I: JsonSchema, O: JsonSchema>(name: &str, description: &str) -> ToolSpec {
    let tool = ResponsesApiTool {
        name: name.to_string(),
        description: description.to_string(),
        strict: false,
        defer_loading: None,
        parameters: parse_tool_input_schema(&input_schema_for::<I>())
            .unwrap_or_else(|err| panic!("generated input schema for {name} should parse: {err}")),
        output_schema: Some(output_schema_for::<O>()),
    };

    ToolSpec::Namespace(ResponsesApiNamespace {
        name: TOOLS_NAMESPACE.to_string(),
        description: default_namespace_description(TOOLS_NAMESPACE),
        tools: vec![ResponsesApiNamespaceTool::Function(tool)],
    })
}

fn input_schema_for<T: JsonSchema>() -> Value {
    schema_for::<T>(/*option_add_null_type*/ false)
}

fn output_schema_for<T: JsonSchema>() -> Value {
    schema_for::<T>(/*option_add_null_type*/ true)
}

fn schema_for<T: JsonSchema>(option_add_null_type: bool) -> Value {
    let schema = SchemaSettings::draft2019_09()
        .with(|settings| {
            settings.inline_subschemas = true;
            settings.option_add_null_type = option_add_null_type;
        })
        .into_generator()
        .into_root_schema_for::<T>();
    let schema_value = serde_json::to_value(schema)
        .unwrap_or_else(|err| panic!("generated tool schema should serialize: {err}"));
    let Value::Object(mut schema_object) = schema_value else {
        panic!("root tool schema must be an object");
    };

    let mut tool_schema = Map::new();
    for key in [
        "properties",
        "required",
        "type",
        "additionalProperties",
        "$defs",
        "definitions",
    ] {
        if let Some(value) = schema_object.remove(key) {
            tool_schema.insert(key.to_string(), value);
        }
    }
    Value::Object(tool_schema)
}

fn parse_args<T: for<'de> Deserialize<'de>>(call: &ToolCall) -> Result<T, FunctionCallError> {
    let arguments = call.function_arguments()?;
    let value = if arguments.trim().is_empty() {
        Value::Object(Map::new())
    } else {
        serde_json::from_str(arguments)
            .map_err(|err| FunctionCallError::RespondToModel(err.to_string()))?
    };
    serde_json::from_value(value).map_err(|err| FunctionCallError::RespondToModel(err.to_string()))
}

fn to_function_call_error(err: StoreError) -> FunctionCallError {
    match err {
        StoreError::Io(io_error) => FunctionCallError::Fatal(io_error.to_string()),
        other => FunctionCallError::RespondToModel(other.to_string()),
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
    /// Path relative to the assistant memory root. Defaults to the root itself.
    path: Option<String>,
    /// 1-indexed line to start from.
    #[schemars(range(min = 1))]
    line_offset: Option<usize>,
    /// Maximum number of lines to return.
    #[schemars(range(min = 1))]
    max_lines: Option<usize>,
}

/// Reads a file from the odyBox assistant memory folder.
#[derive(Clone)]
pub struct ReadTool {
    store: AssistantMemoryStore,
}

impl ReadTool {
    pub fn new(store: AssistantMemoryStore) -> Self {
        Self { store }
    }

    async fn handle_call(
        &self,
        call: ToolCall,
    ) -> Result<Box<dyn ody_extension_api::ToolOutput>, FunctionCallError> {
        let args: ReadArgs = parse_args(&call)?;
        let response = self
            .store
            .read(
                args.path.as_deref(),
                args.line_offset.unwrap_or(1),
                args.max_lines,
                DEFAULT_READ_MAX_CHARS,
            )
            .await
            .map_err(to_function_call_error)?;
        Ok(Box::new(JsonToolOutput::new(json!(response))))
    }
}

impl ToolExecutor<ToolCall> for ReadTool {
    fn tool_name(&self) -> ToolName {
        tool_name(READ_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        function_tool::<ReadArgs, ReadResponse>(
            READ_TOOL_NAME,
            "Read a file from the odyBox assistant memory folder by relative path, optionally starting at a 1-indexed line offset and limiting the number of lines returned.",
        )
    }

    fn handle(&self, call: ToolCall) -> ToolExecutorFuture<'_> {
        Box::pin(self.handle_call(call))
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    /// Literal substrings to look for. A line matches when it contains any of them.
    queries: Vec<String>,
    /// Optional path relative to the assistant memory root to scope the search.
    path: Option<String>,
    /// Lines of context to include around each match.
    context_lines: Option<usize>,
    /// Whether matching is case sensitive. Defaults to false.
    case_sensitive: Option<bool>,
    /// Maximum number of matches to return.
    #[schemars(range(min = 1))]
    max_results: Option<usize>,
}

/// Substring-searches the odyBox assistant memory folder.
#[derive(Clone)]
pub struct SearchTool {
    store: AssistantMemoryStore,
}

impl SearchTool {
    pub fn new(store: AssistantMemoryStore) -> Self {
        Self { store }
    }

    async fn handle_call(
        &self,
        call: ToolCall,
    ) -> Result<Box<dyn ody_extension_api::ToolOutput>, FunctionCallError> {
        let args: SearchArgs = parse_args(&call)?;
        let response = self
            .store
            .search(
                &args.queries,
                args.path.as_deref(),
                args.context_lines.unwrap_or(0),
                args.case_sensitive.unwrap_or(false),
                args.max_results.unwrap_or(DEFAULT_SEARCH_MAX_RESULTS),
            )
            .await
            .map_err(to_function_call_error)?;
        Ok(Box::new(JsonToolOutput::new(json!(response))))
    }
}

impl ToolExecutor<ToolCall> for SearchTool {
    fn tool_name(&self) -> ToolName {
        tool_name(SEARCH_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        function_tool::<SearchArgs, SearchResponse>(
            SEARCH_TOOL_NAME,
            "Search the odyBox assistant memory folder for literal substrings. Matching is literal, never regex. Results are grouped per matching line.",
        )
    }

    fn handle(&self, call: ToolCall) -> ToolExecutorFuture<'_> {
        Box::pin(self.handle_call(call))
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddNoteArgs {
    /// Filename in the form YYYY-MM-DDTHH-MM-SS-slug.md (lowercase letters, digits, hyphens).
    filename: String,
    /// The memory update to record. Written verbatim as one note file.
    note: String,
}

/// Records a user-requested assistant memory update as a note file.
///
/// This is the only write path: memory files are never edited directly.
#[derive(Clone)]
pub struct AddNoteTool {
    store: AssistantMemoryStore,
}

impl AddNoteTool {
    pub fn new(store: AssistantMemoryStore) -> Self {
        Self { store }
    }

    async fn handle_call(
        &self,
        call: ToolCall,
    ) -> Result<Box<dyn ody_extension_api::ToolOutput>, FunctionCallError> {
        let args: AddNoteArgs = parse_args(&call)?;
        self.store
            .add_note(&args.filename, &args.note)
            .await
            .map_err(to_function_call_error)?;
        Ok(Box::new(JsonToolOutput::new(json!(EmptyResponse {}))))
    }
}

impl ToolExecutor<ToolCall> for AddNoteTool {
    fn tool_name(&self) -> ToolName {
        tool_name(ADD_NOTE_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        function_tool::<AddNoteArgs, EmptyResponse>(
            ADD_NOTE_TOOL_NAME,
            "Record a requested update to the odyBox assistant memory folder. Writes one small note file under extensions/ad_hoc/notes/; it does not edit memory files directly.",
        )
    }

    fn handle(&self, call: ToolCall) -> ToolExecutorFuture<'_> {
        Box::pin(self.handle_call(call))
    }
}

/// Builds the assistant memory tool set bound to `store`.
pub fn assistant_memory_tools(store: AssistantMemoryStore) -> Vec<Arc<dyn ToolExecutor<ToolCall>>> {
    vec![
        Arc::new(AddNoteTool::new(store.clone())),
        Arc::new(ReadTool::new(store.clone())),
        Arc::new(SearchTool::new(store)),
    ]
}
