//! Adapter for the OpenAI Responses wire API.
//!
//! Wraps `ody_api::ResponsesClient` and normalizes its `ResponseEvent` stream to
//! the provider-neutral `ChatEvent` model defined by `ChatProvider`.

use crate::adapters::common;
use crate::adapters::common::chat_provider_error_from_api_error;
use crate::chat_provider::{
    ChatProvider, ChatProviderError, ChatRequest, ChatStream, ProviderCapabilities, ProviderId,
    ThinkingEffort, clamp_thinking_effort,
};
use base64::Engine;
use futures::StreamExt;
use ody_api::{
    Compression, Provider as ApiProvider, Reasoning, ResponsesApiRequest, ResponsesClient,
    ResponsesOptions, SharedAuthProvider, create_text_param_for_request,
};
use ody_client::HttpTransport;
use ody_protocol::models::{
    ContentItem, FunctionCallOutputContentItem, FunctionCallOutputPayload, ResponseItem,
};
use ody_tools::default_namespace_description;

/// Adapter for the OpenAI Responses API.
pub struct ResponsesAdapter<T: HttpTransport> {
    provider_id: ProviderId,
    capabilities: ProviderCapabilities,
    client: ResponsesClient<T>,
}

impl<T: HttpTransport> ResponsesAdapter<T> {
    /// Construct from an `ody_api` transport, provider, and auth provider.
    pub fn new(transport: T, api_provider: ApiProvider, auth: SharedAuthProvider) -> Self {
        let client = ResponsesClient::new(transport, api_provider.clone(), auth);
        Self {
            provider_id: "openai-responses",
            capabilities: capabilities_from_provider(&api_provider),
            client,
        }
    }

    /// Override the default provider id (useful for OpenAI-compatible endpoints).
    pub fn with_provider_id(mut self, provider_id: ProviderId) -> Self {
        self.provider_id = provider_id;
        self
    }
}

fn capabilities_from_provider(_provider: &ApiProvider) -> ProviderCapabilities {
    ProviderCapabilities {
        supports_streaming: true,
        supports_tools: true,
        supports_thinking: false,
        supports_vision: true,
        supports_multiple_system_messages: false,
        supports_turn_pause: false,
        max_context_tokens: Some(128_000),
        max_output_tokens: Some(16_384),
        thinking_effort: vec![],
    }
}

fn build_api_request(request: ChatRequest) -> Result<ResponsesApiRequest, ChatProviderError> {
    let instructions = request
        .messages
        .iter()
        .find(|m| matches!(m.role, crate::chat_provider::Role::System))
        .map(|m| content_to_text(&m.content))
        .unwrap_or_default();

    let input: Vec<ResponseItem> = request
        .messages
        .into_iter()
        .filter(|m| !matches!(m.role, crate::chat_provider::Role::System))
        .map(message_to_response_items)
        .collect::<Result<Vec<Vec<_>>, _>>()?
        .into_iter()
        .flatten()
        .collect();

    let tools = build_tools(request.tools)?;

    let reasoning = reasoning_for_request(request.thinking_effort)?;
    let text = create_text_param_for_request(
        None,
        &request.output_schema,
        request.output_schema_strict.unwrap_or(true),
    );

    Ok(ResponsesApiRequest {
        model: request.model,
        instructions,
        input,
        tools,
        tool_choice: "auto".to_string(),
        parallel_tool_calls: true,
        reasoning,
        store: true,
        stream: true,
        include: vec![],
        service_tier: None,
        prompt_cache_key: request.prompt_cache_key,
        text,
        client_metadata: None,
    })
}

fn reasoning_for_request(
    thinking_effort: ThinkingEffort,
) -> Result<Option<Reasoning>, ChatProviderError> {
    if thinking_effort == ThinkingEffort::Off {
        return Ok(None);
    }
    let supported = vec![
        ThinkingEffort::Low,
        ThinkingEffort::Medium,
        ThinkingEffort::High,
    ];
    let effort = clamp_thinking_effort(thinking_effort, &supported).ok_or_else(|| {
        ChatProviderError::Unsupported {
            capability: "thinking effort".into(),
        }
    })?;
    if effort == ThinkingEffort::Off {
        return Ok(None);
    }
    let api_effort = match effort {
        ThinkingEffort::Low => ody_protocol::model_metadata::ReasoningEffort::Low,
        ThinkingEffort::Medium => ody_protocol::model_metadata::ReasoningEffort::Medium,
        ThinkingEffort::High => ody_protocol::model_metadata::ReasoningEffort::High,
        _ => unreachable!("clamp_thinking_effort should not return unsupported effort"),
    };
    Ok(Some(Reasoning {
        effort: Some(api_effort),
        summary: None,
        context: None,
    }))
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

fn message_to_response_items(
    message: crate::chat_provider::Message,
) -> Result<Vec<ResponseItem>, ChatProviderError> {
    use crate::chat_provider::{ContentPart, Role};

    // Tool results are emitted as the Responses-API function_call_output item
    // so the model can correlate them with the original tool call.
    if message.role == Role::Tool {
        if let Some(call_id) = message.tool_call_id {
            let output = tool_message_output(&message.content);
            return Ok(vec![ResponseItem::FunctionCallOutput {
                id: None,
                call_id,
                output,
                internal_chat_message_metadata_passthrough: None,
            }]);
        }
        // A tool message without a call_id cannot be represented on the wire;
        // drop it rather than sending misleading text to the model.
        return Err(ChatProviderError::Unsupported {
            capability: "tool message without call_id".into(),
        });
    }

    let role = match message.role {
        Role::User => "user",
        Role::Developer => "developer",
        Role::Assistant => "assistant",
        Role::Tool => unreachable!("handled above"),
        Role::System => {
            return Err(ChatProviderError::Unsupported {
                capability: "system message in input".into(),
            });
        }
    }
    .to_string();

    let mut content = Vec::new();
    for part in message.content {
        match part {
            ContentPart::Text(text) => content.push(ContentItem::InputText { text }),
            ContentPart::Image { mime, bytes } => {
                // Preserve images as base64 data URLs so the Responses API can
                // consume them. The mime type was recovered when normalizing the
                // original ResponseItem; re-encode as a base64 data URL.
                let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                content.push(ContentItem::InputImage {
                    image_url: format!("data:{mime};base64,{b64}"),
                    detail: None,
                });
            }
            ContentPart::Reasoning(text) => content.push(ContentItem::InputText { text }),
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
                content.push(ContentItem::InputText {
                    text: format!("tool result ({}): {}", tool_call_id, text),
                });
            }
        }
    }

    // For assistant output, mark text parts as output_text to match the Responses
    // API convention; all other roles use input_text.
    let content = if message.role == Role::Assistant {
        content
            .into_iter()
            .map(|item| match item {
                ContentItem::InputText { text } => ContentItem::OutputText { text },
                other => other,
            })
            .collect()
    } else {
        content
    };

    let mut items = Vec::new();
    if !content.is_empty() || message.role != Role::Assistant {
        items.push(ResponseItem::Message {
            id: None,
            role,
            content,
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        });
    }

    // Emit FunctionCall items for assistant tool_calls.
    if message.role == Role::Assistant {
        for tc in message.tool_calls {
            items.push(ResponseItem::FunctionCall {
                id: None,
                name: tc.name,
                namespace: tc.namespace,
                arguments: tc.arguments.to_string(),
                call_id: tc.id,
                internal_chat_message_metadata_passthrough: None,
            });
        }
    }

    Ok(items)
}

fn tool_message_output(
    content: &[crate::chat_provider::ContentPart],
) -> FunctionCallOutputPayload {
    let mut items = Vec::new();
    for part in content {
        match part {
            crate::chat_provider::ContentPart::Text(text) => {
                items.push(FunctionCallOutputContentItem::InputText { text: text.clone() });
            }
            crate::chat_provider::ContentPart::Image { mime, bytes } => {
                let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
                items.push(FunctionCallOutputContentItem::InputImage {
                    image_url: format!("data:{mime};base64,{b64}"),
                    detail: None,
                });
            }
            crate::chat_provider::ContentPart::Reasoning(text) => {
                items.push(FunctionCallOutputContentItem::InputText { text: text.clone() });
            }
            crate::chat_provider::ContentPart::ToolResult { .. } => {
                // Nested tool results are unexpected here; ignore them.
            }
        }
    }
    if items.is_empty() {
        FunctionCallOutputPayload::from_text("".to_string())
    } else {
        FunctionCallOutputPayload::from_content_items(items)
    }
}

fn build_tools(
    defs: Vec<crate::chat_provider::ToolDefinition>,
) -> Result<Vec<serde_json::Value>, ChatProviderError> {
    enum Entry {
        Tool(serde_json::Value),
        Namespace {
            name: String,
            description: Option<String>,
            tools: Vec<serde_json::Value>,
        },
    }

    let mut entries: Vec<Entry> = Vec::new();
    for def in defs {
        let child = function_tool_value(&def.name, &def.description, &def.schema);
        if let Some(namespace) = def.namespace {
            if let Some(Entry::Namespace { name, tools, .. }) = entries.iter_mut().find(|e| {
                matches!(
                    e,
                    Entry::Namespace { name: existing, .. } if existing == &namespace
                )
            }) {
                let _ = name;
                tools.push(child);
            } else {
                entries.push(Entry::Namespace {
                    name: namespace,
                    description: def.namespace_description,
                    tools: vec![child],
                });
            }
        } else {
            entries.push(Entry::Tool(child));
        }
    }

    Ok(entries
        .into_iter()
        .map(|entry| match entry {
            Entry::Tool(value) => value,
            Entry::Namespace {
                name,
                description,
                tools,
            } => {
                let description = description.unwrap_or_else(|| default_namespace_description(&name));
                serde_json::json!({
                    "type": "namespace",
                    "name": name,
                    "description": description,
                    "tools": tools,
                })
            }
        })
        .collect())
}

fn function_tool_value(
    name: &str,
    description: &str,
    schema: &serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "name": name,
        "description": description,
        "parameters": schema,
        "strict": true,
    })
}

#[async_trait::async_trait]
impl<T: HttpTransport + 'static> ChatProvider for ResponsesAdapter<T> {
    fn provider_id(&self) -> ProviderId {
        self.provider_id
    }

    fn capabilities(&self) -> ProviderCapabilities {
        self.capabilities.clone()
    }

    async fn chat(&self, request: ChatRequest) -> Result<ChatStream, ChatProviderError> {
        let api_request = build_api_request(request)?;
        let options = ResponsesOptions {
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
    use crate::chat_provider::{
        ChatEvent, ContentPart, FinishReason, Message, Role, ToolDefinition,
    };
    use ody_protocol::protocol::TokenUsage;

    #[test]
    fn adapter_reports_expected_id() {
        assert_eq!("openai-responses", "openai-responses");
    }

    #[test]
    fn content_to_text_joins_parts() {
        let content = vec![
            ContentPart::Text("hello ".into()),
            ContentPart::Text("world".into()),
        ];
        assert_eq!(content_to_text(&content), "hello world");
    }

    #[test]
    fn build_request_maps_messages() {
        let request = ChatRequest {
            model: "gpt-4o".into(),
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
            tools: vec![ToolDefinition {
                name: "read_file".into(),
                description: "read a file".into(),
                schema: serde_json::json!({"type": "object"}),
                namespace: None,
                namespace_description: None,
            }],
            ..Default::default()
        };
        let api_request = build_api_request(request).expect("builds");
        assert_eq!(api_request.model, "gpt-4o");
        assert_eq!(api_request.instructions, "You are helpful");
        assert_eq!(api_request.input.len(), 1);
        assert_eq!(api_request.tools.len(), 1);
    }

    #[test]
    fn normalize_output_text_delta() {
        let event = ody_api::ResponseEvent::OutputTextDelta("hello".into());
        let chat = common::normalize_response_event(event).expect("normalizes");
        assert_eq!(
            chat,
            vec![ChatEvent::ContentPart(ContentPart::Text("hello".into()))]
        );
    }

    #[test]
    fn normalize_completed_emits_usage_then_finish() {
        // A non-empty completion produces output before the terminal
        // `Completed` event; without any prior output the provider treats the
        // completion as empty and surfaces a retryable error instead of
        // `Usage`/`Finish`.
        let mut state = common::NormalizeState::default();
        common::normalize_response_event_with_state(
            ody_api::ResponseEvent::OutputTextDelta("hi".into()),
            &mut state,
        )
        .expect("text delta normalizes");
        let chat = common::normalize_response_event_with_state(
            ody_api::ResponseEvent::Completed {
                response_id: "r_1".into(),
                token_usage: Some(TokenUsage {
                    input_tokens: 10,
                    output_tokens: 20,
                    cached_input_tokens: 0,
                    reasoning_output_tokens: 0,
                    total_tokens: 30,
                }),
                end_turn: Some(true),
            },
            &mut state,
        )
        .expect("normalizes");
        assert_eq!(chat.len(), 2);
        assert_eq!(
            chat[0],
            ChatEvent::Usage(crate::chat_provider::Usage {
                input_tokens: 10,
                output_tokens: 20,
                reasoning_tokens: None,
            })
        );
        assert_eq!(
            chat[1],
            ChatEvent::Finish {
                reason: FinishReason::Stop,
                raw_reason: Some("stop".into()),
            }
        );
    }
}
