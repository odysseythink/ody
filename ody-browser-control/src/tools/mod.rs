use std::sync::Arc;

use ody_tools::{
    FunctionCallError, JsonToolOutput, ToolCall, ToolExecutor, ToolExecutorFuture, ToolExposure,
    ToolName, ToolOutput, ToolSpec, default_namespace_description,
    parse_tool_input_schema, ResponsesApiNamespace, ResponsesApiNamespaceTool, ResponsesApiTool,
};
use serde_json::json;

use crate::{
    BrowserControlApprovalTicket, BrowserControlError, BrowserControlMode, BrowserThreadState,
    types::{LogKind, LogLevel, Point, WaitCondition},
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
                        "enum": ["load", "domcontentloaded", "networkidle"],
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
}

impl ToolExecutor<ToolCall> for BrowserScreenshotTool {
    fn tool_name(&self) -> ToolName {
        namespaced_tool_name("screenshot")
    }

    fn spec(&self) -> ToolSpec {
        namespace_spec(
            "screenshot",
            "Capture a PNG screenshot of the current page.",
            json!({
                "type": "object",
                "properties": {
                    "full_page": {
                        "type": "boolean",
                        "description": "Capture the full page instead of the viewport."
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
            let result = state
                .screenshot(input.full_page)
                .await
                .map_err(map_error)?;
            Ok(wrap_output(
                serde_json::to_value(result).map_err(|e| FunctionCallError::Fatal(e.to_string()))?,
                "screenshot",
                serde_json::to_value(input).map_err(|e| FunctionCallError::Fatal(e.to_string()))?,
            ))
        })
    }
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
            tracing::info!(kind = ?input.kind, level = ?input.level);
            let result = state
                .read_logs(input.kind, input.level)
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

    use ody_protocol::models::ResponseInputItem;
    use ody_protocol::protocol::TruncationPolicy;
    use ody_tools::{NoopTurnItemEmitter, ToolPayload};
    use serde_json::Value;

    use crate::BrowserControlConfig;

    #[test]
    fn navigate_input_schema_is_valid() {
        let schema = json!({
            "type": "object",
            "properties": {
                "url": { "type": "string" },
                "wait_until": { "type": "string", "enum": ["load", "domcontentloaded", "networkidle"] }
            },
            "required": ["url"],
            "additionalProperties": false
        });
        let parsed = parse_tool_input_schema(&schema);
        assert!(parsed.is_ok(), "{parsed:?}");
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
}
