//! Anthropic Messages wire protocol (`POST /v1/messages`).
//!
//! Two deployment shapes share this wire:
//! - Direct Anthropic API: `{base_url}/v1/messages`, `x-api-key` auth, SSE
//!   streaming selected by `"stream": true` in the body.
//! - AWS Bedrock Claude: `{bedrock-runtime}/model/{model}/invoke-with-response-stream`,
//!   SigV4-signed, `"anthropic_version": "bedrock-2023-05-31"` in the body and
//!   NO `stream` field (the operation itself implies streaming).
//!
//! The `bedrock` flag on [`AnthropicMessagesRequest`] selects the Bedrock
//! dialect; everything else is identical, and the streaming parser consumes
//! the same Anthropic event JSON in both modes.

use ody_protocol::model_metadata::InputModality;
use ody_protocol::models::ContentItem;
use ody_protocol::models::FunctionCallOutputPayload;
use ody_protocol::models::ResponseItem;
use serde_json::json;
use serde_json::Map;
use serde_json::Value;

/// High-level description of an Anthropic Messages request. Mirrors
/// [`crate::chat::ChatCompletionsRequest`]: `core` builds one of these and
/// [`AnthropicMessagesRequest::to_wire`] renders the on-the-wire JSON.
#[derive(Debug, Clone)]
pub struct AnthropicMessagesRequest {
    pub model: String,
    /// Input modalities accepted by the selected model.
    pub input_modalities: Vec<InputModality>,
    /// System prompt / base instructions (top-level `system` field).
    pub instructions: String,
    /// Conversation history in the internal item model.
    pub input: Vec<ResponseItem>,
    /// Tool definitions in Responses-API JSON form (as produced by
    /// `create_tools_json_for_responses_api`).
    pub tools: Vec<Value>,
    pub max_completion_tokens: Option<u64>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub stop: Vec<String>,
    /// Reasoning effort hint ("low" | "medium" | "high"), when the model
    /// supports thinking.
    pub reasoning_effort: Option<String>,
    /// AWS Bedrock Claude dialect (`anthropic_version`, no `stream` field).
    pub bedrock: bool,
}

const DEFAULT_MAX_TOKENS: u64 = 8192;
const THINKING_BUDGET_LOW: u64 = 1024;
const THINKING_BUDGET_MEDIUM: u64 = 4096;
const THINKING_BUDGET_HIGH: u64 = 10000;
/// Thinking `budget_tokens` must stay below `max_tokens`; leave headroom.
const THINKING_MAX_TOKENS_HEADROOM: u64 = 1;

/// `anthropic_version` required by Bedrock's invoke operations.
pub const BEDROCK_ANTHROPIC_VERSION: &str = "bedrock-2023-05-31";

impl AnthropicMessagesRequest {
    /// Build the JSON body sent to `POST /v1/messages` (or Bedrock's
    /// invoke-with-response-stream).
    pub fn to_wire(&self) -> Value {
        tracing::info!(
            model = %self.model,
            bedrock = self.bedrock,
            input_len = self.input.len(),
            "anthropic:to_wire building request"
        );

        let mut system: Vec<Value> = Vec::new();
        if !self.instructions.is_empty() {
            system.push(json!({ "type": "text", "text": self.instructions }));
        }

        let mut messages: Vec<Value> = Vec::new();
        for item in &self.input {
            append_item_message(item, &mut messages, &mut system, &self.input_modalities);
        }
        ensure_user_first(&mut messages);

        let tools = convert_tools(&self.tools);
        tracing::info!(
            messages_len = messages.len(),
            tools_len = tools.len(),
            "anthropic:to_wire built messages"
        );

        // `max_tokens` is required by the Anthropic Messages API. Thinking
        // budgets must stay strictly below it, so a thinking request raises
        // the default ceiling to fit the budget.
        let thinking_budget = self.thinking_budget();
        let default_max_tokens = match thinking_budget {
            Some(budget) => DEFAULT_MAX_TOKENS.max(budget + THINKING_MAX_TOKENS_HEADROOM + 1),
            None => DEFAULT_MAX_TOKENS,
        };
        let max_tokens = self.max_completion_tokens.unwrap_or(default_max_tokens);

        let mut body = Map::new();
        body.insert("model".into(), Value::String(self.model.clone()));
        body.insert("max_tokens".into(), json!(max_tokens));
        if !messages.is_empty() {
            body.insert("messages".into(), Value::Array(messages));
        }
        if !system.is_empty() {
            body.insert("system".into(), Value::Array(system));
        }
        if !tools.is_empty() {
            body.insert("tools".into(), Value::Array(tools));
        }
        if let Some(budget) = thinking_budget {
            let budget = budget.min(max_tokens.saturating_sub(THINKING_MAX_TOKENS_HEADROOM));
            if budget >= THINKING_BUDGET_LOW {
                body.insert(
                    "thinking".into(),
                    json!({ "type": "enabled", "budget_tokens": budget }),
                );
            }
        }
        if self.bedrock {
            body.insert(
                "anthropic_version".into(),
                Value::String(BEDROCK_ANTHROPIC_VERSION.into()),
            );
        } else {
            body.insert("stream".into(), Value::Bool(true));
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
        if !self.stop.is_empty() {
            body.insert(
                "stop_sequences".into(),
                Value::Array(self.stop.iter().cloned().map(Value::String).collect()),
            );
        }

        let body = Value::Object(body);
        log_invalid_tool_names(&body);
        body
    }

    /// Map the reasoning effort hint onto a thinking budget. `None` when
    /// thinking is off.
    fn thinking_budget(&self) -> Option<u64> {
        match self.reasoning_effort.as_deref() {
            Some("low") => Some(THINKING_BUDGET_LOW),
            Some("medium") => Some(THINKING_BUDGET_MEDIUM),
            Some("high") | Some("xhigh") | Some("max") => Some(THINKING_BUDGET_HIGH),
            _ => None,
        }
    }
}

/// Convert a single internal [`ResponseItem`] into Anthropic message shape.
///
/// Anthropic requires strict user/assistant alternation with tool traffic
/// inline: `tool_use` blocks ride in an assistant message, `tool_result`
/// blocks in a user message. Consecutive blocks of the same role are merged
/// into one message; reasoning items have no replay slot in the Messages API
/// and are dropped (interleaved thinking is out of scope).
fn append_item_message(
    item: &ResponseItem,
    messages: &mut Vec<Value>,
    system: &mut Vec<Value>,
    input_modalities: &[InputModality],
) {
    match item {
        ResponseItem::Message { role, content, .. } => match role.as_str() {
            "user" => {
                let blocks = content_to_blocks(content, input_modalities);
                push_role_blocks(messages, "user", blocks);
            }
            "assistant" => {
                let blocks = content_to_blocks(content, input_modalities);
                push_role_blocks(messages, "assistant", blocks);
            }
            // Anthropic has no in-thread system/developer role; fold into the
            // top-level system array.
            _ => {
                for block in content_to_blocks(content, input_modalities) {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        system.push(json!({ "type": "text", "text": text }));
                    }
                }
            }
        },
        ResponseItem::FunctionCall {
            name,
            arguments,
            call_id,
            ..
        } => {
            let input: Value = serde_json::from_str(arguments)
                .unwrap_or_else(|_| json!({ "arguments": arguments }));
            push_tool_use(messages, call_id, name, input);
        }
        ResponseItem::CustomToolCall {
            name,
            input,
            call_id,
            ..
        } => {
            let parsed: Value =
                serde_json::from_str(input).unwrap_or_else(|_| json!({ "input": input }));
            push_tool_use(messages, call_id, name, parsed);
        }
        ResponseItem::FunctionCallOutput { call_id, output, .. }
        | ResponseItem::CustomToolCallOutput { call_id, output, .. } => {
            push_role_blocks(
                messages,
                "user",
                vec![json!({
                    "type": "tool_result",
                    "tool_use_id": call_id,
                    "content": tool_output_content(output, input_modalities),
                })],
            );
        }
        // Reasoning has no replay slot in the Messages API; Responses-only
        // items (shell calls, web search, image generation, compaction, ...)
        // have no Anthropic representation either.
        other => {
            tracing::debug!(
                "dropping unsupported response item for anthropic messages: {}",
                response_item_kind(other)
            );
        }
    }
}

/// Emit a tool_use block in an (merged) assistant message.
fn push_tool_use(messages: &mut Vec<Value>, call_id: &str, name: &str, input: Value) {
    push_role_blocks(
        messages,
        "assistant",
        vec![json!({
            "type": "tool_use",
            "id": call_id,
            "name": name,
            "input": input,
        })],
    );
}
/// Append content blocks to `messages`, merging with the trailing message
/// when it has the same role so consecutive assistant turns (text followed
/// by tool calls, or several tool calls) stay one message.
fn push_role_blocks(messages: &mut Vec<Value>, role: &str, blocks: Vec<Value>) {
    if blocks.is_empty() {
        return;
    }
    if let Some(last) = messages.last_mut()
        && let Some(object) = last.as_object_mut()
        && object.get("role").and_then(Value::as_str) == Some(role)
        && let Some(content) = object.get_mut("content").and_then(Value::as_array_mut)
    {
        content.extend(blocks);
        return;
    }
    messages.push(json!({ "role": role, "content": blocks }));
}

/// Anthropic rejects conversations whose first message is not from the user.
fn ensure_user_first(messages: &mut Vec<Value>) {
    if messages
        .first()
        .and_then(|message| message.get("role"))
        .and_then(Value::as_str)
        .is_some_and(|role| role != "user")
    {
        messages.insert(
            0,
            json!({ "role": "user", "content": [{ "type": "text", "text": "(conversation start)" }] }),
        );
    }
}

/// Render message content blocks, retaining only media modalities the
/// selected model declares. Unsupported media degrades to a text placeholder.
fn content_to_blocks(content: &[ContentItem], input_modalities: &[InputModality]) -> Vec<Value> {
    let supports_images = input_modalities.contains(&InputModality::Image);
    content
        .iter()
        .filter_map(|item| match item {
            ContentItem::InputText { text } | ContentItem::OutputText { text } => {
                // Empty text blocks add nothing but noise (and an empty
                // assistant text block would ride along tool_use blocks).
                (!text.is_empty()).then(|| json!({ "type": "text", "text": text }))
            }
            ContentItem::InputImage { image_url, .. } if supports_images => {
                Some(json!({
                    "type": "image",
                    "source": { "type": "url", "url": image_url },
                }))
            }
            ContentItem::InputImage { .. } => {
                Some(json!({ "type": "text", "text": unsupported_media_placeholder(InputModality::Image) }))
            }
            ContentItem::InputAudio { .. } => {
                Some(json!({ "type": "text", "text": unsupported_media_placeholder(InputModality::Audio) }))
            }
            ContentItem::InputVideo { .. } => {
                Some(json!({ "type": "text", "text": unsupported_media_placeholder(InputModality::Video) }))
            }
            ContentItem::InputFile { .. } => None,
        })
        .collect()
}

fn tool_output_content(
    output: &FunctionCallOutputPayload,
    input_modalities: &[InputModality],
) -> Value {
    // Anthropic tool_result content accepts the same text/blocks shape as
    // user messages; unsupported media degrades to a placeholder via to_text.
    let _ = input_modalities;
    match output.body.to_text() {
        Some(text) => Value::String(text),
        None => Value::String(
            "<media content omitted because this anthropic wire does not support media tool results>"
                .to_string(),
        ),
    }
}

fn unsupported_media_placeholder(modality: InputModality) -> String {
    format!("{modality} content omitted because you do not support {modality} input")
}

/// Convert Responses-API flat tool definitions into Anthropic tool shape.
/// Anthropic has no namespaced tools; the (already unique) flat name is used.
fn convert_tools(tools: &[Value]) -> Vec<Value> {
    tools
        .iter()
        .filter_map(|tool| {
            let obj = tool.as_object()?;
            let kind = obj.get("type").and_then(Value::as_str).unwrap_or("function");
            if kind != "function" {
                tracing::debug!("dropping unsupported tool type for anthropic messages: {kind}");
                return None;
            }
            Some(json!({
                "name": obj.get("name").cloned().unwrap_or(Value::Null),
                "description": obj.get("description").cloned().unwrap_or(Value::String(String::new())),
                "input_schema": obj.get("parameters").cloned().unwrap_or_else(|| json!({ "type": "object" })),
            }))
        })
        .collect()
}

fn response_item_kind(item: &ResponseItem) -> &'static str {
    match item {
        ResponseItem::Message { .. } => "message",
        ResponseItem::Reasoning { .. } => "reasoning",
        ResponseItem::FunctionCall { .. } => "function_call",
        ResponseItem::FunctionCallOutput { .. } => "function_call_output",
        ResponseItem::CustomToolCall { .. } => "custom_tool_call",
        ResponseItem::CustomToolCallOutput { .. } => "custom_tool_call_output",
        _ => "other",
    }
}

/// Warn about tool names Anthropic rejects (must match `[a-zA-Z0-9_-]{1,128}`,
/// starting with a letter or underscore).
fn log_invalid_tool_names(body: &Value) {
    let Some(tools) = body.get("tools").and_then(Value::as_array) else {
        return;
    };
    for tool in tools {
        let Some(name) = tool.get("name").and_then(Value::as_str) else {
            continue;
        };
        let mut chars = name.chars();
        let valid = matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            && name.len() <= 128;
        if !valid {
            tracing::warn!(
                tool_name = name,
                "tool name on wire may be rejected by anthropic"
            );
        }
    }
}

#[cfg(test)]
#[path = "anthropic_tests.rs"]
mod anthropic_tests;
