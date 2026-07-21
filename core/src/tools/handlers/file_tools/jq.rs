use super::PathAccessMode;
use super::local_search_root;
use super::pagination_notice;
use crate::function_tool::FunctionCallError;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::file_tools_spec::FileToolOptions;
use crate::tools::handlers::file_tools_spec::JQ_TOOL_NAME;
use crate::tools::handlers::file_tools_spec::MAX_BYTES;
use crate::tools::handlers::file_tools_spec::create_jq_tool;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use jaq_core::load::{Arena, File, Loader};
use jaq_core::{Compiler, Ctx, Vars};
use jaq_json::{Val, read, write};
use ody_protocol::models::ResponseInputItem;
use ody_tools::ToolName;
use ody_tools::ToolSpec;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use serde_json::json;
use std::path::Path;

/// Lines above this per call are an unreasonable context cost for a tool whose
/// whole point is avoiding shell `jq` dumps into the conversation.
const MAX_OUTPUT_LINES: usize = 1000;

#[derive(Deserialize)]
struct JqArgs {
    path: String,
    filter: String,
    #[serde(default)]
    offset: Option<i64>,
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    output_mode: Option<String>,
    #[serde(default)]
    environment_id: Option<String>,
    #[serde(default)]
    count: Option<bool>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OutputMode {
    Lines,
    Array,
}

struct JqOutput {
    content: String,
    truncated: bool,
}

impl ToolOutput for JqOutput {
    fn log_preview(&self) -> String {
        format!("{} lines", self.content.lines().count())
    }

    fn success_for_logging(&self) -> bool {
        true
    }

    fn to_response_item(&self, call_id: &str, payload: &ToolPayload) -> ResponseInputItem {
        FunctionToolOutput::from_text(self.content.clone(), Some(true))
            .to_response_item(call_id, payload)
    }

    fn code_mode_result(&self, _payload: &ToolPayload) -> JsonValue {
        json!({ "content": self.content, "truncated": self.truncated })
    }
}

#[derive(Default)]
pub struct JqHandler {
    options: FileToolOptions,
}

impl JqHandler {
    pub fn new(options: FileToolOptions) -> Self {
        Self { options }
    }
}

impl ToolExecutor<ToolInvocation> for JqHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(JQ_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_jq_tool(self.options)
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }

    fn handle(&self, invocation: ToolInvocation) -> ody_tools::ToolExecutorFuture<'_> {
        Box::pin(async move {
            let ToolInvocation { turn, payload, .. } = invocation;
            let ToolPayload::Function { arguments } = payload else {
                return Err(FunctionCallError::RespondToModel(format!(
                    "{JQ_TOOL_NAME} handler received unsupported payload"
                )));
            };
            let args: JqArgs = parse_arguments(&arguments)?;
            let abs_path = local_search_root(
                turn.as_ref(),
                args.environment_id.as_deref(),
                Some(&args.path),
                PathAccessMode::AbsoluteOutsideAllowed,
            )?;
            let (content, truncated) = run(&args, abs_path.as_path())?;
            Ok(boxed_tool_output(JqOutput { content, truncated }))
        })
    }
}

impl CoreToolRuntime for JqHandler {}

fn parse_output_mode(raw: Option<&str>) -> Result<OutputMode, FunctionCallError> {
    match raw {
        None | Some("lines") => Ok(OutputMode::Lines),
        Some("array") => Ok(OutputMode::Array),
        Some(other) => Err(FunctionCallError::RespondToModel(format!(
            "jq.output_mode must be `lines` or `array`, got `{other}`"
        ))),
    }
}

fn run(args: &JqArgs, abs_path: &Path) -> Result<(String, bool), FunctionCallError> {
    let mode = parse_output_mode(args.output_mode.as_deref())?;

    let bytes = std::fs::read(abs_path).map_err(|err| {
        FunctionCallError::RespondToModel(format!("unable to read `{}`: {err}", abs_path.display()))
    })?;

    let byte_capped = bytes.len() > MAX_BYTES;
    let slice = if byte_capped {
        &bytes[..floor_char_boundary(&bytes, MAX_BYTES)]
    } else {
        &bytes[..]
    };

    // Count is a separate operation: it streams through the whole file and returns
    // the number of input values, which for JSONL equals the line count. This lets
    // models discover file size before paging through it with offset/limit.
    if args.count == Some(true) {
        let mut count = 0usize;
        for result in read::parse_many(&bytes[..]) {
            result.map_err(|err| {
                FunctionCallError::RespondToModel(format!(
                    "`{}` is not valid JSON/JSONL: {err}",
                    abs_path.display()
                ))
            })?;
            count += 1;
        }
        return Ok((count.to_string(), false));
    }

    let program = File {
        code: args.filter.as_str(),
        path: (),
    };
    let defs = jaq_core::defs()
        .chain(jaq_std::defs())
        .chain(jaq_json::defs());
    let funs = jaq_core::funs()
        .chain(jaq_std::funs())
        .chain(jaq_json::funs());

    let loader = Loader::new(defs);
    let arena = Arena::default();
    let modules = loader.load(&arena, program).map_err(|err| {
        FunctionCallError::RespondToModel(format!(
            "jq filter `{}` is not valid: {:?}",
            args.filter, err
        ))
    })?;
    let filter = Compiler::default()
        .with_funs(funs)
        .compile(modules)
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!(
                "jq filter `{}` could not be compiled: {:?}",
                args.filter, err
            ))
        })?;

    let values = read::parse_many(slice)
        .collect::<Result<Vec<Val>, _>>()
        .map_err(|err| {
            if byte_capped {
                FunctionCallError::RespondToModel(format!(
                    "`{}` is larger than {} KiB and the first chunk is not valid JSON/JSONL; \
                 try a more specific filter or read a smaller file. Original error: {err}",
                    abs_path.display(),
                    MAX_BYTES / 1024
                ))
            } else {
                FunctionCallError::RespondToModel(format!(
                    "`{}` is not valid JSON/JSONL: {err}",
                    abs_path.display()
                ))
            }
        })?;

    let ctx = Ctx::<jaq_core::data::JustLut<Val>>::new(&filter.lut, Vars::new([]));
    let mut rows: Vec<String> = Vec::new();

    for input in values {
        for result in filter
            .id
            .run((ctx.clone(), input))
            .map(jaq_core::unwrap_valr)
        {
            let value = result.map_err(|err| {
                FunctionCallError::RespondToModel(format!(
                    "jq filter `{}` failed at runtime: {err}",
                    args.filter
                ))
            })?;
            rows.push(val_to_string(&value));
        }
    }

    let (content, truncated) = render(&rows, mode, args.offset, args.limit)?;
    Ok((content, truncated || byte_capped))
}

fn val_to_string(value: &Val) -> String {
    let mut buf = Vec::new();
    write::write(&mut buf, &write::Pp::default(), 0, value).unwrap();
    String::from_utf8(buf).unwrap()
}

fn render(
    rows: &[String],
    mode: OutputMode,
    offset: Option<i64>,
    limit: Option<i64>,
) -> Result<(String, bool), FunctionCallError> {
    let total = rows.len();
    let start = match offset {
        None => 0usize,
        Some(0) => {
            return Err(FunctionCallError::RespondToModel(
                "jq.offset is 1-based; use 1 for the first result, or omit to start at 1"
                    .to_string(),
            ));
        }
        Some(negative) if negative < 0 => {
            let back = negative.unsigned_abs() as usize;
            if back > MAX_OUTPUT_LINES {
                return Err(FunctionCallError::RespondToModel(format!(
                    "jq.offset cannot look further back than {MAX_OUTPUT_LINES} results"
                )));
            }
            total.saturating_sub(back)
        }
        Some(positive) => (positive as usize).saturating_sub(1),
    };

    let requested = limit
        .filter(|limit| *limit > 0)
        .map_or(MAX_OUTPUT_LINES, |limit| {
            (limit as usize).min(MAX_OUTPUT_LINES)
        });
    let end = start.saturating_add(requested).min(total);

    let window = &rows[start.min(total)..end];
    let mut content = if window.is_empty() {
        "No results produced by the filter.".to_string()
    } else if mode == OutputMode::Array {
        format!("[{}]", window.join(","))
    } else {
        window.join("\n")
    };

    let truncated = end < total;
    if let Some(notice) = pagination_notice(total, window.len(), start) {
        content.push_str(&notice);
    }

    Ok((content, truncated))
}

/// Largest index <= `max` that is a UTF-8 character boundary.
fn floor_char_boundary(bytes: &[u8], max: usize) -> usize {
    let mut index = max.min(bytes.len());
    while index > 0 && (bytes[index] & 0b1100_0000) == 0b1000_0000 {
        index -= 1;
    }
    index
}

#[cfg(test)]
pub(super) fn run_for_test(filter: &str, root: &Path) -> Result<(String, bool), FunctionCallError> {
    run(
        &JqArgs {
            path: root.to_string_lossy().to_string(),
            filter: filter.to_string(),
            offset: None,
            limit: None,
            output_mode: None,
            environment_id: None,
            count: None,
        },
        root,
    )
}

#[cfg(test)]
pub(super) fn run_with_options_for_test(
    filter: &str,
    root: &Path,
    offset: Option<i64>,
    limit: Option<i64>,
    output_mode: Option<&str>,
) -> Result<(String, bool), FunctionCallError> {
    run(
        &JqArgs {
            path: root.to_string_lossy().to_string(),
            filter: filter.to_string(),
            offset,
            limit,
            output_mode: output_mode.map(|s| s.to_string()),
            environment_id: None,
            count: None,
        },
        root,
    )
}

#[cfg(test)]
pub(super) fn run_with_count_for_test(path: &Path) -> Result<(String, bool), FunctionCallError> {
    run(
        &JqArgs {
            path: path.to_string_lossy().to_string(),
            filter: ".".to_string(),
            offset: None,
            limit: None,
            output_mode: None,
            environment_id: None,
            count: Some(true),
        },
        path,
    )
}
