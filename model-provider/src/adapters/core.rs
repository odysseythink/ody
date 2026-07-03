//! Helpers to map the provider-neutral `ChatEvent` stream back into the
//! `ody_api::ResponseEvent` model consumed by `core::client`.
//!
//! This is a temporary compatibility shim used during T2.1.5 to let the core
//! stream path remain on `ResponseEvent` while adapters speak `ChatEvent`.
//! As `core` is refactored to consume `ChatEvent` directly, this module shrinks.

use base64::Engine;

use crate::chat_provider::{
    ChatEvent, ChatRequest, ContentPart, FinishReason, Message, Role, ToolCall, ToolDefinition,
    Usage,
};
use ody_api::ResponseEvent;
use ody_protocol::models::{ContentItem, FunctionCallOutputContentItem, ResponseItem};
use ody_protocol::protocol::TokenUsage;
use ody_tools::{ResponsesApiNamespaceTool, ToolSpec};

/// Convert a single `ChatEvent` back into the `ResponseEvent` model.
///
/// Returns zero or more events because a `Usage` event may be folded into the
/// `Completed` event and a `Finish` event is always required to close the turn.
pub fn to_response_event(event: ChatEvent) -> Vec<ResponseEvent> {
    match event {
        ChatEvent::Start => vec![ResponseEvent::Created],
        ChatEvent::ContentPart(ContentPart::Text(text)) => {
            vec![ResponseEvent::OutputTextDelta(text)]
        }
        ChatEvent::ContentPart(ContentPart::Reasoning(text)) => {
            vec![ResponseEvent::ReasoningContentDelta {
                delta: text,
                content_index: 0,
            }]
        }
        ChatEvent::ContentPart(part) => {
            // For other content parts (image, tool result) emit a message item
            // containing the text representation.
            vec![ResponseEvent::OutputItemDone(ResponseItem::Message {
                id: None,
                role: "assistant".to_string(),
                content: vec![ContentItem::OutputText {
                    text: format!("{:?}", part),
                }],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            })]
        }
        ChatEvent::ToolCall(ToolCall { id, name, arguments }) => {
            vec![ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
                id: None,
                name,
                namespace: None,
                arguments: arguments.to_string(),
                call_id: id,
                internal_chat_message_metadata_passthrough: None,
            })]
        }
        ChatEvent::ReasoningPart(delta) => vec![ResponseEvent::ReasoningContentDelta {
            delta,
            content_index: 0,
        }],
        ChatEvent::Usage(Usage {
            input_tokens,
            output_tokens,
            reasoning_tokens,
        }) => {
            vec![ResponseEvent::Completed {
                response_id: String::new(),
                token_usage: Some(TokenUsage {
                    input_tokens: input_tokens as i64,
                    output_tokens: output_tokens as i64,
                    cached_input_tokens: 0,
                    reasoning_output_tokens: reasoning_tokens.unwrap_or(0) as i64,
                    total_tokens: (input_tokens + output_tokens) as i64,
                }),
                end_turn: Some(true),
            }]
        }
        ChatEvent::Finish {
            reason,
            raw_reason: _,
        } => {
            let end_turn = matches!(reason, FinishReason::Stop);
            vec![ResponseEvent::Completed {
                response_id: String::new(),
                token_usage: None,
                end_turn: Some(end_turn),
            }]
        }
        ChatEvent::Raw(_) => vec![],
        ChatEvent::Error(_err) => {
            // Errors are propagated as transport errors on the stream.
            vec![ResponseEvent::Completed {
                response_id: String::new(),
                token_usage: None,
                end_turn: Some(false),
            }]
        }
    }
}

/// True if the chat event stream has emitted a terminal `Completed` event.
pub fn is_terminal(event: &ResponseEvent) -> bool {
    matches!(event, ResponseEvent::Completed { .. })
}

/// Convert a reasoning-effort config into the normalized chat effort.
fn map_effort(
    effort: Option<&ody_protocol::odysseythink_models::ReasoningEffort>,
) -> crate::chat_provider::ThinkingEffort {
    use ody_protocol::odysseythink_models::ReasoningEffort as Effort;
    match effort {
        None | Some(Effort::None) => crate::chat_provider::ThinkingEffort::Off,
        Some(Effort::Minimal) | Some(Effort::Low) => crate::chat_provider::ThinkingEffort::Low,
        Some(Effort::Medium) => crate::chat_provider::ThinkingEffort::Medium,
        Some(Effort::High) | Some(Effort::XHigh) => crate::chat_provider::ThinkingEffort::High,
        Some(Effort::Ultra) | Some(Effort::Custom(_)) => crate::chat_provider::ThinkingEffort::Max,
    }
}

/// Build a provider-neutral `ChatRequest` from a `core` `Prompt`.
///
/// This helper reuses the existing `ResponseItem` formatting that the core
/// client already builds for `Responses` and `Chat Completions` requests. It
/// then normalizes those items into the `ChatProvider` message model.
///
/// It accepts `core::client_common::Prompt` via a minimal trait so the adapter
/// can live in `model-provider` without creating a dependency cycle on `core`.
pub fn prompt_to_chat_request(
    model: &str,
    prompt: &dyn Prompt,
    effort: Option<ody_protocol::odysseythink_models::ReasoningEffort>,
) -> ChatRequest {
    let formatted = prompt.get_formatted_input_for_request(/*use_responses_lite*/ false);
    let mut messages = Vec::new();
    let base_instructions = prompt.base_instructions();
    if !base_instructions.is_empty() {
        messages.push(Message {
            role: Role::System,
            content: vec![ContentPart::Text(base_instructions)],
            tool_calls: Vec::new(),
            tool_call_id: None,
        });
    }
    for item in formatted {
        messages.extend(response_item_to_messages(item));
    }

    let tools = prompt
        .tools()
        .iter()
        .flat_map(tools_from_spec)
        .collect();

    ChatRequest {
        model: model.to_string(),
        messages,
        tools,
        thinking_effort: map_effort(effort.as_ref()),
        max_tokens: None,
        temperature: None,
        top_p: None,
        stop: Vec::new(),
        output_schema: prompt.output_schema(),
        output_schema_strict: Some(prompt.output_schema_strict()),
        prompt_cache_key: None,
        extra: serde_json::Map::new(),
    }
}

/// A minimal abstraction for the `core` `Prompt` so this adapter can live inside
/// `model-provider` without a cyclic dependency on `core`.
///
/// The real `core::client_common::Prompt` implements this trait; callers in
/// `core` use the `prompt_to_chat_request` helper by passing their concrete
/// `Prompt` type.
pub trait Prompt: Send + Sync {
    fn get_formatted_input_for_request(&self, use_responses_lite: bool) -> Vec<ResponseItem>;
    fn tools(&self) -> &[ToolSpec];
    fn base_instructions(&self) -> String {
        String::new()
    }
    fn output_schema(&self) -> Option<serde_json::Value> {
        None
    }
    fn output_schema_strict(&self) -> bool {
        true
    }
}

/// Convert a `ToolSpec` into one or more provider-neutral `ToolDefinition`s.
///
/// Function tools and namespace tools are expanded into their individual
/// function definitions, preserving description and JSON schema. Responses-only
/// built-ins such as `image_generation` and `web_search` are omitted because
/// they have no Chat Completions equivalent.
fn tools_from_spec(tool: &ToolSpec) -> Vec<ToolDefinition> {
    match tool {
        ToolSpec::Function(tool) => {
            vec![ToolDefinition {
                name: tool.name.clone(),
                description: tool.description.clone(),
                schema: schema_to_value(&tool.parameters),
            }]
        }
        ToolSpec::Namespace(namespace) => namespace
            .tools
            .iter()
            .filter_map(|t| match t {
                ResponsesApiNamespaceTool::Function(tool) => Some(ToolDefinition {
                    name: tool.name.clone(),
                    description: tool.description.clone(),
                    schema: schema_to_value(&tool.parameters),
                }),
            })
            .collect(),
        ToolSpec::ToolSearch {
            description,
            parameters,
            ..
        } => {
            vec![ToolDefinition {
                name: "tool_search".to_string(),
                description: description.clone(),
                schema: schema_to_value(parameters),
            }]
        }
        ToolSpec::Freeform(tool) => {
            vec![ToolDefinition {
                name: tool.name.clone(),
                description: tool.description.clone(),
                schema: serde_json::json!({}),
            }]
        }
        // Responses-only built-in tools have no Chat Completions equivalent;
        // omit them rather than emitting malformed definitions.
        ToolSpec::ImageGeneration { .. } | ToolSpec::WebSearch { .. } => Vec::new(),
    }
}

fn schema_to_value(schema: &ody_tools::JsonSchema) -> serde_json::Value {
    serde_json::to_value(schema).unwrap_or_else(|_| serde_json::json!({}))
}

fn response_item_to_messages(item: ResponseItem) -> Vec<Message> {
    match item {
        ResponseItem::Message { role, content, .. } => {
            let role = match role.as_str() {
                "assistant" => Role::Assistant,
                "user" => Role::User,
                "system" => Role::System,
                "developer" => Role::Developer,
                "tool" => Role::Tool,
                _ => Role::User,
            };
            let mut content_parts = Vec::new();
            for content_item in content {
                if let Some(part) = content_item_to_part(content_item) {
                    content_parts.push(part);
                }
            }
            vec![Message {
                role,
                content: content_parts,
                tool_calls: Vec::new(),
                tool_call_id: None,
            }]
        }
        ResponseItem::FunctionCall {
            name,
            arguments,
            call_id,
            ..
        } => {
            vec![Message {
                role: Role::Assistant,
                content: Vec::new(),
                tool_calls: vec![ToolCall {
                    id: call_id,
                    name,
                    arguments: serde_json::from_str(&arguments).unwrap_or_default(),
                }],
                tool_call_id: None,
            }]
        }
        ResponseItem::FunctionCallOutput { call_id, output, .. }
        | ResponseItem::CustomToolCallOutput { call_id, output, .. } => {
            let mut parts = Vec::new();
            match output.content_items() {
                Some(items) => {
                    for item in items {
                        if let Some(part) = function_call_output_item_to_part(item) {
                            parts.push(part);
                        }
                    }
                }
                // Text-body outputs (exec/shell and most tools store their result
                // as a plain string) make `content_items()` return `None`. The old
                // code dropped them, so every tool result reached the model as an
                // empty `role: tool` message — the model saw all tool output as
                // blank and looped ("file not found" despite a successful read).
                // Preserve the text instead.
                None => {
                    if let Some(text) = output.body.to_text()
                        && !text.is_empty()
                    {
                        parts.push(ContentPart::Text(text));
                    }
                }
            }
            vec![Message {
                role: Role::Tool,
                content: parts,
                tool_calls: Vec::new(),
                tool_call_id: Some(call_id),
            }]
        }
        ResponseItem::Reasoning { .. } => Vec::new(),
        _ => Vec::new(),
    }
}

fn content_item_to_part(item: ContentItem) -> Option<ContentPart> {
    match item {
        ContentItem::InputText { text } => Some(ContentPart::Text(text)),
        ContentItem::OutputText { text } => Some(ContentPart::Text(text)),
        ContentItem::InputImage { image_url, .. } => Some(ContentPart::Image {
            mime: image_url
                .split_once(':')
                .and_then(|(_, rest)| rest.split_once(';').map(|(mime, _)| mime.to_string()))
                .unwrap_or_else(|| "image/png".to_string()),
            bytes: image_url
                .split_once("base64,")
                .and_then(|(_, b64)| base64::engine::general_purpose::STANDARD.decode(b64).ok())
                .unwrap_or_default(),
        }),
    }
}

fn function_call_output_item_to_part(item: &FunctionCallOutputContentItem) -> Option<ContentPart> {
    match item {
        FunctionCallOutputContentItem::InputText { text } => Some(ContentPart::Text(text.clone())),
        FunctionCallOutputContentItem::InputImage { image_url, .. } => Some(ContentPart::Image {
            mime: image_url
                .split_once(':')
                .and_then(|(_, rest)| rest.split_once(';').map(|(mime, _)| mime.to_string()))
                .unwrap_or_else(|| "image/png".to_string()),
            bytes: image_url
                .split_once("base64,")
                .and_then(|(_, b64)| base64::engine::general_purpose::STANDARD.decode(b64).ok())
                .unwrap_or_default(),
        }),
        FunctionCallOutputContentItem::EncryptedContent { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ody_protocol::models::ContentItem;
    use ody_tools::{ResponsesApiNamespace, ResponsesApiTool, ToolSpec};

    struct TestPrompt {
        input: Vec<ResponseItem>,
        tools: Vec<ToolSpec>,
    }

    impl Prompt for TestPrompt {
        fn get_formatted_input_for_request(&self, _use_responses_lite: bool) -> Vec<ResponseItem> {
            self.input.clone()
        }
        fn tools(&self) -> &[ToolSpec] {
            &self.tools
        }
    }

    fn sample_prompt_with_text() -> TestPrompt {
        TestPrompt {
            input: vec![ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText { text: "hello".into() }],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            }],
            tools: Vec::new(),
        }
    }

    #[test]
    fn start_event_maps_to_created() {
        let events = to_response_event(ChatEvent::Start);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], ResponseEvent::Created));
    }

    #[test]
    fn text_delta_maps_to_output_text_delta() {
        let events = to_response_event(ChatEvent::ContentPart(ContentPart::Text("hi".into())));
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], ResponseEvent::OutputTextDelta(ref s) if s == "hi"));
    }

    #[test]
    fn tool_call_maps_to_function_call_item() {
        let events = to_response_event(ChatEvent::ToolCall(ToolCall {
            id: "call_1".into(),
            name: "read".into(),
            arguments: serde_json::json!({"path": "/tmp"}),
        }));
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn usage_event_maps_to_completed_with_usage() {
        let events = to_response_event(ChatEvent::Usage(Usage {
            input_tokens: 10,
            output_tokens: 20,
            reasoning_tokens: None,
        }));
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], ResponseEvent::Completed { .. }));
    }

    #[test]
    fn prompt_to_chat_request_round_trip() {
        let prompt = sample_prompt_with_text();
        let request = prompt_to_chat_request("test-model", &prompt, None);
        assert_eq!(request.model, "test-model");
        assert_eq!(request.messages.len(), 1);
        assert_eq!(request.messages[0].role, Role::User);
        assert_eq!(
            request.messages[0].content,
            vec![ContentPart::Text("hello".into())]
        );
    }

    #[test]
    fn function_call_item_maps_to_tool_call() {
        let prompt = TestPrompt {
            input: vec![ResponseItem::FunctionCall {
                id: None,
                name: "read".into(),
                namespace: None,
                arguments: r#"{"path":"/tmp"}"#.into(),
                call_id: "call_1".into(),
                internal_chat_message_metadata_passthrough: None,
            }],
            tools: Vec::new(),
        };
        let request = prompt_to_chat_request("m", &prompt, None);
        assert_eq!(request.messages.len(), 1);
        assert_eq!(request.messages[0].tool_calls.len(), 1);
        assert_eq!(request.messages[0].tool_calls[0].id, "call_1");
        assert_eq!(request.messages[0].tool_calls[0].name, "read");
    }

    #[test]
    fn function_tool_spec_preserves_description_and_schema() {
        use ody_tools::JsonSchema;

        let schema = JsonSchema::string(Some("the file path".into()));
        let prompt = TestPrompt {
            input: vec![ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText { text: "hi".into() }],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            }],
            tools: vec![ToolSpec::Function(ResponsesApiTool {
                name: "read_file".into(),
                description: "Read a file from disk.".into(),
                strict: false,
                defer_loading: None,
                parameters: schema.clone(),
                output_schema: None,
            })],
        };
        let request = prompt_to_chat_request("m", &prompt, None);
        assert_eq!(request.tools.len(), 1);
        assert_eq!(request.tools[0].name, "read_file");
        assert_eq!(request.tools[0].description, "Read a file from disk.");
        assert_eq!(request.tools[0].schema, serde_json::to_value(&schema).unwrap());
    }

    #[test]
    fn namespace_tool_spec_expands_to_individual_functions() {
        use ody_tools::JsonSchema;

        let prompt = TestPrompt {
            input: vec![ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText { text: "hi".into() }],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            }],
            tools: vec![ToolSpec::Namespace(ResponsesApiNamespace {
                name: "fs".into(),
                description: "Filesystem namespace.".into(),
                tools: vec![
                    ResponsesApiNamespaceTool::Function(ResponsesApiTool {
                        name: "read".into(),
                        description: "Read".into(),
                        strict: false,
                        defer_loading: None,
                        parameters: JsonSchema::string(None),
                        output_schema: None,
                    }),
                    ResponsesApiNamespaceTool::Function(ResponsesApiTool {
                        name: "write".into(),
                        description: "Write".into(),
                        strict: false,
                        defer_loading: None,
                        parameters: JsonSchema::string(None),
                        output_schema: None,
                    }),
                ],
            })],
        };
        let request = prompt_to_chat_request("m", &prompt, None);
        let names: Vec<_> = request.tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["read", "write"]);
    }

    #[test]
    fn responses_only_builtin_tools_are_omitted() {
        let prompt = TestPrompt {
            input: vec![ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText { text: "hi".into() }],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            }],
            tools: vec![
                ToolSpec::ImageGeneration {
                    output_format: "png".into(),
                },
                ToolSpec::WebSearch {
                    external_web_access: None,
                    index_gated_web_access: None,
                    filters: None,
                    user_location: None,
                    search_context_size: None,
                    search_content_types: None,
                },
            ],
        };
        let request = prompt_to_chat_request("m", &prompt, None);
        assert!(request.tools.is_empty());
    }
}
