//! Adapter for the Chat Completions wire API.
//!
//! Wraps `ody_api::ChatCompletionsClient` and normalizes its SSE stream to
//! the provider-neutral `ChatEvent` model. Used for Kimi, DeepSeek, GLM and
//! other OpenAI-compatible providers.

use crate::adapters::common::{self, chat_provider_error_from_api_error};
use crate::chat_provider::{
    ChatProvider, ChatProviderError, ChatRequest, ChatStream, ProviderCapabilities, ProviderId,
    ThinkingEffort, clamp_thinking_effort,
};
use base64::Engine;
use futures::StreamExt;
use ody_api::chat::{ChatCompletionsRequest, ChatVendor};
use ody_api::{
    ChatCompletionsClient, ChatCompletionsOptions, Compression, Provider as ApiProvider,
    SharedAuthProvider,
};
use ody_client::HttpTransport;
use ody_protocol::models::ResponseItem;

/// Adapter for the Chat Completions API.
pub struct ChatAdapter<T: HttpTransport> {
    provider_id: ProviderId,
    vendor: ChatVendor,
    capabilities: ProviderCapabilities,
    client: ChatCompletionsClient<T>,
}

impl<T: HttpTransport> ChatAdapter<T> {
    /// Construct from an `ody_api` transport, provider, auth provider, and vendor dialect.
    pub fn new(
        transport: T,
        api_provider: ApiProvider,
        auth: SharedAuthProvider,
        vendor: ChatVendor,
    ) -> Self {
        let client = ChatCompletionsClient::new(transport, api_provider.clone(), auth);
        Self {
            provider_id: "chat",
            vendor,
            capabilities: capabilities_from_vendor(vendor),
            client,
        }
    }

    /// Override the default provider id.
    pub fn with_provider_id(mut self, provider_id: ProviderId) -> Self {
        self.provider_id = provider_id;
        self
    }
}

fn capabilities_from_vendor(vendor: ChatVendor) -> ProviderCapabilities {
    match vendor {
        ChatVendor::Kimi => ProviderCapabilities {
            supports_streaming: true,
            supports_tools: true,
            supports_thinking: false,
            supports_vision: true,
            supports_multiple_system_messages: true,
            supports_turn_pause: false,
            max_context_tokens: Some(256_000),
            max_output_tokens: Some(8_192),
            thinking_effort: vec![],
        },
        ChatVendor::DeepSeek => ProviderCapabilities {
            supports_streaming: true,
            supports_tools: true,
            supports_thinking: true,
            supports_vision: false,
            supports_multiple_system_messages: true,
            supports_turn_pause: false,
            max_context_tokens: Some(64_000),
            max_output_tokens: Some(8_192),
            thinking_effort: vec![
                ThinkingEffort::Low,
                ThinkingEffort::Medium,
                ThinkingEffort::High,
            ],
        },
        ChatVendor::Glm => ProviderCapabilities {
            supports_streaming: true,
            supports_tools: true,
            supports_thinking: false,
            supports_vision: true,
            supports_multiple_system_messages: true,
            supports_turn_pause: false,
            max_context_tokens: Some(128_000),
            max_output_tokens: Some(8_192),
            thinking_effort: vec![],
        },
        ChatVendor::Generic => ProviderCapabilities {
            supports_streaming: true,
            supports_tools: true,
            supports_thinking: false,
            supports_vision: false,
            supports_multiple_system_messages: true,
            supports_turn_pause: false,
            max_context_tokens: Some(128_000),
            max_output_tokens: Some(4_096),
            thinking_effort: vec![],
        },
    }
}

fn build_api_request(
    request: ChatRequest,
    vendor: ChatVendor,
) -> Result<ChatCompletionsRequest, ChatProviderError> {
    use crate::chat_provider::Role;
    let instructions = request
        .messages
        .iter()
        .find(|m| matches!(m.role, Role::System))
        .map(|m| content_to_text(&m.content))
        .unwrap_or_default();

    let input: Vec<ResponseItem> = request
        .messages
        .into_iter()
        .filter(|m| !matches!(m.role, Role::System))
        .flat_map(message_to_response_items)
        .collect();

    let tools = request
        .tools
        .into_iter()
        .map(tool_definition_to_value)
        .collect::<Result<Vec<_>, _>>()?;

    log_invalid_function_names(&tools, vendor);
    log_invalid_message_function_names(&input, vendor);

    let reasoning_effort = reasoning_effort_for_request(request.thinking_effort, vendor)?;

    Ok(ChatCompletionsRequest {
        model: request.model,
        instructions,
        input,
        tools,
        parallel_tool_calls: true,
        reasoning_effort,
        max_completion_tokens: request.max_tokens.map(|v| v as u64),
        temperature: request.temperature.map(|v| v as f64),
        top_p: request.top_p.map(|v| v as f64),
        stop: request.stop,
        vendor,
    })
}

fn reasoning_effort_for_request(
    thinking_effort: ThinkingEffort,
    vendor: ChatVendor,
) -> Result<Option<String>, ChatProviderError> {
    if thinking_effort == ThinkingEffort::Off {
        return Ok(None);
    }
    let supported = match vendor {
        ChatVendor::DeepSeek => vec![
            ThinkingEffort::Low,
            ThinkingEffort::Medium,
            ThinkingEffort::High,
        ],
        _ => vec![],
    };
    let effort = clamp_thinking_effort(thinking_effort, &supported).ok_or_else(|| {
        ChatProviderError::Unsupported {
            capability: "thinking effort".into(),
        }
    })?;
    if effort == ThinkingEffort::Off {
        return Ok(None);
    }
    let value = match effort {
        ThinkingEffort::Low => "low",
        ThinkingEffort::Medium => "medium",
        ThinkingEffort::High => "high",
        _ => unreachable!("clamp_thinking_effort should not return unsupported effort"),
    };
    Ok(Some(value.to_string()))
}

fn content_to_text(content: &[crate::chat_provider::ContentPart]) -> String {
    content
        .iter()
        .filter_map(|part| match part {
            crate::chat_provider::ContentPart::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

fn message_to_response_items(message: crate::chat_provider::Message) -> Vec<ResponseItem> {
    use crate::chat_provider::{ContentPart, Role};

    // Tool messages → FunctionCallOutput
    if message.role == Role::Tool {
        if let Some(call_id) = message.tool_call_id {
            let text = message
                .content
                .iter()
                .filter_map(|p| match p {
                    ContentPart::Text(t) => Some(t.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("");
            return vec![ResponseItem::FunctionCallOutput {
                id: None,
                call_id,
                output: ody_protocol::models::FunctionCallOutputPayload::from_text(text),
                internal_chat_message_metadata_passthrough: None,
            }];
        }
        // Fallback: tool message without `tool_call_id` → skip (or emit as
        // user, but that would be incorrect for Chat Completions wire).
        return vec![];
    }

    let role = match message.role {
        Role::User => "user",
        Role::Developer => "system",
        Role::Assistant => "assistant",
        Role::Tool => unreachable!("handled above"),
        Role::System => "system",
    }
    .to_string();

    // Build content items from text/image/reasoning parts.
    // ToolResult parts within assistant/user messages are rendered as text.
    let mut content = Vec::new();
    let mut reasoning = String::new();
    for part in message.content {
        match part {
            ContentPart::Text(text) => {
                content.push(ody_protocol::models::ContentItem::InputText { text })
            }
            ContentPart::Image { mime, bytes } => {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                content.push(ody_protocol::models::ContentItem::InputImage {
                    image_url: format!("data:{mime};base64,{b64}"),
                    detail: None,
                });
            }
            // Reasoning is not message content: flattening it into text would
            // replay the model's private thinking as if it were spoken. Keep it
            // as a Reasoning item so the wire layer can place it correctly (or
            // drop it, for vendors that do not accept it).
            ContentPart::Reasoning(text) => reasoning.push_str(&text),
            ContentPart::ToolResult {
                tool_call_id,
                content: parts,
            } => {
                let text = parts
                    .iter()
                    .filter_map(|p| match p {
                        ContentPart::Text(t) => Some(t.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                content.push(ody_protocol::models::ContentItem::InputText {
                    text: format!("tool result ({}): {}", tool_call_id, text),
                });
            }
        }
    }

    let content = if message.role == Role::Assistant {
        content
            .into_iter()
            .map(|item| match item {
                ody_protocol::models::ContentItem::InputText { text } => {
                    ody_protocol::models::ContentItem::OutputText { text }
                }
                other => other,
            })
            .collect()
    } else {
        content
    };

    let mut items = Vec::new();

    // Reasoning precedes the assistant output it produced; the wire layer
    // attaches it to the assistant message that follows it.
    if !reasoning.is_empty() {
        items.push(ResponseItem::Reasoning {
            id: None,
            summary: Vec::new(),
            content: Some(vec![
                ody_protocol::models::ReasoningItemContent::ReasoningText { text: reasoning },
            ]),
            encrypted_content: None,
            internal_chat_message_metadata_passthrough: None,
        });
    }

    // Emit a Message item for text content (only for assistant, where
    // content might coexist with tool_calls; for user/developer/system,
    // content is the only output and is never empty in practice).
    let has_text = content.iter().any(|c| match c {
        ody_protocol::models::ContentItem::OutputText { text } => !text.is_empty(),
        _ => true,
    });
    if has_text {
        items.push(ResponseItem::Message {
            id: None,
            role: role.clone(),
            content,
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        });
    }

    // Emit FunctionCall items for assistant tool_calls.
    for tc in message.tool_calls {
        items.push(ResponseItem::FunctionCall {
            id: None,
            name: tc.name,
            arguments: tc.arguments.to_string(),
            call_id: tc.id,
            namespace: tc.namespace,
            internal_chat_message_metadata_passthrough: None,
        });
    }

    items
}

fn tool_definition_to_value(
    def: crate::chat_provider::ToolDefinition,
) -> Result<serde_json::Value, ChatProviderError> {
    // Emit the Responses-API flat tool shape that `ody_api::chat::convert_tools`
    // expects; it will rewrite this into the nested Chat Completions shape on the
    // wire. Emitting the nested shape here causes `convert_tools` to drop the name,
    // which providers such as Kimi reject as an invalid function name.
    let mut tool = serde_json::json!({
        "type": "function",
        "name": def.name,
        "description": def.description,
        "parameters": def.schema,
        "strict": true,
    });
    if let Some(namespace) = def.namespace {
        tool
            .as_object_mut()
            .expect("tool value is an object")
            .insert("namespace".to_string(), serde_json::Value::String(namespace));
    }
    Ok(tool)
}

/// Some Chat Completions providers (notably Kimi/Moonshot) reject function names
/// that do not start with a letter or contain characters other than letters,
/// numbers, underscores, and dashes. Log any violating names at warning level so
/// the offending tool can be identified without guessing.
fn log_invalid_function_names(tools: &[serde_json::Value], vendor: ChatVendor) {
    if vendor != ChatVendor::Kimi {
        return;
    }
    for tool in tools {
        let Some(name) = tool.get("name").and_then(|n| n.as_str()) else {
            continue;
        };
        if !is_valid_function_name(name) {
            tracing::warn!(
                tool_name = name,
                "tool name may be rejected by provider: must start with a letter and contain only letters, numbers, underscores, and dashes"
            );
        }
    }
}

fn is_valid_function_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Also validate function names inside message history (assistant tool_calls and
/// tool response call_ids). Kimi rejects these with the same error even when the
/// tool definitions themselves are valid.
fn log_invalid_message_function_names(input: &[ResponseItem], vendor: ChatVendor) {
    if vendor != ChatVendor::Kimi {
        return;
    }
    for item in input {
        match item {
            ResponseItem::FunctionCall {
                name,
                call_id,
                arguments,
                ..
            } => {
                if !is_valid_function_name(name) {
                    tracing::warn!(
                        function_name = name,
                        call_id = call_id,
                        arguments = arguments,
                        "function call name in message history may be rejected by provider"
                    );
                }
            }
            ResponseItem::FunctionCallOutput {
                call_id, output, ..
            }
            | ResponseItem::CustomToolCallOutput {
                call_id, output, ..
            } => {
                if !is_valid_function_name(call_id) {
                    tracing::warn!(
                        call_id = call_id,
                        output = ?output,
                        "function call output call_id in message history may be rejected by provider"
                    );
                }
            }
            _ => {}
        }
    }
}

#[async_trait::async_trait]
impl<T: HttpTransport + 'static> ChatProvider for ChatAdapter<T> {
    fn provider_id(&self) -> ProviderId {
        self.provider_id
    }

    fn capabilities(&self) -> ProviderCapabilities {
        self.capabilities.clone()
    }

    async fn chat(&self, request: ChatRequest) -> Result<ChatStream, ChatProviderError> {
        let api_request = build_api_request(request, self.vendor)?;
        let options = ChatCompletionsOptions {
            compression: Compression::None,
            ..Default::default()
        };
        let stream = self
            .client
            .stream_request(api_request, options)
            .await
            .map_err(chat_provider_error_from_api_error)?;

        let mut state = common::NormalizeState::default();
        let mapped = stream.map(move |result: Result<ody_api::ResponseEvent, _>| -> ChatStream {
            match result
                .map_err(chat_provider_error_from_api_error)
                .and_then(|event| common::normalize_response_event_with_state(event, &mut state))
            {
                Ok(events) => Box::pin(futures::stream::iter(events.into_iter().map(Ok))),
                Err(e) => Box::pin(futures::stream::iter(std::iter::once(Err(e)))),
            }
        });
        Ok(Box::pin(mapped.flatten()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat_provider::{ChatEvent, ContentPart, Message, Role, ToolCall};
    use ody_protocol::models::ReasoningItemContent;

    struct BridgePrompt {
        input: Vec<ResponseItem>,
    }

    impl crate::adapters::core::Prompt for BridgePrompt {
        fn get_formatted_input_for_request(&self, _use_responses_lite: bool) -> Vec<ResponseItem> {
            self.input.clone()
        }
        fn tools(&self) -> &[ody_tools::ToolSpec] {
            &[]
        }
    }

    #[test]
    fn capabilities_for_kimi() {
        let caps = capabilities_from_vendor(ChatVendor::Kimi);
        assert!(caps.supports_streaming);
        assert!(caps.supports_tools);
    }

    #[test]
    fn deepseek_supports_thinking() {
        let caps = capabilities_from_vendor(ChatVendor::DeepSeek);
        assert!(caps.supports_thinking);
        assert_eq!(caps.thinking_effort.len(), 3);
    }

    #[test]
    fn build_request_preserves_model_and_temperature() {
        let request = ChatRequest {
            model: "kimi-k2".into(),
            temperature: Some(0.5),
            messages: vec![
                Message {
                    role: Role::System,
                    content: vec![ContentPart::Text("You are helpful".into())],
                    tool_calls: vec![],
                    tool_call_id: None,
                },
                Message {
                    role: Role::User,
                    content: vec![ContentPart::Text("hi".into())],
                    tool_calls: vec![],
                    tool_call_id: None,
                },
            ],
            ..Default::default()
        };
        let api_request = build_api_request(request, ChatVendor::Kimi).expect("builds");
        assert_eq!(api_request.model, "kimi-k2");
        assert_eq!(api_request.instructions, "You are helpful");
        assert_eq!(api_request.temperature, Some(0.5));
        assert_eq!(api_request.input.len(), 1);
    }

    #[test]
    fn build_request_ignores_thinking_for_unsupported_vendor() {
        let request = ChatRequest {
            model: "kimi-k2".into(),
            thinking_effort: ThinkingEffort::High,
            messages: vec![Message {
                role: Role::User,
                content: vec![ContentPart::Text("hi".into())],
                tool_calls: vec![],
                tool_call_id: None,
            }],
            ..Default::default()
        };
        let api_request = build_api_request(request, ChatVendor::Kimi).expect("builds");
        assert_eq!(api_request.reasoning_effort, None);
    }

    #[test]
    fn build_request_maps_deepseek_thinking() {
        let request = ChatRequest {
            model: "deepseek-chat".into(),
            thinking_effort: ThinkingEffort::High,
            messages: vec![Message {
                role: Role::User,
                content: vec![ContentPart::Text("hi".into())],
                tool_calls: vec![],
                tool_call_id: None,
            }],
            ..Default::default()
        };
        let api_request = build_api_request(request, ChatVendor::DeepSeek).expect("builds");
        assert_eq!(api_request.reasoning_effort, Some("high".to_string()));
    }

    #[test]
    fn build_request_maps_top_p_and_stop() {
        let request = ChatRequest {
            model: "kimi-k2".into(),
            temperature: Some(0.7),
            top_p: Some(0.9),
            stop: vec!["<|end|>".into()],
            messages: vec![Message {
                role: Role::User,
                content: vec![ContentPart::Text("hi".into())],
                tool_calls: vec![],
                tool_call_id: None,
            }],
            ..Default::default()
        };
        let api_request = build_api_request(request, ChatVendor::Kimi).expect("builds");
        assert!((api_request.temperature.unwrap() - 0.7).abs() < 1e-6);
        assert!((api_request.top_p.unwrap() - 0.9).abs() < 1e-6);
        assert_eq!(api_request.stop, vec!["<|end|>"]);
    }

    #[test]
    fn build_request_emits_responses_api_flat_tool_shape() {
        use crate::chat_provider::ToolDefinition;

        let request = ChatRequest {
            model: "kimi-k2".into(),
            messages: vec![Message {
                role: Role::User,
                content: vec![ContentPart::Text("hi".into())],
                tool_calls: vec![],
                tool_call_id: None,
            }],
            tools: vec![ToolDefinition {
                name: "read_file".into(),
                description: "Read a file".into(),
                schema: serde_json::json!({"type": "object"}),
                namespace: None,
                namespace_description: None,
            }],
            ..Default::default()
        };
        let api_request = build_api_request(request, ChatVendor::Kimi).expect("builds");
        assert_eq!(api_request.tools.len(), 1);
        let tool = &api_request.tools[0];
        // The wire conversion in `ody_api::chat` expects the Responses-API flat
        // shape and rewrites it into the nested Chat Completions shape.
        assert_eq!(tool["type"], "function");
        assert_eq!(tool["name"], "read_file");
        assert_eq!(tool["description"], "Read a file");
        assert!(tool.get("function").is_none());
    }

    #[test]
    fn normalize_function_call() {
        let event = ody_api::ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
            id: None,
            name: "read_file".into(),
            namespace: None,
            arguments: r#"{"path":"/tmp"}"#.into(),
            call_id: "call_1".into(),
            internal_chat_message_metadata_passthrough: None,
        });
        let chat = common::normalize_response_event(event).expect("normalizes");
        assert_eq!(
            chat,
            vec![ChatEvent::ToolCall(ToolCall {
                id: "call_1".into(),
                name: "read_file".into(),
                namespace: None,
                arguments: serde_json::json!({"path": "/tmp"}),
            })]
        );
    }

    /// Regression across the whole `Prompt` -> `ChatRequest` -> wire bridge.
    /// Kimi is a thinking model and conditions on its own prior reasoning: a
    /// probe against `api.kimi.com` recovered a secret planted only in an
    /// inbound `reasoning_content`. Dropping it makes the model re-derive its
    /// plan from tool output every turn.
    ///
    /// This spans two layers on purpose. `adapters/core` silently dropped
    /// reasoning while the wire layer's own tests passed, so the replay was
    /// dead code; a test of either layer alone reproduces that blind spot.
    #[test]
    fn reasoning_survives_the_prompt_to_wire_bridge_for_kimi() {
        let prompt = BridgePrompt {
            input: vec![
                ResponseItem::Reasoning {
                    id: None,
                    summary: Vec::new(),
                    content: Some(vec![ReasoningItemContent::ReasoningText {
                        text: "I should call the tool.".into(),
                    }]),
                    encrypted_content: None,
                    internal_chat_message_metadata_passthrough: None,
                },
                ResponseItem::FunctionCall {
                    id: None,
                    name: "do_it".into(),
                    namespace: None,
                    arguments: "{}".into(),
                    call_id: "call_1".into(),
                    internal_chat_message_metadata_passthrough: None,
                },
            ],
        };
        let request = crate::adapters::core::prompt_to_chat_request("kimi-for-coding", &prompt, None);
        let wire = build_api_request(request, ChatVendor::Kimi)
            .expect("builds")
            .to_wire();

        let messages = wire["messages"].as_array().expect("messages");
        let assistant = messages
            .iter()
            .find(|m| m["role"] == "assistant")
            .expect("assistant message");
        assert_eq!(assistant["reasoning_content"], "I should call the tool.");
        assert!(
            assistant.get("tool_calls").is_some(),
            "reasoning must ride on the assistant message, not a bare one: {assistant}"
        );
    }

    /// The model's private thinking must never be replayed as if it were spoken.
    #[test]
    fn reasoning_is_not_replayed_as_message_content() {
        let prompt = BridgePrompt {
            input: vec![ResponseItem::Reasoning {
                id: None,
                summary: Vec::new(),
                content: Some(vec![ReasoningItemContent::ReasoningText {
                    text: "secret thought".into(),
                }]),
                encrypted_content: None,
                internal_chat_message_metadata_passthrough: None,
            }],
        };
        let request = crate::adapters::core::prompt_to_chat_request("kimi-for-coding", &prompt, None);
        let wire = build_api_request(request, ChatVendor::Kimi)
            .expect("builds")
            .to_wire();

        for message in wire["messages"].as_array().expect("messages") {
            assert!(
                !message["content"].to_string().contains("secret thought"),
                "thinking leaked into content: {message}"
            );
        }
    }
}
