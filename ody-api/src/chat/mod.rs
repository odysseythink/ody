//! Chat Completions wire protocol support.
//!
//! Kimi (Moonshot), DeepSeek and GLM are OpenAI-compatible providers that speak
//! the `/chat/completions` API rather than the Responses API. This module owns
//! the translation between the internal [`ResponseItem`] model and the Chat
//! Completions wire format (request building here, streaming responses in
//! [`crate::sse::chat`]).

pub mod kimi_schema;
pub mod vendor;

use ody_protocol::models::ContentItem;
use ody_protocol::models::ReasoningItemContent;
use ody_protocol::models::ResponseItem;
use serde_json::Map;
use serde_json::Value;
use serde_json::json;
use tracing::debug;

pub use vendor::ChatVendor;

/// High-level description of a Chat Completions request. `core` builds one of
/// these (mirroring `build_responses_request`) and the endpoint client turns it
/// into the on-the-wire JSON body via [`ChatCompletionsRequest::to_wire`].
#[derive(Debug, Clone)]
pub struct ChatCompletionsRequest {
    pub model: String,
    /// System prompt / base instructions.
    pub instructions: String,
    /// Conversation history in the internal item model.
    pub input: Vec<ResponseItem>,
    /// Tool definitions in Responses-API JSON form (as produced by
    /// `create_tools_json_for_responses_api`).
    pub tools: Vec<Value>,
    pub parallel_tool_calls: bool,
    /// Reasoning effort hint ("low" | "medium" | "high"), when the model
    /// supports thinking.
    pub reasoning_effort: Option<String>,
    pub max_completion_tokens: Option<u64>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub stop: Vec<String>,
    /// Which provider's dialect to emit. Controls vendor-specific extensions.
    pub vendor: ChatVendor,
}

impl ChatCompletionsRequest {
    /// Build the JSON body sent to `POST /chat/completions`.
    pub fn to_wire(&self) -> Value {
        let mut messages: Vec<Value> = Vec::new();
        tracing::info!(
            model = %self.model,
            vendor = ?self.vendor,
            input_len = self.input.len(),
            "chat_completions:to_wire building request"
        );
        if !self.instructions.is_empty() {
            messages.push(json!({
                "role": "system",
                "content": self.instructions,
            }));
        }
        // Thinking models condition on their own prior reasoning. Reasoning is
        // a standalone item internally but has no standalone chat message, so
        // hold it until the assistant message it belongs to is emitted.
        let mut pending_reasoning: Option<String> = None;
        for (idx, item) in self.input.iter().enumerate() {
            if let ResponseItem::Reasoning { content, .. } = item {
                if self.vendor.accepts_reasoning_content()
                    && let Some(text) = reasoning_text(content.as_deref())
                {
                    pending_reasoning.get_or_insert_default().push_str(&text);
                }
                continue;
            }
            let before_len = messages.len();
            append_item_messages(item, &mut messages, self.vendor);
            if let Some(reasoning) = pending_reasoning.take() {
                // Only an assistant message can carry it. Anything else (a user
                // turn, a tool result) means the reasoning has no carrier and is
                // dropped, exactly as before this field existed.
                if let Some(message) = messages[before_len..]
                    .iter_mut()
                    .find(|message| is_assistant(message))
                    && let Some(object) = message.as_object_mut()
                {
                    object.insert("reasoning_content".into(), Value::String(reasoning));
                }
            }
            tracing::info!(
                idx,
                item_kind = %response_item_kind(item),
                messages_before = before_len,
                messages_after = messages.len(),
                "chat_completions:to_wire appended item"
            );
        }

        let tools = convert_tools(&self.tools, self.vendor);
        tracing::info!(
            messages_len = messages.len(),
            tools_len = tools.len(),
            "chat_completions:to_wire built messages"
        );

        // Chat Completions requires all tool_calls from a single assistant turn
        // to live in one assistant message, followed immediately by the tool
        // response messages. Coalesce consecutive assistant messages so that
        // a content-only assistant message followed by tool_calls, or multiple
        // consecutive tool_call assistant messages, become a single message.
        let messages = merge_consecutive_assistant_messages(messages);
        tracing::info!(
            messages_len = messages.len(),
            "chat_completions:to_wire merged assistant messages"
        );

        for (idx, message) in messages.iter().enumerate() {
            let role = message
                .get("role")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("?");
            let tool_call_id = message
                .get("tool_call_id")
                .and_then(serde_json::Value::as_str);
            let tool_calls_ids: Option<Vec<String>> = message.get("tool_calls").map(|tc| {
                tc.as_array()
                    .unwrap_or(&Vec::new())
                    .iter()
                    .map(|t| {
                        t.get("id")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("?")
                            .to_string()
                    })
                    .collect()
            });
            tracing::info!(
                idx,
                role,
                ?tool_call_id,
                ?tool_calls_ids,
                "chat_completions:to_wire message"
            );
            tracing::debug!(?message, "chat_completions:to_wire message detail");
        }

        let mut body = Map::new();
        body.insert("model".into(), Value::String(self.model.clone()));
        body.insert("messages".into(), Value::Array(messages));
        body.insert("stream".into(), Value::Bool(true));
        // Ask the server to include a final usage chunk when streaming.
        body.insert("stream_options".into(), json!({ "include_usage": true }));

        if !tools.is_empty() {
            body.insert("tools".into(), Value::Array(tools));
            body.insert("tool_choice".into(), Value::String("auto".into()));
            body.insert(
                "parallel_tool_calls".into(),
                Value::Bool(self.parallel_tool_calls),
            );
        }
        if let Some(temperature) = self.temperature
            && let Some(temperature) = serde_json::Number::from_f64(temperature)
        {
            body.insert("temperature".into(), Value::Number(temperature));
        }
        if let Some(top_p) = self.top_p
            && let Some(top_p) = serde_json::Number::from_f64(top_p)
        {
            body.insert("top_p".into(), Value::Number(top_p));
        }
        if let Some(max_completion_tokens) = self.max_completion_tokens {
            body.insert(
                self.vendor.max_tokens_field().into(),
                Value::Number(max_completion_tokens.into()),
            );
        }
        if !self.stop.is_empty() {
            body.insert(
                "stop".into(),
                Value::Array(self.stop.iter().cloned().map(Value::String).collect()),
            );
        }
        if let Some(effort) = &self.reasoning_effort
            && self.vendor.emits_reasoning_effort()
        {
            body.insert("reasoning_effort".into(), Value::String(effort.clone()));
        }

        let mut body = Value::Object(body);
        self.vendor.apply_request(&mut body, self);
        log_invalid_function_names(&body);
        body
    }
}

/// Warn about function names that providers such as Kimi/Moonshot reject.
fn log_invalid_function_names(body: &Value) {
    let Some(messages) = body.get("messages").and_then(Value::as_array) else {
        return;
    };
    for (idx, message) in messages.iter().enumerate() {
        if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
            for (tc_idx, tool_call) in tool_calls.iter().enumerate() {
                if let Some(name) = tool_call
                    .get("function")
                    .and_then(|f| f.get("name"))
                    .and_then(Value::as_str)
                {
                    if !is_valid_function_name(name) {
                        tracing::warn!(
                            message_idx = idx,
                            tool_call_idx = tc_idx,
                            function_name = name,
                            "function call name on wire may be rejected by provider"
                        );
                    }
                }
            }
        }
        if let Some(name) = message.get("name").and_then(Value::as_str) {
            if !is_valid_function_name(name) {
                tracing::warn!(
                    message_idx = idx,
                    function_name = name,
                    "function message name on wire may be rejected by provider"
                );
            }
        }
    }
    if let Some(tools) = body.get("tools").and_then(Value::as_array) {
        for (idx, tool) in tools.iter().enumerate() {
            if let Some(name) = tool
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(Value::as_str)
            {
                if !is_valid_function_name(name) {
                    tracing::warn!(
                        tool_idx = idx,
                        tool_name = name,
                        "tool definition name on wire may be rejected by provider"
                    );
                }
            }
        }
    }
}

fn is_valid_function_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Convert a single internal [`ResponseItem`] into zero or more chat messages,
/// appending them to `messages`.
fn is_assistant(message: &Value) -> bool {
    message
        .get("role")
        .and_then(Value::as_str)
        .is_some_and(|role| role == "assistant")
}

/// Flattens a reasoning item's content into the text to replay as
/// `reasoning_content`. `None` when there is nothing to replay.
fn reasoning_text(content: Option<&[ReasoningItemContent]>) -> Option<String> {
    let text = content?
        .iter()
        .map(|part| match part {
            ReasoningItemContent::ReasoningText { text } | ReasoningItemContent::Text { text } => {
                text.as_str()
            }
        })
        .collect::<String>();
    (!text.is_empty()).then_some(text)
}

fn append_item_messages(item: &ResponseItem, messages: &mut Vec<Value>, vendor: ChatVendor) {
    match item {
        ResponseItem::Message { role, content, .. } => {
            let role = normalize_role(role);
            messages.push(json!({
                "role": role,
                "content": content_to_value(content),
            }));
        }
        ResponseItem::FunctionCall {
            name,
            arguments,
            call_id,
            ..
        } => {
            tracing::info!(call_id = %call_id, tool_name = %name, "chat_completions: emitting assistant tool_calls");
            messages.push(json!({
                "role": "assistant",
                "content": Value::Null,
                "tool_calls": [{
                    "id": vendor.sanitize_tool_call_id(call_id),
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": arguments,
                    },
                }],
            }));
        }
        ResponseItem::CustomToolCall {
            name,
            input,
            call_id,
            ..
        } => {
            messages.push(json!({
                "role": "assistant",
                "content": Value::Null,
                "tool_calls": [{
                    "id": vendor.sanitize_tool_call_id(call_id),
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": input,
                    },
                }],
            }));
        }
        ResponseItem::FunctionCallOutput {
            call_id, output, ..
        } => {
            tracing::info!(call_id = %call_id, "chat_completions: emitting tool response");
            messages.push(json!({
                "role": "tool",
                "tool_call_id": vendor.sanitize_tool_call_id(call_id),
                "content": output.to_string(),
            }));
        }
        ResponseItem::CustomToolCallOutput {
            call_id, output, ..
        } => {
            messages.push(json!({
                "role": "tool",
                "tool_call_id": vendor.sanitize_tool_call_id(call_id),
                "content": output.to_string(),
            }));
        }
        // Reasoning is handled by the caller (replayed as `reasoning_content`
        // on the assistant message it precedes) and never reaches here.
        // Responses-only items (shell calls, web search, image generation,
        // compaction, ...) have no chat representation.
        other => {
            debug!(
                "dropping unsupported response item for chat completions: {}",
                response_item_kind(other)
            );
        }
    }
}

/// Map internal roles onto chat roles. Chat Completions has no `developer`
/// role, so fold it into `system`.
fn normalize_role(role: &str) -> &str {
    match role {
        "developer" => "system",
        other => other,
    }
}

/// Render message content. When all parts are plain text we emit a string;
/// when an image is present we emit the structured content-parts array.
fn content_to_value(content: &[ContentItem]) -> Value {
    let has_image = content
        .iter()
        .any(|c| matches!(c, ContentItem::InputImage { .. }));

    if !has_image {
        let mut text = String::new();
        for item in content {
            match item {
                ContentItem::InputText { text: t } | ContentItem::OutputText { text: t } => {
                    text.push_str(t);
                }
                ContentItem::InputImage { .. } => {}
            }
        }
        return Value::String(text);
    }

    let mut parts: Vec<Value> = Vec::new();
    for item in content {
        match item {
            ContentItem::InputText { text } | ContentItem::OutputText { text } => {
                parts.push(json!({ "type": "text", "text": text }));
            }
            ContentItem::InputImage { image_url, .. } => {
                parts.push(json!({
                    "type": "image_url",
                    "image_url": { "url": image_url },
                }));
            }
        }
    }
    Value::Array(parts)
}

/// Convert Responses-API tool JSON into Chat Completions tool JSON.
///
/// Function tools are rewritten from the flat Responses shape
/// (`{type, name, description, parameters}`) into the nested chat shape
/// (`{type:"function", function:{name, description, parameters}}`). The vendor
/// adapter gets a chance to rewrite each tool (e.g. Kimi's `builtin_function`).
fn convert_tools(tools: &[Value], vendor: ChatVendor) -> Vec<Value> {
    let mut out = Vec::new();
    for tool in tools {
        if let Some(converted) = convert_tool(tool, vendor) {
            out.push(converted);
        }
    }
    out
}

fn convert_tool(tool: &Value, vendor: ChatVendor) -> Option<Value> {
    if let Some(custom) = vendor.convert_tool(tool) {
        return Some(custom);
    }

    let obj = tool.as_object()?;
    let kind = obj
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("function");
    match kind {
        "function" => {
            let mut function = Map::new();
            if let Some(name) = obj.get("name") {
                function.insert("name".into(), name.clone());
            }
            if let Some(description) = obj.get("description") {
                function.insert("description".into(), description.clone());
            }
            if let Some(parameters) = obj.get("parameters") {
                function.insert(
                    "parameters".into(),
                    vendor.normalize_tool_parameters(parameters),
                );
            }
            Some(json!({ "type": "function", "function": Value::Object(function) }))
        }
        other => {
            debug!("dropping unsupported tool type for chat completions: {other}");
            None
        }
    }
}

fn response_item_kind(item: &ResponseItem) -> &'static str {
    match item {
        ResponseItem::Message { .. } => "message",
        ResponseItem::AgentMessage { .. } => "agent_message",
        ResponseItem::Reasoning { .. } => "reasoning",
        ResponseItem::LocalShellCall { .. } => "local_shell_call",
        ResponseItem::FunctionCall { .. } => "function_call",
        ResponseItem::ToolSearchCall { .. } => "tool_search_call",
        ResponseItem::FunctionCallOutput { .. } => "function_call_output",
        ResponseItem::CustomToolCall { .. } => "custom_tool_call",
        ResponseItem::CustomToolCallOutput { .. } => "custom_tool_call_output",
        ResponseItem::ToolSearchOutput { .. } => "tool_search_output",
        ResponseItem::WebSearchCall { .. } => "web_search_call",
        ResponseItem::ImageGenerationCall { .. } => "image_generation_call",
        ResponseItem::Compaction { .. } => "compaction",
        ResponseItem::CompactionTrigger { .. } => "compaction_trigger",
        ResponseItem::ContextCompaction { .. } => "context_compaction",
        ResponseItem::Other => "other",
    }
}

/// Coalesce consecutive assistant messages so that a single assistant turn
/// (which may contain both content and one or more tool_calls) is emitted as
/// one Chat Completions message, followed immediately by tool response
/// messages.
fn reasoning_of(message: &Value) -> Option<&str> {
    message.get("reasoning_content").and_then(Value::as_str)
}

/// Concatenation of both messages' replayed reasoning, in order.
fn joined_reasoning(first: &Value, second: &Value) -> Option<String> {
    match (reasoning_of(first), reasoning_of(second)) {
        (Some(a), Some(b)) => Some(format!("{a}{b}")),
        (Some(only), None) | (None, Some(only)) => Some(only.to_string()),
        (None, None) => None,
    }
}

/// Moves `from`'s reasoning onto `into` when merging collapses `from` away, so
/// that replayed thinking survives the collapse.
fn carry_reasoning(from: &Value, into: &mut Value) {
    if let Some(reasoning) = joined_reasoning(from, into)
        && let Some(object) = into.as_object_mut()
    {
        object.insert("reasoning_content".into(), Value::String(reasoning));
    }
}

fn merge_consecutive_assistant_messages(messages: Vec<Value>) -> Vec<Value> {
    let mut merged: Vec<Value> = Vec::with_capacity(messages.len());

    for mut message in messages {
        if message
            .get("role")
            .and_then(Value::as_str)
            .is_some_and(|r| r == "assistant")
        {
            if let Some(last) = merged.last_mut() {
                if last
                    .get("role")
                    .and_then(Value::as_str)
                    .is_some_and(|r| r == "assistant")
                {
                    let last_has_tools = last
                        .get("tool_calls")
                        .and_then(Value::as_array)
                        .is_some_and(|a| !a.is_empty());
                    let msg_has_tools = message
                        .get("tool_calls")
                        .and_then(Value::as_array)
                        .is_some_and(|a| !a.is_empty());

                    if msg_has_tools {
                        if !last_has_tools {
                            // Content-only assistant message immediately
                            // followed by tool_calls: fold the content into
                            // the tool_calls message.
                            if message.get("content").map_or(true, Value::is_null) {
                                if let Some(obj) = message.as_object_mut() {
                                    if let Some(content) = last.get("content") {
                                        obj.insert("content".into(), content.clone());
                                    }
                                }
                            }
                            carry_reasoning(last, &mut message);
                            *last = message;
                            continue;
                        }

                        // Both messages are assistant tool_calls: combine
                        // into one assistant message.
                        let merged_reasoning = joined_reasoning(last, &message);
                        if let (Some(last_tc), Some(msg_tc)) =
                            (last.get_mut("tool_calls"), message.get("tool_calls"))
                        {
                            if let (Some(last_arr), Some(msg_arr)) =
                                (last_tc.as_array_mut(), msg_tc.as_array())
                            {
                                last_arr.extend(msg_arr.iter().cloned());
                                if let Some(reasoning) = merged_reasoning
                                    && let Some(obj) = last.as_object_mut()
                                {
                                    obj.insert(
                                        "reasoning_content".into(),
                                        Value::String(reasoning),
                                    );
                                }
                                continue;
                            }
                        }
                    }
                }
            }
        }
        merged.push(message);
    }

    merged
}

#[cfg(test)]
#[path = "chat_tests.rs"]
mod chat_tests;
