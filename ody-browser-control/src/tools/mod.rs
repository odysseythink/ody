use std::path::{Component, Path};
use std::sync::Arc;

use ody_file_system::{CreateDirectoryOptions, ExecutorFileSystem, FileSystemSandboxContext};
use ody_tools::{
    FunctionCallError, JsonToolOutput, ToolCall, ToolExecutor, ToolExecutorFuture, ToolExposure,
    ToolName, ToolOutput, ToolSpec, default_namespace_description,
    parse_tool_input_schema, ResponsesApiNamespace, ResponsesApiNamespaceTool, ResponsesApiTool,
};
use ody_utils_path_uri::PathUri;
use serde_json::json;

use crate::{
    BrowserControlApprovalTicket, BrowserControlError, BrowserControlMode, BrowserThreadState,
    types::{
        LogKind, LogLevel, Point, ScreenshotFile, ViewportOverride, WaitCondition,
    },
};

/// Trait marker for browser tools. The only per-type information needed is the
/// shared [`BrowserThreadState`]. All schema/input/output helpers are free
/// functions so they can be used without a concrete `Self` type.
pub trait BrowserTool: Send + Sync {
    fn state(&self) -> &Arc<BrowserThreadState>;
}

fn parse_args<T: serde::de::DeserializeOwned>(call: &ToolCall) -> Result<T, FunctionCallError> {
    let arguments = call
        .function_arguments()
        .map_err(|e| FunctionCallError::Fatal(format!("failed to read tool arguments: {e}")))?;
    serde_json::from_str(arguments)
        .map_err(|e| FunctionCallError::Fatal(format!("invalid tool arguments: {e}")))
}

fn wrap_output(
    result: serde_json::Value,
    action: &str,
    input: serde_json::Value,
) -> Box<dyn ToolOutput> {
    let ticket = BrowserControlApprovalTicket {
        action: action.to_string(),
        details: input,
    };
    let value = json!({
        "result": result,
        "approval_ticket": ticket,
    });
    Box::new(JsonToolOutput::new(value))
}

fn browser_action_name(tool_name: &ToolName) -> Option<&'static str> {
    if tool_name.namespace.as_deref() != Some("browser") {
        return None;
    }
    match tool_name.name.as_str() {
        "navigate" => Some("navigate"),
        "go_back" => Some("go_back"),
        "go_forward" => Some("go_forward"),
        "reload" => Some("reload"),
        "click" => Some("click"),
        "type" => Some("type_text"),
        "evaluate" => Some("evaluate"),
        "execute_raw_cdp" => Some("execute_raw_cdp"),
        _ => None,
    }
}

fn ensure_browser_approved(call: &ToolCall) -> Result<(), FunctionCallError> {
    let Some(action) = browser_action_name(&call.tool_name) else {
        return Ok(());
    };
    if call.guardian_approved_action_id.is_some() {
        return Ok(());
    }
    let arguments = call.function_arguments().map_err(|e| {
        FunctionCallError::Fatal(format!("failed to read browser tool arguments: {e}"))
    })?;
    let mut details: serde_json::Value = serde_json::from_str(arguments)
        .unwrap_or_else(|_| serde_json::Value::String(arguments.to_string()));

    // For evaluate, replace the full expression with a truncated preview so the
    // approval ticket and any logs do not leak the entire script body.
    if action == "evaluate" {
        if let Some(expr) = details.get("expression").and_then(serde_json::Value::as_str) {
            let expr = expr.to_string();
            let preview = crate::types::truncate_string_bytes(&expr, 500);
            if let Some(obj) = details.as_object_mut() {
                obj.insert("expression".to_string(), serde_json::Value::String(preview));
                obj.insert(
                    "expression_truncated".to_string(),
                    serde_json::Value::Bool(expr.len() > 500),
                );
            }
        }
    }

    let ticket = BrowserControlApprovalTicket {
        action: action.to_string(),
        details,
    };
    tracing::info!(action, "browser tool requires guardian approval");
    Err(FunctionCallError::NeedsApproval {
        ticket: serde_json::to_value(ticket).unwrap_or_else(|_| serde_json::Value::Null),
    })
}

fn namespaced_tool_name(name: &str) -> ToolName {
    ToolName::namespaced("browser", name)
}

/// The `wait_until` values the model is told about.
///
/// Derived from [`WaitCondition`] rather than written out: the hand-written list
/// advertised `networkidle` while serde only accepted `network_idle`, so a model
/// that followed the schema was rejected on every retry of an otherwise correct
/// call.
fn wait_condition_names() -> Vec<String> {
    [
        WaitCondition::Load,
        WaitCondition::DomContentLoaded,
        WaitCondition::NetworkIdle,
    ]
    .into_iter()
    .filter_map(|condition| serde_json::to_value(condition).ok())
    .filter_map(|value| value.as_str().map(str::to_string))
    .collect()
}

fn namespace_spec(name: &str, description: &str, parameters: serde_json::Value) -> ToolSpec {
    let tool = ResponsesApiTool {
        name: name.to_string(),
        description: description.to_string(),
        strict: true,
        parameters: parse_tool_input_schema(&parameters)
            .unwrap_or_else(|e| panic!("built-in browser tool {name} schema is valid: {e}")),
        defer_loading: None,
        output_schema: None,
    };
    ToolSpec::Namespace(ResponsesApiNamespace {
        name: "browser".to_string(),
        description: default_namespace_description("browser"),
        tools: vec![ResponsesApiNamespaceTool::Function(tool)],
    })
}

fn map_error(e: BrowserControlError) -> FunctionCallError {
    e.to_function_call_error()
}

fn check_external_sensitive(
    state: &BrowserThreadState,
    action: &str,
) -> Result<(), FunctionCallError> {
    if state.config().mode == BrowserControlMode::External
        && !state.config().external_browser_allow_sensitive
    {
        return Err(FunctionCallError::Fatal(format!(
            "{action} is disabled in external browser mode unless external_browser_allow_sensitive is enabled"
        )));
    }
    Ok(())
}

/// Navigate the browser to a URL.
pub struct BrowserNavigateTool {
    state: Arc<BrowserThreadState>,
}

impl BrowserNavigateTool {
    pub fn new(state: Arc<BrowserThreadState>) -> Self {
        Self { state }
    }
}

impl BrowserTool for BrowserNavigateTool {
    fn state(&self) -> &Arc<BrowserThreadState> {
        &self.state
    }
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct BrowserNavigateInput {
    url: String,
    #[serde(default)]
    wait_until: Option<WaitCondition>,
}

impl ToolExecutor<ToolCall> for BrowserNavigateTool {
    fn tool_name(&self) -> ToolName {
        namespaced_tool_name("navigate")
    }

    fn spec(&self) -> ToolSpec {
        namespace_spec(
            "navigate",
            "Navigate the browser to the given URL.",
            json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "URL to navigate to." },
                    "wait_until": {
                        "type": "string",
                        "enum": wait_condition_names(),
                        "description": "When the navigation should be considered complete."
                    }
                },
                "required": ["url"],
                "additionalProperties": false
            }),
        )
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Direct
    }

    #[tracing::instrument(skip_all, fields(tool_name = ?call.tool_name, call_id = %call.call_id, turn_id = %call.turn_id, model = %call.model, approved = call.guardian_approved_action_id.is_some()))]
    fn handle(&self, call: ToolCall) -> ToolExecutorFuture<'_> {
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            let input: BrowserNavigateInput = parse_args(&call)?;
            let cwd = call.environments.first().map(|e| e.cwd.as_ref());
            if let Some(reason) =
                crate::approval_exemption::is_approval_exempt(&input.url, cwd, cfg!(test))
            {
                tracing::info!(
                    tool = "browser__navigate",
                    auto_approved = true,
                    url_preview = %crate::types::truncate_string_bytes(&input.url, 120),
                    reason = reason,
                    "browser navigate auto-approved by exemption"
                );
            } else {
                ensure_browser_approved(&call)?;
            }
            tracing::info!(
                url_preview = %crate::types::truncate_string_bytes(&input.url, 120),
                wait_until = ?input.wait_until,
                "browser navigate proceeding"
            );
            let result = state
                .navigate(&input.url, input.wait_until)
                .await
                .map_err(map_error)?;
            Ok(wrap_output(
                serde_json::to_value(result).map_err(|e| FunctionCallError::Fatal(e.to_string()))?,
                "navigate",
                serde_json::to_value(input).map_err(|e| FunctionCallError::Fatal(e.to_string()))?,
            ))
        })
    }
}

/// Go back in the browser history.
pub struct BrowserGoBackTool {
    state: Arc<BrowserThreadState>,
}

impl BrowserGoBackTool {
    pub fn new(state: Arc<BrowserThreadState>) -> Self {
        Self { state }
    }
}

impl BrowserTool for BrowserGoBackTool {
    fn state(&self) -> &Arc<BrowserThreadState> {
        &self.state
    }
}

impl ToolExecutor<ToolCall> for BrowserGoBackTool {
    fn tool_name(&self) -> ToolName {
        namespaced_tool_name("go_back")
    }

    fn spec(&self) -> ToolSpec {
        namespace_spec(
            "go_back",
            "Go back one step in the browser history.",
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        )
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Direct
    }

    #[tracing::instrument(skip_all, fields(tool_name = ?call.tool_name, call_id = %call.call_id, turn_id = %call.turn_id, model = %call.model, approved = call.guardian_approved_action_id.is_some()))]
    fn handle(&self, call: ToolCall) -> ToolExecutorFuture<'_> {
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            ensure_browser_approved(&call)?;
            let result = state.go_back().await.map_err(map_error)?;
            Ok(wrap_output(
                serde_json::to_value(result).map_err(|e| FunctionCallError::Fatal(e.to_string()))?,
                "go_back",
                json!({}),
            ))
        })
    }
}

/// Go forward in the browser history.
pub struct BrowserGoForwardTool {
    state: Arc<BrowserThreadState>,
}

impl BrowserGoForwardTool {
    pub fn new(state: Arc<BrowserThreadState>) -> Self {
        Self { state }
    }
}

impl BrowserTool for BrowserGoForwardTool {
    fn state(&self) -> &Arc<BrowserThreadState> {
        &self.state
    }
}

impl ToolExecutor<ToolCall> for BrowserGoForwardTool {
    fn tool_name(&self) -> ToolName {
        namespaced_tool_name("go_forward")
    }

    fn spec(&self) -> ToolSpec {
        namespace_spec(
            "go_forward",
            "Go forward one step in the browser history.",
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        )
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Direct
    }

    #[tracing::instrument(skip_all, fields(tool_name = ?call.tool_name, call_id = %call.call_id, turn_id = %call.turn_id, model = %call.model, approved = call.guardian_approved_action_id.is_some()))]
    fn handle(&self, call: ToolCall) -> ToolExecutorFuture<'_> {
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            ensure_browser_approved(&call)?;
            let result = state.go_forward().await.map_err(map_error)?;
            Ok(wrap_output(
                serde_json::to_value(result).map_err(|e| FunctionCallError::Fatal(e.to_string()))?,
                "go_forward",
                json!({}),
            ))
        })
    }
}

/// Reload the current page.
pub struct BrowserReloadTool {
    state: Arc<BrowserThreadState>,
}

impl BrowserReloadTool {
    pub fn new(state: Arc<BrowserThreadState>) -> Self {
        Self { state }
    }
}

impl BrowserTool for BrowserReloadTool {
    fn state(&self) -> &Arc<BrowserThreadState> {
        &self.state
    }
}

impl ToolExecutor<ToolCall> for BrowserReloadTool {
    fn tool_name(&self) -> ToolName {
        namespaced_tool_name("reload")
    }

    fn spec(&self) -> ToolSpec {
        namespace_spec(
            "reload",
            "Reload the current page.",
            json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        )
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Direct
    }

    #[tracing::instrument(skip_all, fields(tool_name = ?call.tool_name, call_id = %call.call_id, turn_id = %call.turn_id, model = %call.model, approved = call.guardian_approved_action_id.is_some()))]
    fn handle(&self, call: ToolCall) -> ToolExecutorFuture<'_> {
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            ensure_browser_approved(&call)?;
            let result = state.reload().await.map_err(map_error)?;
            Ok(wrap_output(
                serde_json::to_value(result).map_err(|e| FunctionCallError::Fatal(e.to_string()))?,
                "reload",
                json!({}),
            ))
        })
    }
}

/// Capture a screenshot of the page.
pub struct BrowserScreenshotTool {
    state: Arc<BrowserThreadState>,
}

impl BrowserScreenshotTool {
    pub fn new(state: Arc<BrowserThreadState>) -> Self {
        Self { state }
    }
}

impl BrowserTool for BrowserScreenshotTool {
    fn state(&self) -> &Arc<BrowserThreadState> {
        &self.state
    }
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct BrowserScreenshotInput {
    #[serde(default)]
    full_page: bool,
    #[serde(default)]
    path: Option<String>,
}

/// Where a `path`-mode screenshot is written.
///
/// The path is pinned inside the turn's working directory: absolute paths and
/// `..` components are rejected rather than resolved. The browser tool has no
/// business writing outside the workspace, and a model-supplied path is exactly
/// the input that would otherwise let it.
struct ScreenshotTarget {
    path_uri: PathUri,
    requested_path: String,
    file_system: Arc<dyn ExecutorFileSystem>,
    sandbox: FileSystemSandboxContext,
}

impl ScreenshotTarget {
    fn resolve(call: &ToolCall, requested: &str) -> Result<Self, FunctionCallError> {
        let requested = requested.trim();
        if requested.is_empty() {
            return Err(FunctionCallError::RespondToModel(
                "`path` must not be empty; omit it to receive base64 instead".to_string(),
            ));
        }
        let candidate = Path::new(requested);
        if candidate.is_absolute() {
            return Err(FunctionCallError::RespondToModel(format!(
                "`path` must be relative to the workspace, got the absolute path `{requested}`"
            )));
        }
        if candidate
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        {
            return Err(FunctionCallError::RespondToModel(format!(
                "`path` must not contain `..`, got `{requested}`"
            )));
        }
        // A screenshot writes a PNG. Requiring the extension keeps the tool from
        // clobbering a source file when the model passes the wrong path.
        let is_png = candidate
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|extension| extension.eq_ignore_ascii_case("png"));
        if !is_png {
            return Err(FunctionCallError::RespondToModel(format!(
                "`path` must end in `.png`, got `{requested}`"
            )));
        }
        let environment = call.environments.first().ok_or_else(|| {
            FunctionCallError::RespondToModel(
                "`path` needs a host environment with a working directory; this call has none, so omit `path` to receive base64 instead"
                    .to_string(),
            )
        })?;
        let absolute = environment.cwd.join(candidate);
        Ok(Self {
            path_uri: PathUri::from_abs_path(&absolute),
            requested_path: requested.to_string(),
            file_system: Arc::clone(&environment.file_system),
            sandbox: environment.file_system_sandbox_context.clone(),
        })
    }

    async fn write(&self, png: Vec<u8>) -> Result<ScreenshotFile, FunctionCallError> {
        if let Some(parent) = self.path_uri.parent() {
            match self
                .file_system
                .create_directory(
                    &parent,
                    CreateDirectoryOptions { recursive: true },
                    Some(&self.sandbox),
                )
                .await
            {
                Ok(()) => {}
                // The directory already exists; writing into it is still correct.
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(FunctionCallError::RespondToModel(format!(
                        "unable to create the directory for `{}`: {error}",
                        self.requested_path
                    )));
                }
            }
        }
        let bytes = png.len();
        self.file_system
            .write_file(&self.path_uri, png, Some(&self.sandbox))
            .await
            .map_err(|error| {
                FunctionCallError::RespondToModel(format!(
                    "unable to write the screenshot to `{}`: {error}",
                    self.requested_path
                ))
            })?;
        Ok(ScreenshotFile {
            path: self.requested_path.clone(),
            bytes,
            mime_type: "image/png".to_string(),
        })
    }
}

impl ToolExecutor<ToolCall> for BrowserScreenshotTool {
    fn tool_name(&self) -> ToolName {
        namespaced_tool_name("screenshot")
    }

    fn spec(&self) -> ToolSpec {
        namespace_spec(
            "screenshot",
            "Capture a PNG screenshot of the current page. Pass `path` to write it inside the workspace, then read it with `view_image`; omit `path` to get base64 back.",
            json!({
                "type": "object",
                "properties": {
                    "full_page": {
                        "type": "boolean",
                        "description": "Capture the full page instead of the viewport."
                    },
                    "path": {
                        "type": "string",
                        "description": "Workspace-relative PNG path, e.g. `.ody-code/frontend/shots/home@1280.png`. Parent directories are created. Read the file back with `view_image` to actually see the page. Absolute paths and `..` are rejected."
                    }
                },
                "additionalProperties": false
            }),
        )
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Direct
    }

    #[tracing::instrument(skip_all, fields(tool_name = ?call.tool_name, call_id = %call.call_id, turn_id = %call.turn_id, model = %call.model, approved = call.guardian_approved_action_id.is_some()))]
    fn handle(&self, call: ToolCall) -> ToolExecutorFuture<'_> {
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            let input: BrowserScreenshotInput = parse_args(&call)?;
            let details = serde_json::to_value(&input)
                .map_err(|e| FunctionCallError::Fatal(e.to_string()))?;
            if let Some(requested) = input.path.as_deref() {
                // Resolve before capturing: a rejected path must not cost a capture.
                let target = ScreenshotTarget::resolve(&call, requested)?;
                let png = state
                    .screenshot_bytes(input.full_page)
                    .await
                    .map_err(map_error)?;
                let saved = target.write(png).await?;
                return Ok(wrap_output(
                    serde_json::to_value(saved)
                        .map_err(|e| FunctionCallError::Fatal(e.to_string()))?,
                    "screenshot",
                    details,
                ));
            }
            let result = state
                .screenshot(input.full_page)
                .await
                .map_err(map_error)?;
            Ok(wrap_output(
                serde_json::to_value(result).map_err(|e| FunctionCallError::Fatal(e.to_string()))?,
                "screenshot",
                details,
            ))
        })
    }
}

/// Change the layout viewport, or clear the override.
///
/// Responsive verification needs a breakpoint matrix, and launching a browser
/// per width is both slow and unrepeatable. This is the first-class tool for it:
/// a raw CDP call would work in local mode but is refused for external browsers.
pub struct BrowserSetViewportTool {
    state: Arc<BrowserThreadState>,
}

impl BrowserSetViewportTool {
    pub fn new(state: Arc<BrowserThreadState>) -> Self {
        Self { state }
    }
}

impl BrowserTool for BrowserSetViewportTool {
    fn state(&self) -> &Arc<BrowserThreadState> {
        &self.state
    }
}

/// Chrome rejects zero-sized metrics, and anything above this is not a real
/// device: both are caller mistakes worth reporting instead of forwarding.
const MAX_VIEWPORT_EDGE: u32 = 16_384;

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct BrowserSetViewportInput {
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
    #[serde(default)]
    device_scale_factor: Option<f64>,
    #[serde(default)]
    mobile: bool,
    /// Drop the override and return to the browser window's own size.
    #[serde(default)]
    reset: bool,
}

impl ToolExecutor<ToolCall> for BrowserSetViewportTool {
    fn tool_name(&self) -> ToolName {
        namespaced_tool_name("set_viewport")
    }

    fn spec(&self) -> ToolSpec {
        namespace_spec(
            "set_viewport",
            "Set the layout viewport size so a responsive breakpoint can be screenshotted, or clear the override with `reset`.",
            json!({
                "type": "object",
                "properties": {
                    "width": {
                        "type": "integer",
                        "description": "Viewport width in CSS pixels, from 1 to 16384."
                    },
                    "height": {
                        "type": "integer",
                        "description": "Viewport height in CSS pixels, from 1 to 16384."
                    },
                    "device_scale_factor": {
                        "type": "number",
                        "description": "Device pixel ratio. Defaults to 1."
                    },
                    "mobile": {
                        "type": "boolean",
                        "description": "Emulate a mobile device (touch events, meta viewport). Defaults to false."
                    },
                    "reset": {
                        "type": "boolean",
                        "description": "Clear the override instead of setting one."
                    }
                },
                "additionalProperties": false
            }),
        )
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Direct
    }

    #[tracing::instrument(skip_all, fields(tool_name = ?call.tool_name, call_id = %call.call_id, turn_id = %call.turn_id, model = %call.model, approved = call.guardian_approved_action_id.is_some()))]
    fn handle(&self, call: ToolCall) -> ToolExecutorFuture<'_> {
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            let input: BrowserSetViewportInput = parse_args(&call)?;
            let details = serde_json::to_value(&input)
                .map_err(|e| FunctionCallError::Fatal(e.to_string()))?;
            if input.reset {
                state.clear_viewport().await.map_err(map_error)?;
                return Ok(wrap_output(
                    json!({"reset": true}),
                    "set_viewport",
                    details,
                ));
            }
            let width = input.width.ok_or_else(|| missing_edge("width"))?;
            let height = input.height.ok_or_else(|| missing_edge("height"))?;
            for (label, value) in [("width", width), ("height", height)] {
                if value == 0 || value > MAX_VIEWPORT_EDGE {
                    return Err(FunctionCallError::RespondToModel(format!(
                        "`{label}` must be between 1 and {MAX_VIEWPORT_EDGE}, got {value}"
                    )));
                }
            }
            let device_scale_factor = input.device_scale_factor.unwrap_or(1.0);
            if !(device_scale_factor.is_finite() && device_scale_factor > 0.0) {
                return Err(FunctionCallError::RespondToModel(format!(
                    "`device_scale_factor` must be a positive number, got {device_scale_factor}"
                )));
            }
            let viewport = ViewportOverride {
                width,
                height,
                device_scale_factor,
                mobile: input.mobile,
            };
            state.set_viewport(viewport).await.map_err(map_error)?;
            Ok(wrap_output(
                serde_json::to_value(viewport)
                    .map_err(|e| FunctionCallError::Fatal(e.to_string()))?,
                "set_viewport",
                details,
            ))
        })
    }
}

fn missing_edge(field: &str) -> FunctionCallError {
    FunctionCallError::RespondToModel(format!(
        "`{field}` is required unless `reset` is true"
    ))
}

/// Evaluate a JavaScript expression on the page.
pub struct BrowserEvaluateTool {
    state: Arc<BrowserThreadState>,
}

impl BrowserEvaluateTool {
    pub fn new(state: Arc<BrowserThreadState>) -> Self {
        Self { state }
    }
}

impl BrowserTool for BrowserEvaluateTool {
    fn state(&self) -> &Arc<BrowserThreadState> {
        &self.state
    }
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct BrowserEvaluateInput {
    expression: String,
}

impl ToolExecutor<ToolCall> for BrowserEvaluateTool {
    fn tool_name(&self) -> ToolName {
        namespaced_tool_name("evaluate")
    }

    fn spec(&self) -> ToolSpec {
        namespace_spec(
            "evaluate",
            "Evaluate a JavaScript expression on the current page and return the result.",
            json!({
                "type": "object",
                "properties": {
                    "expression": {
                        "type": "string",
                        "description": "JavaScript expression to evaluate."
                    }
                },
                "required": ["expression"],
                "additionalProperties": false
            }),
        )
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Direct
    }

    #[tracing::instrument(skip_all, fields(tool_name = ?call.tool_name, call_id = %call.call_id, turn_id = %call.turn_id, model = %call.model, approved = call.guardian_approved_action_id.is_some()))]
    fn handle(&self, call: ToolCall) -> ToolExecutorFuture<'_> {
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            check_external_sensitive(&state, "evaluate")?;
            let input: BrowserEvaluateInput = parse_args(&call)?;
            tracing::info!(expression_preview = %crate::types::expression_preview(&input.expression));
            if let Err(reason) = crate::url_block::check_js_allowed(&input.expression) {
                return Err(FunctionCallError::Fatal(reason));
            }
            ensure_browser_approved(&call)?;
            let result = state
                .evaluate(&input.expression)
                .await
                .map_err(map_error)?;
            Ok(wrap_output(
                serde_json::to_value(result).map_err(|e| FunctionCallError::Fatal(e.to_string()))?,
                "evaluate",
                serde_json::to_value(input).map_err(|e| FunctionCallError::Fatal(e.to_string()))?,
            ))
        })
    }
}

/// Click at a coordinate on the page.
pub struct BrowserClickTool {
    state: Arc<BrowserThreadState>,
}

impl BrowserClickTool {
    pub fn new(state: Arc<BrowserThreadState>) -> Self {
        Self { state }
    }
}

impl BrowserTool for BrowserClickTool {
    fn state(&self) -> &Arc<BrowserThreadState> {
        &self.state
    }
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct BrowserClickInput {
    x: f64,
    y: f64,
}

impl ToolExecutor<ToolCall> for BrowserClickTool {
    fn tool_name(&self) -> ToolName {
        namespaced_tool_name("click")
    }

    fn spec(&self) -> ToolSpec {
        namespace_spec(
            "click",
            "Click at the given coordinates on the current page.",
            json!({
                "type": "object",
                "properties": {
                    "x": { "type": "number", "description": "X coordinate in CSS pixels." },
                    "y": { "type": "number", "description": "Y coordinate in CSS pixels." }
                },
                "required": ["x", "y"],
                "additionalProperties": false
            }),
        )
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Direct
    }

    #[tracing::instrument(skip_all, fields(tool_name = ?call.tool_name, call_id = %call.call_id, turn_id = %call.turn_id, model = %call.model, approved = call.guardian_approved_action_id.is_some()))]
    fn handle(&self, call: ToolCall) -> ToolExecutorFuture<'_> {
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            check_external_sensitive(&state, "click")?;
            ensure_browser_approved(&call)?;
            let input: BrowserClickInput = parse_args(&call)?;
            state
                .click(Point {
                    x: input.x,
                    y: input.y,
                })
                .await
                .map_err(map_error)?;
            Ok(wrap_output(
                json!({"clicked": true}),
                "click",
                serde_json::to_value(input).map_err(|e| FunctionCallError::Fatal(e.to_string()))?,
            ))
        })
    }
}

/// Type text into an element selected by CSS selector.
pub struct BrowserTypeTool {
    state: Arc<BrowserThreadState>,
}

impl BrowserTypeTool {
    pub fn new(state: Arc<BrowserThreadState>) -> Self {
        Self { state }
    }
}

impl BrowserTool for BrowserTypeTool {
    fn state(&self) -> &Arc<BrowserThreadState> {
        &self.state
    }
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct BrowserTypeInput {
    selector: String,
    text: String,
}

impl ToolExecutor<ToolCall> for BrowserTypeTool {
    fn tool_name(&self) -> ToolName {
        namespaced_tool_name("type")
    }

    fn spec(&self) -> ToolSpec {
        namespace_spec(
            "type",
            "Type text into the element selected by the given CSS selector.",
            json!({
                "type": "object",
                "properties": {
                    "selector": { "type": "string", "description": "CSS selector for the target element." },
                    "text": { "type": "string", "description": "Text to type into the element." }
                },
                "required": ["selector", "text"],
                "additionalProperties": false
            }),
        )
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Direct
    }

    #[tracing::instrument(skip_all, fields(tool_name = ?call.tool_name, call_id = %call.call_id, turn_id = %call.turn_id, model = %call.model, approved = call.guardian_approved_action_id.is_some()))]
    fn handle(&self, call: ToolCall) -> ToolExecutorFuture<'_> {
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            check_external_sensitive(&state, "type")?;
            ensure_browser_approved(&call)?;
            let input: BrowserTypeInput = parse_args(&call)?;
            tracing::info!(
                selector_preview = %crate::types::truncate_string_bytes(&input.selector, 120),
                text_size = input.text.len()
            );
            state
                .type_text(&input.selector, &input.text)
                .await
                .map_err(map_error)?;
            Ok(wrap_output(
                json!({"typed": true}),
                "type",
                serde_json::to_value(input).map_err(|e| FunctionCallError::Fatal(e.to_string()))?,
            ))
        })
    }
}

/// Return a DOM representation of the page or a selected element.
pub struct BrowserGetDomTool {
    state: Arc<BrowserThreadState>,
}

impl BrowserGetDomTool {
    pub fn new(state: Arc<BrowserThreadState>) -> Self {
        Self { state }
    }
}

impl BrowserTool for BrowserGetDomTool {
    fn state(&self) -> &Arc<BrowserThreadState> {
        &self.state
    }
}

#[derive(Debug, Default, serde::Deserialize, serde::Serialize)]
struct BrowserGetDomInput {
    #[serde(default)]
    selector: Option<String>,
}

impl ToolExecutor<ToolCall> for BrowserGetDomTool {
    fn tool_name(&self) -> ToolName {
        namespaced_tool_name("get_dom")
    }

    fn spec(&self) -> ToolSpec {
        namespace_spec(
            "get_dom",
            "Return a JSON or HTML representation of the page DOM, optionally scoped to a selector.",
            json!({
                "type": "object",
                "properties": {
                    "selector": {
                        "type": "string",
                        "description": "Optional CSS selector. If omitted, the full document is returned."
                    }
                },
                "additionalProperties": false
            }),
        )
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Direct
    }

    #[tracing::instrument(skip_all, fields(tool_name = ?call.tool_name, call_id = %call.call_id, turn_id = %call.turn_id, model = %call.model, approved = call.guardian_approved_action_id.is_some()))]
    fn handle(&self, call: ToolCall) -> ToolExecutorFuture<'_> {
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            let input: BrowserGetDomInput = parse_args(&call)?;
            tracing::info!(
                selector_preview = ?input.selector.as_deref().map(|s| crate::types::truncate_string_bytes(s, 120))
            );
            let result = state
                .get_dom(input.selector.as_deref())
                .await
                .map_err(map_error)?;
            Ok(wrap_output(
                result,
                "get_dom",
                serde_json::to_value(input).map_err(|e| FunctionCallError::Fatal(e.to_string()))?,
            ))
        })
    }
}

/// Read buffered console and network logs.
pub struct BrowserReadLogsTool {
    state: Arc<BrowserThreadState>,
}

impl BrowserReadLogsTool {
    pub fn new(state: Arc<BrowserThreadState>) -> Self {
        Self { state }
    }
}

impl BrowserTool for BrowserReadLogsTool {
    fn state(&self) -> &Arc<BrowserThreadState> {
        &self.state
    }
}

#[derive(Debug, Default, serde::Deserialize, serde::Serialize)]
struct BrowserReadLogsInput {
    #[serde(default = "default_read_logs_kind")]
    kind: LogKind,
    #[serde(default = "default_read_logs_level")]
    level: LogLevel,
    #[serde(default)]
    drain: bool,
}

fn default_read_logs_kind() -> LogKind {
    LogKind::All
}

fn default_read_logs_level() -> LogLevel {
    LogLevel::Verbose
}

impl ToolExecutor<ToolCall> for BrowserReadLogsTool {
    fn tool_name(&self) -> ToolName {
        namespaced_tool_name("read_logs")
    }

    fn spec(&self) -> ToolSpec {
        namespace_spec(
            "read_logs",
            "Read buffered console and network logs from the page.",
            json!({
                "type": "object",
                "properties": {
                    "kind": {
                        "type": "string",
                        "enum": ["console", "network", "all"],
                        "description": "Which log entries to include."
                    },
                    "level": {
                        "type": "string",
                        "enum": ["verbose", "info", "warning", "error"],
                        "description": "Minimum console log level to include."
                    },
                    "drain": {
                        "type": "boolean",
                        "description": "Remove the entries this call returns from the buffer, so the next read reports only what happened afterwards. Use it to mark the start of a verification round: read with drain before the actions under test, then read again and attribute every entry to this round. Defaults to false, which returns the whole history."
                    }
                },
                "additionalProperties": false
            }),
        )
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Direct
    }

    #[tracing::instrument(skip_all, fields(tool_name = ?call.tool_name, call_id = %call.call_id, turn_id = %call.turn_id, model = %call.model, approved = call.guardian_approved_action_id.is_some()))]
    fn handle(&self, call: ToolCall) -> ToolExecutorFuture<'_> {
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            let input: BrowserReadLogsInput = parse_args(&call)?;
            tracing::info!(kind = ?input.kind, level = ?input.level, drain = input.drain);
            let result = state
                .read_logs(input.kind, input.level, input.drain)
                .await
                .map_err(map_error)?;
            Ok(wrap_output(
                serde_json::to_value(result).map_err(|e| FunctionCallError::Fatal(e.to_string()))?,
                "read_logs",
                serde_json::to_value(input).map_err(|e| FunctionCallError::Fatal(e.to_string()))?,
            ))
        })
    }
}

/// Execute a raw Chrome DevTools Protocol command.
pub struct BrowserExecuteRawCdpTool {
    state: Arc<BrowserThreadState>,
}

impl BrowserExecuteRawCdpTool {
    pub fn new(state: Arc<BrowserThreadState>) -> Self {
        Self { state }
    }
}

impl BrowserTool for BrowserExecuteRawCdpTool {
    fn state(&self) -> &Arc<BrowserThreadState> {
        &self.state
    }
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct BrowserExecuteRawCdpInput {
    method: String,
    #[serde(default)]
    params: serde_json::Value,
}

impl ToolExecutor<ToolCall> for BrowserExecuteRawCdpTool {
    fn tool_name(&self) -> ToolName {
        namespaced_tool_name("execute_raw_cdp")
    }

    fn spec(&self) -> ToolSpec {
        namespace_spec(
            "execute_raw_cdp",
            "Execute a raw Chrome DevTools Protocol method with JSON parameters. Requires full CDP access.",
            json!({
                "type": "object",
                "properties": {
                    "method": {
                        "type": "string",
                        "description": "CDP method name, e.g. 'Runtime.getProperties'."
                    },
                    "params": {
                        "type": "object",
                        "description": "JSON parameters for the CDP method."
                    }
                },
                "required": ["method"],
                "additionalProperties": false
            }),
        )
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Direct
    }

    #[tracing::instrument(skip_all, fields(tool_name = ?call.tool_name, call_id = %call.call_id, turn_id = %call.turn_id, model = %call.model, approved = call.guardian_approved_action_id.is_some()))]
    fn handle(&self, call: ToolCall) -> ToolExecutorFuture<'_> {
        let state = Arc::clone(&self.state);
        Box::pin(async move {
            if state.config().mode == crate::config::BrowserControlMode::External {
                tracing::info!(
                    tool = "browser__execute_raw_cdp",
                    reason = "external mode",
                    "execute_raw_cdp rejected"
                );
                return Err(FunctionCallError::Fatal(
                    "execute_raw_cdp is disabled in external browser mode".to_string(),
                ));
            }
            let input: BrowserExecuteRawCdpInput = parse_args(&call)?;
            tracing::info!(
                method = %input.method,
                params_size = serde_json::to_string(&input.params).map(|s| s.len()).unwrap_or(0)
            );
            if let Some(pattern) = crate::raw_cdp_blocklist::is_raw_cdp_blocked(&input.method) {
                return Err(map_error(BrowserControlError::NotAllowed {
                    reason: format!(
                        "raw CDP method is blocked: {} (matched pattern: {pattern})",
                        input.method
                    ),
                }));
            }
            ensure_browser_approved(&call)?;
            let result = state
                .execute_raw(&input.method, input.params.clone())
                .await
                .map_err(map_error)?;
            Ok(wrap_output(
                result,
                "execute_raw_cdp",
                serde_json::to_value(input).map_err(|e| FunctionCallError::Fatal(e.to_string()))?,
            ))
        })
    }
}

/// Return all browser tools backed by `state`.
///
/// The caller (typically the `app-server` extension) decides which tools are
/// exposed to the model based on the effective feature flags.
pub fn all_tools(state: Arc<BrowserThreadState>) -> Vec<Arc<dyn ToolExecutor<ToolCall>>> {
    vec![
        Arc::new(BrowserNavigateTool::new(Arc::clone(&state))) as Arc<dyn ToolExecutor<ToolCall>>,
        Arc::new(BrowserGoBackTool::new(Arc::clone(&state))) as Arc<dyn ToolExecutor<ToolCall>>,
        Arc::new(BrowserGoForwardTool::new(Arc::clone(&state))) as Arc<dyn ToolExecutor<ToolCall>>,
        Arc::new(BrowserReloadTool::new(Arc::clone(&state))) as Arc<dyn ToolExecutor<ToolCall>>,
        Arc::new(BrowserScreenshotTool::new(Arc::clone(&state))) as Arc<dyn ToolExecutor<ToolCall>>,
        Arc::new(BrowserSetViewportTool::new(Arc::clone(&state))) as Arc<dyn ToolExecutor<ToolCall>>,
        Arc::new(BrowserEvaluateTool::new(Arc::clone(&state))) as Arc<dyn ToolExecutor<ToolCall>>,
        Arc::new(BrowserClickTool::new(Arc::clone(&state))) as Arc<dyn ToolExecutor<ToolCall>>,
        Arc::new(BrowserTypeTool::new(Arc::clone(&state))) as Arc<dyn ToolExecutor<ToolCall>>,
        Arc::new(BrowserGetDomTool::new(Arc::clone(&state))) as Arc<dyn ToolExecutor<ToolCall>>,
        Arc::new(BrowserReadLogsTool::new(Arc::clone(&state))) as Arc<dyn ToolExecutor<ToolCall>>,
        Arc::new(BrowserExecuteRawCdpTool::new(Arc::clone(&state))) as Arc<dyn ToolExecutor<ToolCall>>,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    use ody_file_system::{
        CopyOptions, ExecutorFileSystemFuture, FileMetadata, FileSystemReadStream,
        ReadDirectoryEntry, RemoveOptions,
    };
    use ody_protocol::models::ResponseInputItem;
    use ody_protocol::protocol::TruncationPolicy;
    use ody_tools::{NoopTurnItemEmitter, ToolEnvironment, ToolPayload};
    use ody_utils_path_uri::PathUri;
    use serde_json::Value;

    use crate::BrowserControlConfig;

    /// The schema is the only thing the model reads before it calls. When it
    /// advertised `networkidle` and serde demanded `network_idle`, a correct call
    /// failed on every retry. Compare the real spec against the real enum.
    #[test]
    fn navigate_schema_enum_matches_the_wait_condition_names() {
        let state = Arc::new(
            BrowserThreadState::new_uninitialized_for_test(BrowserControlConfig::default())
                .expect("uninitialized state"),
        );
        let spec = BrowserNavigateTool::new(state).spec();
        let ToolSpec::Namespace(namespace) = &spec else {
            panic!("browser tools are published as a namespace");
        };
        let ResponsesApiNamespaceTool::Function(function) = &namespace.tools[0];

        let advertised: Vec<String> = function
            .parameters
            .properties
            .as_ref()
            .and_then(|properties| properties.get("wait_until"))
            .and_then(|schema| schema.enum_values.clone())
            .expect("navigate advertises a closed set of wait conditions")
            .into_iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect();

        assert_eq!(advertised, wait_condition_names());
        for value in &advertised {
            let parsed: WaitCondition = serde_json::from_value(json!(value))
                .unwrap_or_else(|e| panic!("advertised value `{value}` must parse: {e}"));
            assert_eq!(&serde_json::to_value(parsed).unwrap(), &json!(value));
        }
        assert!(
            !advertised.iter().any(|value| value == "networkidle"),
            "networkidle is not a name serde accepts; advertising it costs a retry"
        );
    }

    #[test]
    /// A verification round needs to say "count from here". That only works if
    /// the schema advertises the switch, and if omitting it keeps the old
    /// whole-history behaviour for every other caller.
    fn read_logs_advertises_the_drain_switch_and_still_defaults_to_the_history() {
        let state = Arc::new(
            BrowserThreadState::new_uninitialized_for_test(BrowserControlConfig::default())
                .expect("uninitialized state"),
        );
        let spec = BrowserReadLogsTool::new(state).spec();
        let ToolSpec::Namespace(namespace) = &spec else {
            panic!("browser tools are published as a namespace");
        };
        let ResponsesApiNamespaceTool::Function(function) = &namespace.tools[0];

        let drain = function
            .parameters
            .properties
            .as_ref()
            .and_then(|properties| properties.get("drain"))
            .expect("read_logs advertises the drain switch");
        assert_eq!(
            serde_json::to_value(drain)
                .unwrap()
                .get("type")
                .and_then(Value::as_str),
            Some("boolean"),
            "the model can only send what the schema names"
        );

        let defaults: BrowserReadLogsInput =
            serde_json::from_value(json!({})).expect("an empty call must still parse");
        assert!(
            !defaults.drain,
            "omitting drain must keep the cumulative behaviour"
        );

        let explicit: BrowserReadLogsInput =
            serde_json::from_value(json!({"kind": "console", "level": "error", "drain": true}))
                .expect("an explicit drain must parse");
        assert!(explicit.drain);
    }

    #[test]
    fn wrap_output_includes_approval_ticket() {
        let out = wrap_output(
            json!({"url": "https://example.com"}),
            "navigate",
            json!({"url": "https://example.com"}),
        );
        let item = out.to_response_item(
            "call-1",
            &ToolPayload::Function {
                arguments: String::new(),
            },
        );
        let text = match item {
            ResponseInputItem::FunctionCallOutput { output, .. } => {
                output.body.to_text().unwrap_or_default()
            }
            _ => panic!("expected function call output"),
        };
        assert!(text.contains("approval_ticket"));
        assert!(text.contains("navigate"));
    }

    fn make_tool_call(
        tool_name: ToolName,
        arguments: &str,
        guardian_approved_action_id: Option<String>,
    ) -> ToolCall {
        ToolCall {
            turn_id: "turn-1".to_string(),
            call_id: "call-1".to_string(),
            tool_name,
            model: "test".to_string(),
            truncation_policy: TruncationPolicy::Bytes(1),
            conversation_history: Default::default(),
            turn_item_emitter: Arc::new(NoopTurnItemEmitter),
            environments: Vec::new(),
            payload: ToolPayload::Function {
                arguments: arguments.to_string(),
            },
            guardian_approved_action_id,
        }
    }

    #[tokio::test]
    async fn navigate_test_mode_exempt_proceeds_to_state_error() {
        let state = Arc::new(
            BrowserThreadState::new_uninitialized_for_test(BrowserControlConfig::default())
                .expect("uninitialized state"),
        );
        let tool = BrowserNavigateTool::new(state);
        let call = make_tool_call(
            ToolName::namespaced("browser", "navigate"),
            r#"{"url": "https://example.com"}"#,
            None,
        );
        let result = ToolExecutor::handle(&tool, call).await;
        assert!(result.is_err(), "expected an error");
        let Err(err) = result else { unreachable!() };
        // In test builds the call is auto-approved by the test-mode exemption, so
        // it reaches the uninitialized state and returns a fatal error instead of
        // NeedsApproval.
        assert!(
            !matches!(err, FunctionCallError::NeedsApproval { .. }),
            "test-mode navigate should be exempt from approval: {err:?}"
        );
    }

    #[tokio::test]
    async fn navigate_skips_approval_with_action_id() {
        let state = Arc::new(
            BrowserThreadState::new_uninitialized_for_test(BrowserControlConfig::default())
                .expect("uninitialized state"),
        );
        let tool = BrowserNavigateTool::new(state);
        let call = make_tool_call(
            ToolName::namespaced("browser", "navigate"),
            r#"{"url": "https://example.com"}"#,
            Some("review-1".to_string()),
        );
        let result = ToolExecutor::handle(&tool, call).await;
        assert!(result.is_err(), "expected an error from uninitialized state");
        let Err(err) = result else { unreachable!() };
        assert!(
            !matches!(err, FunctionCallError::NeedsApproval { .. }),
            "approved call should not ask for approval: {err:?}"
        );
    }

    #[tokio::test]
    async fn read_only_screenshot_does_not_require_approval() {
        let state = Arc::new(
            BrowserThreadState::new_uninitialized_for_test(BrowserControlConfig::default())
                .expect("uninitialized state"),
        );
        let tool = BrowserScreenshotTool::new(state);
        let call = ToolCall {
            turn_id: "turn-1".to_string(),
            call_id: "call-1".to_string(),
            tool_name: ToolName::namespaced("browser", "screenshot"),
            model: "test".to_string(),
            truncation_policy: TruncationPolicy::Bytes(1),
            conversation_history: Default::default(),
            turn_item_emitter: Arc::new(NoopTurnItemEmitter),
            environments: Vec::new(),
            payload: ToolPayload::Function { arguments: "{}".to_string() },
            guardian_approved_action_id: None,
        };
        let result = ToolExecutor::handle(&tool, call).await;
        assert!(result.is_err(), "expected an error from uninitialized state");
        let Err(err) = result else { unreachable!() };
        assert!(
            !matches!(err, FunctionCallError::NeedsApproval { .. }),
            "screenshot should not require approval: {err:?}"
        );
    }

    #[tokio::test]
    async fn evaluate_rejects_forbidden_expression_before_approval() {
        let state = Arc::new(
            BrowserThreadState::new_uninitialized_for_test(BrowserControlConfig::default())
                .expect("uninitialized state"),
        );
        let tool = BrowserEvaluateTool::new(state);
        let call = make_tool_call(
            ToolName::namespaced("browser", "evaluate"),
            r#"{"expression": "document.cookie"}"#,
            None,
        );
        let result = ToolExecutor::handle(&tool, call).await;
        assert!(result.is_err(), "expected an error");
        let Err(err) = result else { unreachable!() };
        assert!(
            !matches!(err, FunctionCallError::NeedsApproval { .. }),
            "forbidden expression should be rejected before approval: {err:?}"
        );
        assert!(err.to_string().contains("document.cookie"));
    }

    #[tokio::test]
    async fn evaluate_approval_ticket_truncates_long_expression() {
        let state = Arc::new(
            BrowserThreadState::new_uninitialized_for_test(BrowserControlConfig::default())
                .expect("uninitialized state"),
        );
        let tool = BrowserEvaluateTool::new(state);
        let long = "a".repeat(1000);
        let arguments = format!(r#"{{"expression": "{long}"}}"#);
        let call = make_tool_call(
            ToolName::namespaced("browser", "evaluate"),
            &arguments,
            None,
        );
        let result = ToolExecutor::handle(&tool, call).await;
        let Err(err) = result else { unreachable!() };
        let FunctionCallError::NeedsApproval { ticket } = err else {
            panic!("expected NeedsApproval, got {err:?}");
        };
        let details = ticket.get("details").expect("details");
        let expr = details.get("expression").and_then(Value::as_str).expect("expression");
        assert!(expr.len() <= 600, "expression should be truncated: got {} bytes", expr.len());
        assert_eq!(details.get("expression_truncated").and_then(Value::as_bool), Some(true));
    }

    #[tokio::test]
    async fn execute_raw_cdp_blocked_method_returns_not_allowed() {
        let state = Arc::new(
            BrowserThreadState::new_uninitialized_for_test(BrowserControlConfig::default())
                .expect("uninitialized state"),
        );
        let tool = BrowserExecuteRawCdpTool::new(state);
        let call = make_tool_call(
            ToolName::namespaced("browser", "execute_raw_cdp"),
            r#"{"method": "Storage.getCookies", "params": {}}"#,
            None,
        );
        let result = ToolExecutor::handle(&tool, call).await;
        assert!(result.is_err(), "expected an error");
        let Err(err) = result else { unreachable!() };
        assert!(
            !matches!(err, FunctionCallError::NeedsApproval { .. }),
            "blocked raw CDP should not ask for approval: {err:?}"
        );
        assert!(err.to_string().contains("Storage.getCookies"));
    }

    #[tokio::test]
    async fn execute_raw_cdp_safe_method_requires_approval() {
        let state = Arc::new(
            BrowserThreadState::new_uninitialized_for_test(BrowserControlConfig::default())
                .expect("uninitialized state"),
        );
        let tool = BrowserExecuteRawCdpTool::new(state);
        let call = make_tool_call(
            ToolName::namespaced("browser", "execute_raw_cdp"),
            r#"{"method": "Runtime.evaluate", "params": {"expression": "1+1"}}"#,
            None,
        );
        let result = ToolExecutor::handle(&tool, call).await;
        assert!(result.is_err(), "expected an error");
        let Err(err) = result else { unreachable!() };
        assert!(
            matches!(err, FunctionCallError::NeedsApproval { .. }),
            "safe raw CDP should require approval: {err:?}"
        );
    }

    #[tokio::test]
    async fn execute_raw_cdp_disabled_in_external_mode() {
        let mut config = BrowserControlConfig::default();
        config.mode = BrowserControlMode::External;
        let state = Arc::new(
            BrowserThreadState::new_uninitialized_for_test(config)
                .expect("uninitialized state"),
        );
        let tool = BrowserExecuteRawCdpTool::new(state);
        let call = make_tool_call(
            ToolName::namespaced("browser", "execute_raw_cdp"),
            r#"{"method": "Runtime.evaluate", "params": {}}"#,
            None,
        );
        let result = ToolExecutor::handle(&tool, call).await;
        assert!(result.is_err(), "expected an error");
        let Err(err) = result else { unreachable!() };
        assert!(err.to_string().contains("external browser mode"));
    }

    /// Filesystem that records what the screenshot writer asked for.
    ///
    /// Only the write path is exercised. Every other operation returns an error
    /// so that an unexpected call fails loudly instead of succeeding silently.
    #[derive(Default)]
    struct RecordingFileSystem {
        directories: std::sync::Mutex<Vec<String>>,
        writes: std::sync::Mutex<Vec<(String, Vec<u8>)>>,
    }

    impl RecordingFileSystem {
        fn directories(&self) -> Vec<String> {
            self.directories.lock().expect("directories lock").clone()
        }

        fn writes(&self) -> Vec<(String, Vec<u8>)> {
            self.writes.lock().expect("writes lock").clone()
        }
    }

    fn unsupported<T: Send + 'static>(operation: &str) -> ExecutorFileSystemFuture<'static, T> {
        let operation = operation.to_string();
        Box::pin(async move {
            Err(std::io::Error::other(format!(
                "{operation} is not supported by the recording filesystem"
            )))
        })
    }

    impl ExecutorFileSystem for RecordingFileSystem {
        fn canonicalize<'a>(
            &'a self,
            path: &'a PathUri,
            _sandbox: Option<&'a FileSystemSandboxContext>,
        ) -> ExecutorFileSystemFuture<'a, PathUri> {
            Box::pin(async move { Ok(path.clone()) })
        }

        fn read_file<'a>(
            &'a self,
            _path: &'a PathUri,
            _sandbox: Option<&'a FileSystemSandboxContext>,
        ) -> ExecutorFileSystemFuture<'a, Vec<u8>> {
            unsupported("read_file")
        }

        fn read_file_stream<'a>(
            &'a self,
            _path: &'a PathUri,
            _sandbox: Option<&'a FileSystemSandboxContext>,
        ) -> ExecutorFileSystemFuture<'a, FileSystemReadStream> {
            unsupported("read_file_stream")
        }

        fn write_file<'a>(
            &'a self,
            path: &'a PathUri,
            contents: Vec<u8>,
            _sandbox: Option<&'a FileSystemSandboxContext>,
        ) -> ExecutorFileSystemFuture<'a, ()> {
            Box::pin(async move {
                self.writes
                    .lock()
                    .expect("writes lock")
                    .push((path.to_path_buf().display().to_string(), contents));
                Ok(())
            })
        }

        fn create_directory<'a>(
            &'a self,
            path: &'a PathUri,
            _options: CreateDirectoryOptions,
            _sandbox: Option<&'a FileSystemSandboxContext>,
        ) -> ExecutorFileSystemFuture<'a, ()> {
            Box::pin(async move {
                self.directories
                    .lock()
                    .expect("directories lock")
                    .push(path.to_path_buf().display().to_string());
                Ok(())
            })
        }

        fn get_metadata<'a>(
            &'a self,
            _path: &'a PathUri,
            _sandbox: Option<&'a FileSystemSandboxContext>,
        ) -> ExecutorFileSystemFuture<'a, FileMetadata> {
            unsupported("get_metadata")
        }

        fn read_directory<'a>(
            &'a self,
            _path: &'a PathUri,
            _sandbox: Option<&'a FileSystemSandboxContext>,
        ) -> ExecutorFileSystemFuture<'a, Vec<ReadDirectoryEntry>> {
            unsupported("read_directory")
        }

        fn remove<'a>(
            &'a self,
            _path: &'a PathUri,
            _options: RemoveOptions,
            _sandbox: Option<&'a FileSystemSandboxContext>,
        ) -> ExecutorFileSystemFuture<'a, ()> {
            unsupported("remove")
        }

        fn copy<'a>(
            &'a self,
            _source: &'a PathUri,
            _destination: &'a PathUri,
            _options: CopyOptions,
            _sandbox: Option<&'a FileSystemSandboxContext>,
        ) -> ExecutorFileSystemFuture<'a, ()> {
            unsupported("copy")
        }
    }

    fn workspace_path(path: &std::path::Path) -> ody_utils_absolute_path::AbsolutePathBuf {
        ody_utils_absolute_path::AbsolutePathBuf::from_absolute_path(path)
            .expect("tempdir path is absolute")
    }

    fn sandbox_for(cwd: &std::path::Path) -> FileSystemSandboxContext {
        let cwd = PathUri::from_host_native_path(cwd).expect("cwd is a host path");
        FileSystemSandboxContext {
            permissions: Default::default(),
            cwd: Some(cwd.clone()),
            workspace_roots: vec![cwd],
            windows_sandbox_level: Default::default(),
            windows_sandbox_private_desktop: false,
            use_legacy_landlock: false,
        }
    }

    fn make_tool_call_in_workspace(
        workspace: &std::path::Path,
        file_system: Arc<RecordingFileSystem>,
        tool_name: ToolName,
        arguments: &str,
    ) -> ToolCall {
        ToolCall {
            turn_id: "turn-1".to_string(),
            call_id: "call-1".to_string(),
            tool_name,
            model: "test".to_string(),
            truncation_policy: TruncationPolicy::Bytes(1),
            conversation_history: Default::default(),
            turn_item_emitter: Arc::new(NoopTurnItemEmitter),
            environments: vec![ToolEnvironment {
                environment_id: "env-1".to_string(),
                cwd: workspace_path(workspace),
                file_system,
                file_system_sandbox_context: sandbox_for(workspace),
            }],
            payload: ToolPayload::Function {
                arguments: arguments.to_string(),
            },
            guardian_approved_action_id: None,
        }
    }

    fn uninitialized_state() -> Arc<BrowserThreadState> {
        Arc::new(
            BrowserThreadState::new_uninitialized_for_test(BrowserControlConfig::default())
                .expect("uninitialized state"),
        )
    }

    fn spec_json(tool: &impl ToolExecutor<ToolCall>) -> Value {
        serde_json::to_value(tool.spec()).expect("tool spec serializes")
    }

    /// `required` is either absent or empty; either way the model is not forced
    /// to pass the field.
    fn assert_no_required(published: &Value) {
        let required = &published["required"];
        assert!(
            required.is_null() || required.as_array().is_some_and(Vec::is_empty),
            "no field may be required: {required}"
        );
    }

    /// The whole point of `path` is that `view_image` can read the result, so the
    /// bytes must land in the workspace untouched and the parent directory must
    /// exist. A truncated or missing file is a screenshot the model cannot see.
    #[tokio::test]
    async fn screenshot_path_writes_the_png_into_the_workspace() {
        let workspace = tempfile::tempdir().expect("tempdir");
        let file_system = Arc::new(RecordingFileSystem::default());
        let call = make_tool_call_in_workspace(
            workspace.path(),
            Arc::clone(&file_system),
            ToolName::namespaced("browser", "screenshot"),
            r#"{"path": ".ody-code/frontend/shots/home@1280.png", "full_page": true}"#,
        );
        let target = ScreenshotTarget::resolve(&call, ".ody-code/frontend/shots/home@1280.png")
            .expect("relative workspace path resolves");
        let png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        let saved = target.write(png.clone()).await.expect("write succeeds");

        assert_eq!(saved.path, ".ody-code/frontend/shots/home@1280.png");
        assert_eq!(saved.bytes, png.len());
        assert_eq!(saved.mime_type, "image/png");
        assert_eq!(
            file_system.directories(),
            vec![
                workspace
                    .path()
                    .join(".ody-code/frontend/shots")
                    .display()
                    .to_string()
            ],
            "the parent directory is created before the write"
        );
        let writes = file_system.writes();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].1, png, "PNG bytes are written unchanged");
        assert_eq!(
            writes[0].0,
            workspace
                .path()
                .join(".ody-code/frontend/shots/home@1280.png")
                .display()
                .to_string(),
            "the write is resolved against the workspace root"
        );
    }

    #[test]
    fn screenshot_path_refuses_to_escape_the_workspace() {
        let call = make_tool_call(
            ToolName::namespaced("browser", "screenshot"),
            r#"{"path": "../../etc/passwd"}"#,
            None,
        );
        for requested in ["/etc/passwd", "../../etc/passwd", "shots/../../../escape.png"] {
            let Err(err) = ScreenshotTarget::resolve(&call, requested) else {
                panic!("{requested} must be rejected");
            };
            assert!(
                matches!(err, FunctionCallError::RespondToModel(_)),
                "rejection is reported to the model: {err:?}"
            );
            assert!(
                err.to_string().contains("relative") || err.to_string().contains(".."),
                "the error names the rule: {err}"
            );
        }
    }

    /// Without an environment there is no workspace to resolve against. Failing
    /// with an actionable message beats writing to the process working directory.
    #[test]
    fn screenshot_path_requires_an_environment() {
        let call = make_tool_call(
            ToolName::namespaced("browser", "screenshot"),
            r#"{"path": "shot.png"}"#,
            None,
        );
        let Err(err) = ScreenshotTarget::resolve(&call, "shot.png") else {
            panic!("a call without environments must be rejected");
        };
        assert!(matches!(err, FunctionCallError::RespondToModel(_)));
        assert!(err.to_string().contains("omit `path`"), "{err}");
    }

    /// A screenshot writes a PNG; anything else would let a wrong path clobber a
    /// source file with image bytes.
    #[test]
    fn screenshot_path_must_be_a_png() {
        let workspace = tempfile::tempdir().expect("tempdir");
        let call = make_tool_call_in_workspace(
            workspace.path(),
            Arc::new(RecordingFileSystem::default()),
            ToolName::namespaced("browser", "screenshot"),
            r#"{"path": "src/main.rs"}"#,
        );
        for requested in ["src/main.rs", "shots/home@1280.png.txt", "shots/no-extension"] {
            let Err(err) = ScreenshotTarget::resolve(&call, requested) else {
                panic!("{requested} must be rejected");
            };
            assert!(err.to_string().contains(".png"), "{requested}: {err}");
        }
        // Case is not part of the rule.
        assert!(ScreenshotTarget::resolve(&call, "shots/HOME.PNG").is_ok());
    }

    /// The inline route has to stay the default: `path` is opt-in, otherwise every
    /// existing caller silently starts writing files.
    #[test]
    fn screenshot_path_is_optional_in_the_published_schema() {
        let spec = spec_json(&BrowserScreenshotTool::new(uninitialized_state()));
        let parameters = &spec["tools"][0]["parameters"];
        assert_eq!(parameters["properties"]["path"]["type"], "string");
        assert_eq!(parameters["properties"]["full_page"]["type"], "boolean");
        assert_no_required(parameters);
        assert!(
            parameters["properties"]["path"]["description"]
                .as_str()
                .is_some_and(|description| description.contains("view_image")),
            "the description has to name the tool that can read the file"
        );
    }

    #[test]
    fn set_viewport_publishes_width_and_height() {
        let spec = spec_json(&BrowserSetViewportTool::new(uninitialized_state()));
        let parameters = &spec["tools"][0]["parameters"];
        assert_eq!(parameters["properties"]["width"]["type"], "integer");
        assert_eq!(parameters["properties"]["height"]["type"], "integer");
        assert_eq!(
            parameters["properties"]["device_scale_factor"]["type"],
            "number"
        );
        assert_eq!(parameters["properties"]["reset"]["type"], "boolean");
        assert_no_required(parameters);
        // Numeric bounds are not part of the supported schema subset, so they
        // live in the description text instead.
        for field in ["width", "height"] {
            let description = parameters["properties"][field]["description"]
                .as_str()
                .expect("edge fields carry a description");
            assert!(
                description.contains("1 to 16384"),
                "{field} must document its accepted range: {description}"
            );
        }
    }

    #[tokio::test]
    async fn set_viewport_reports_missing_and_out_of_range_edges() {
        let tool = BrowserSetViewportTool::new(uninitialized_state());
        let cases = [
            (r#"{}"#, "`width` is required"),
            (r#"{"width": 1280}"#, "`height` is required"),
            (r#"{"width": 0, "height": 800}"#, "between 1"),
            (
                r#"{"width": 1280, "height": 20000}"#,
                "between 1",
            ),
            (
                r#"{"width": 1280, "height": 800, "device_scale_factor": 0}"#,
                "positive number",
            ),
        ];
        for (arguments, expected) in cases {
            let call = make_tool_call(
                ToolName::namespaced("browser", "set_viewport"),
                arguments,
                None,
            );
            let Err(err) = ToolExecutor::handle(&tool, call).await else {
                panic!("{arguments} must be rejected");
            };
            assert!(
                matches!(err, FunctionCallError::RespondToModel(_)),
                "{arguments}: validation errors are model-correctable: {err:?}"
            );
            assert!(err.to_string().contains(expected), "{arguments}: {err}");
        }
    }

    /// Validation happens before the browser is touched: an uninitialized state
    /// still yields the state error, which proves the request was well formed.
    #[tokio::test]
    async fn set_viewport_reaches_the_browser_once_valid() {
        let tool = BrowserSetViewportTool::new(uninitialized_state());
        for arguments in [
            r#"{"width": 1280, "height": 800}"#,
            r#"{"reset": true}"#,
        ] {
            let call = make_tool_call(
                ToolName::namespaced("browser", "set_viewport"),
                arguments,
                None,
            );
            let Err(err) = ToolExecutor::handle(&tool, call).await else {
                panic!("{arguments} must reach the uninitialized browser");
            };
            assert!(
                !matches!(err, FunctionCallError::RespondToModel(_)),
                "{arguments} is valid and must not be rejected as input: {err:?}"
            );
        }
    }
}
