//! Adapter for the Anthropic Messages wire API.
//!
//! Wraps `ody_api::AnthropicMessagesClient` and normalizes its stream to the
//! provider-neutral `ChatEvent` model. Covers both the direct Anthropic API
//! and AWS Bedrock Claude (SigV4-signed event stream), selected by the
//! `bedrock` flag which mirrors the provider's AWS config presence.

use crate::adapters::common::{self, chat_provider_error_from_api_error};
use crate::chat_provider::{
    ChatProvider, ChatProviderError, ChatRequest, ChatStream, ProviderCapabilities, ProviderId,
};
use futures::StreamExt;
use ody_api::anthropic::AnthropicMessagesRequest;
use ody_api::{
    AnthropicMessagesClient, AnthropicOptions, Compression, Provider as ApiProvider,
    SharedAuthProvider,
};
use ody_client::HttpTransport;
use ody_protocol::models::ResponseItem;

/// Adapter for the Anthropic Messages API.
pub struct AnthropicAdapter<T: HttpTransport> {
    provider_id: ProviderId,
    capabilities: ProviderCapabilities,
    bedrock: bool,
    client: AnthropicMessagesClient<T>,
}

impl<T: HttpTransport> AnthropicAdapter<T> {
    /// Construct from an `ody_api` transport, provider, auth provider, and
    /// Bedrock mode (set when the provider carries AWS SigV4 credentials).
    pub fn new(
        transport: T,
        api_provider: ApiProvider,
        auth: SharedAuthProvider,
        bedrock: bool,
    ) -> Self {
        let client = AnthropicMessagesClient::new(transport, api_provider.clone(), auth, bedrock);
        Self {
            provider_id: "anthropic",
            capabilities: anthropic_capabilities(),
            bedrock,
            client,
        }
    }

    /// Override the default provider id.
    pub fn with_provider_id(mut self, provider_id: ProviderId) -> Self {
        self.provider_id = provider_id;
        self
    }
}

fn anthropic_capabilities() -> ProviderCapabilities {
    ProviderCapabilities {
        supports_streaming: true,
        supports_tools: true,
        supports_thinking: true,
        supports_vision: true,
        supports_multiple_system_messages: true,
        supports_turn_pause: false,
        max_context_tokens: Some(200_000),
        max_output_tokens: Some(8_192),
        thinking_effort: vec![
            crate::chat_provider::ThinkingEffort::Low,
            crate::chat_provider::ThinkingEffort::Medium,
            crate::chat_provider::ThinkingEffort::High,
        ],
    }
}

#[async_trait::async_trait]
impl<T: HttpTransport + 'static> ChatProvider for AnthropicAdapter<T> {
    fn provider_id(&self) -> ProviderId {
        self.provider_id
    }

    fn capabilities(&self) -> ProviderCapabilities {
        self.capabilities.clone()
    }

    async fn chat(&self, request: ChatRequest) -> Result<ChatStream, ChatProviderError> {
        let extra_headers = super::chat::client_metadata_to_headers(request.client_metadata.as_ref());
        let api_request = build_api_request(request, self.bedrock)?;
        let options = AnthropicOptions {
            compression: Compression::None,
            extra_headers,
        };
        let stream = self
            .client
            .stream_request(api_request, options)
            .await
            .map_err(chat_provider_error_from_api_error)?;

        let mut state = common::NormalizeState::default();
        let mapped = stream.map(
            move |result: Result<ody_api::ResponseEvent, _>| -> ChatStream {
                match result
                    .map_err(chat_provider_error_from_api_error)
                    .and_then(|event| {
                        common::normalize_response_event_with_state(event, &mut state)
                    }) {
                    Ok(events) => Box::pin(futures::stream::iter(events.into_iter().map(Ok))),
                    Err(e) => Box::pin(futures::stream::iter(std::iter::once(Err(e)))),
                }
            },
        );
        Ok(Box::pin(mapped.flatten()))
    }
}

fn build_api_request(
    request: ChatRequest,
    bedrock: bool,
) -> Result<AnthropicMessagesRequest, ChatProviderError> {
    use crate::chat_provider::Role;
    let instructions = request
        .messages
        .iter()
        .find(|m| matches!(m.role, Role::System))
        .map(|m| super::chat::content_to_text(&m.content))
        .unwrap_or_default();

    let input: Vec<ResponseItem> = request
        .messages
        .into_iter()
        .filter(|m| !matches!(m.role, Role::System))
        .flat_map(super::chat::message_to_response_items)
        .collect();

    let tools = request
        .tools
        .into_iter()
        .map(super::chat::tool_definition_to_value)
        .collect::<Result<Vec<_>, _>>()?;

    let reasoning_effort =
        super::chat::reasoning_effort_for_request(
            request.thinking_effort,
            &request.supported_thinking_efforts,
        )?;

    Ok(AnthropicMessagesRequest {
        model: request.model,
        input_modalities: request.input_modalities,
        instructions,
        input,
        tools,
        max_completion_tokens: request.max_tokens.map(|v| v as u64),
        temperature: request.temperature.map(|v| v as f64),
        top_p: request.top_p.map(|v| v as f64),
        stop: request.stop,
        reasoning_effort,
        bedrock,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat_provider::{ContentPart, Message, Role};

    fn request() -> ChatRequest {
        ChatRequest {
            model: "claude-sonnet-4-5".into(),
            messages: vec![
                Message {
                    role: Role::System,
                    content: vec![ContentPart::Text("be helpful".into())],
                    tool_calls: vec![],
                    tool_call_id: None,
                },
                Message {
                    role: Role::User,
                    content: vec![ContentPart::Text("hello".into())],
                    tool_calls: vec![],
                    tool_call_id: None,
                },
            ],
            tools: vec![],
            thinking_effort: crate::chat_provider::ThinkingEffort::Off,
            supported_thinking_efforts: vec![],
            max_tokens: Some(1024),
            temperature: Some(0.2),
            top_p: None,
            stop: vec![],
            ..Default::default()
        }
    }

    #[test]
    fn capabilities_cover_streaming_tools_and_thinking() {
        let caps = anthropic_capabilities();
        assert!(caps.supports_streaming);
        assert!(caps.supports_tools);
        assert!(caps.supports_thinking);
        assert_eq!(caps.thinking_effort.len(), 3);
    }

    #[test]
    fn build_request_maps_fields() {
        let api_request = build_api_request(request(), false).expect("builds");
        assert_eq!(api_request.model, "claude-sonnet-4-5");
        assert_eq!(api_request.instructions, "be helpful");
        assert_eq!(api_request.max_completion_tokens, Some(1024));
        // f32 -> f64 widening keeps binary float noise; compare loosely.
        assert!((api_request.temperature.unwrap() - 0.2).abs() < 1e-6);
        assert_eq!(api_request.input.len(), 1, "system message is extracted");
        assert!(api_request.reasoning_effort.is_none());
    }

    #[test]
    fn build_request_maps_supported_thinking_effort() {
        let mut req = request();
        req.thinking_effort = crate::chat_provider::ThinkingEffort::High;
        req.supported_thinking_efforts = vec![
            crate::chat_provider::ThinkingEffort::Medium,
            crate::chat_provider::ThinkingEffort::High,
        ];
        let api_request = build_api_request(req, false).expect("builds");
        assert_eq!(api_request.reasoning_effort.as_deref(), Some("high"));
    }

    #[test]
    fn build_request_strips_unsupported_thinking_effort() {
        let mut req = request();
        req.thinking_effort = crate::chat_provider::ThinkingEffort::High;
        req.supported_thinking_efforts = vec![];
        let api_request = build_api_request(req, false).expect("builds");
        assert!(api_request.reasoning_effort.is_none());
    }
}
