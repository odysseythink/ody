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

fn build_api_request(request: ChatRequest, vendor: ChatVendor) -> Result<ChatCompletionsRequest, ChatProviderError> {
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
        .map(message_to_response_item)
        .collect();

    let tools = request
        .tools
        .into_iter()
        .map(tool_definition_to_value)
        .collect::<Result<Vec<_>, _>>()?;

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
    let effort = clamp_thinking_effort(thinking_effort, &supported)
        .ok_or_else(|| ChatProviderError::Unsupported {
            capability: "thinking effort".into(),
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

fn message_to_response_item(message: crate::chat_provider::Message) -> ResponseItem {
    use crate::chat_provider::{ContentPart, Role};
    let role = match message.role {
        Role::User => "user",
        Role::Developer => "system",
        Role::Assistant => "assistant",
        Role::Tool => "user",
        Role::System => "system",
    }
    .to_string();

    let mut content = Vec::new();
    for part in message.content {
        match part {
            ContentPart::Text(text) => content.push(ody_protocol::models::ContentItem::InputText {
                text,
            }),
            ContentPart::Image { mime, bytes } => {
                let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                content.push(ody_protocol::models::ContentItem::InputImage {
                    image_url: format!("data:{mime};base64,{b64}"),
                    detail: None,
                });
            }
            ContentPart::Reasoning(text) => content.push(ody_protocol::models::ContentItem::InputText {
                text,
            }),
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

    ResponseItem::Message {
        id: None,
        role,
        content,
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

fn tool_definition_to_value(
    def: crate::chat_provider::ToolDefinition,
) -> Result<serde_json::Value, ChatProviderError> {
    Ok(serde_json::json!({
        "type": "function",
        "function": {
            "name": def.name,
            "description": def.description,
            "parameters": def.schema,
            "strict": true,
        }
    }))
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

        let mapped =
            stream.map(
                |result: Result<ody_api::ResponseEvent, _>| -> ChatStream {
                    match result
                        .map_err(chat_provider_error_from_api_error)
                        .and_then(common::normalize_response_event)
                    {
                        Ok(events) => Box::pin(futures::stream::iter(
                            events.into_iter().map(Ok),
                        )),
                        Err(e) => Box::pin(futures::stream::iter(std::iter::once(Err(e)))),
                    }
                },
            );
        Ok(Box::pin(mapped.flatten()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat_provider::{ChatEvent, ContentPart, Message, Role, ToolCall};

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
            messages: vec![
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
        assert_eq!(api_request.reasoning_effort, None);
    }

    #[test]
    fn build_request_maps_deepseek_thinking() {
        let request = ChatRequest {
            model: "deepseek-chat".into(),
            thinking_effort: ThinkingEffort::High,
            messages: vec![
                Message {
                    role: Role::User,
                    content: vec![ContentPart::Text("hi".into())],
                    tool_calls: vec![],
                    tool_call_id: None,
                },
            ],
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
            messages: vec![
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
        assert!((api_request.temperature.unwrap() - 0.7).abs() < 1e-6);
        assert!((api_request.top_p.unwrap() - 0.9).abs() < 1e-6);
        assert_eq!(api_request.stop, vec!["<|end|>"]);
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
                arguments: serde_json::json!({"path": "/tmp"}),
            })]
        );
    }
}
