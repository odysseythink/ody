
use anyhow::Error;
use futures::stream::BoxStream;
use http::StatusCode;
use ody_protocol::error::{OdyErr, RetryLimitReachedError, UnexpectedResponseError};
use serde::{Deserialize, Serialize};
use serde_json::Map;
use thiserror::Error;

/// Vendor-neutral identifier for a provider implementation.
pub type ProviderId = &'static str;

/// A provider-neutral chat request.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDefinition>,
    pub thinking_effort: ThinkingEffort,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub stop: Vec<String>,
    /// Optional JSON schema for the model's final output.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<serde_json::Value>,
    /// Whether the output schema should be enforced strictly.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema_strict: Option<bool>,
    /// Optional provider-specific prompt cache key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_cache_key: Option<String>,
    /// Provider-specific escape hatch for parameters not covered above.
    #[serde(default)]
    pub extra: Map<String, serde_json::Value>,
}

/// A non-streaming chat completion.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChatCompletion {
    pub message: Message,
    pub usage: Option<Usage>,
    pub finish_reason: FinishReason,
    pub raw_finish_reason: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentPart>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    #[default]
    System,
    Developer,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ContentPart {
    Text(String),
    Image { mime: String, bytes: Vec<u8> },
    Reasoning(String),
    ToolResult {
        tool_call_id: String,
        content: Vec<ContentPart>,
    },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub schema: serde_json::Value,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Normalized effort for reasoning / extended thinking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingEffort {
    Off,
    Low,
    Medium,
    High,
    #[serde(rename = "xhigh")]
    XHigh,
    Max,
}

impl Default for ThinkingEffort {
    fn default() -> Self {
        ThinkingEffort::Off
    }
}

/// Normalized finish reason across providers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    MaxTokens,
    ToolCalls,
    ContentFilter,
    PauseTurn,
    #[serde(untagged)]
    Other(String),
}

impl Default for FinishReason {
    fn default() -> Self {
        FinishReason::Stop
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RawFrame {
    Json(serde_json::Value),
    Text(String),
}

/// Streaming event returned by `ChatProvider::chat`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ChatEvent {
    Start,
    ContentPart(ContentPart),
    ToolCall(ToolCall),
    ReasoningPart(String),
    Usage(Usage),
    Finish {
        reason: FinishReason,
        #[serde(skip_serializing_if = "Option::is_none")]
        raw_reason: Option<String>,
    },
    Raw(RawFrame),
    Error(ChatProviderError),
}

/// Normalized chat-model capabilities for a provider/model deployment.
///
/// This describes raw model abilities (streaming, tools, vision, thinking,
/// context limits). It is distinct from `provider::ProviderCapabilities`, which
/// describes app-visible feature flags such as `namespace_tools` or
/// `web_search`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    pub supports_streaming: bool,
    pub supports_tools: bool,
    pub supports_thinking: bool,
    pub supports_vision: bool,
    pub supports_multiple_system_messages: bool,
    pub supports_turn_pause: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_context_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub thinking_effort: Vec<ThinkingEffort>,
}

/// Errors that can occur when interacting with a chat provider.
#[derive(Debug, Error, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatProviderError {
    #[error("failed to serialize request: {0}")]
    RequestSerialization(String),
    #[error("transport error: {0}")]
    Transport(String),
    #[error("provider error ({code}): {message}")]
    Provider { code: String, message: String },
    #[error("authentication error: {0}")]
    Auth(String),
    #[error("unsupported capability: {capability}")]
    Unsupported { capability: String },
    #[error("failed to parse stream frame: {0}")]
    StreamParse(String),
}

impl From<Error> for ChatProviderError {
    fn from(err: Error) -> Self {
        // Use alternate formatter to preserve the full error chain/context.
        ChatProviderError::Transport(format!("{:#}", err))
    }
}

impl ChatProviderError {
    /// Convert this provider-neutral error into the user-facing `OdyErr` used
    /// by the rest of the agent. Category codes produced by
    /// `chat_provider_error_from_api_error` are recognized here so that
    /// retries, error events, and guardian rejection messages keep the right
    /// semantics.
    pub fn to_ody_err(&self) -> OdyErr {
        match self {
            ChatProviderError::Provider { code, message } => match code.as_str() {
                "server_overloaded" => OdyErr::ServerOverloaded,
                "invalid_request" => OdyErr::InvalidRequest(message.clone()),
                "internal_server_error" => OdyErr::InternalServerError,
                "context_window_exceeded" => OdyErr::ContextWindowExceeded,
                "quota_exceeded" => OdyErr::QuotaExceeded,
                "usage_not_included" => OdyErr::UsageNotIncluded,
                "cyber_policy" => OdyErr::CyberPolicy {
                    message: message.clone(),
                },
                "rate_limit" => OdyErr::RetryLimit(RetryLimitReachedError {
                    status: StatusCode::TOO_MANY_REQUESTS,
                    request_id: None,
                }),
                "retry_limit" => OdyErr::RetryLimit(RetryLimitReachedError {
                    status: StatusCode::INTERNAL_SERVER_ERROR,
                    request_id: None,
                }),
                "retryable" | "stream" => OdyErr::Stream(message.clone(), None),
                status_code => {
                    if let Ok(code) = status_code.parse::<u16>()
                        && let Ok(status) = StatusCode::from_u16(code)
                    {
                        OdyErr::UnexpectedStatus(UnexpectedResponseError {
                            status,
                            body: message.clone(),
                            user_message: None,
                            url: None,
                            cf_ray: None,
                            request_id: None,
                            identity_authorization_error: None,
                            identity_error_code: None,
                        })
                    } else {
                        OdyErr::Stream(format!("{code}: {message}"), None)
                    }
                }
            },
            ChatProviderError::Auth(message) => {
                OdyErr::UnexpectedStatus(UnexpectedResponseError {
                    status: StatusCode::UNAUTHORIZED,
                    body: message.clone(),
                    user_message: None,
                    url: None,
                    cf_ray: None,
                    request_id: None,
                    identity_authorization_error: None,
                    identity_error_code: None,
                })
            }
            ChatProviderError::Transport(message)
            | ChatProviderError::RequestSerialization(message)
            | ChatProviderError::StreamParse(message) => OdyErr::Stream(message.clone(), None),
            ChatProviderError::Unsupported { capability } => {
                OdyErr::UnsupportedOperation(capability.clone())
            }
        }
    }
}

/// A trait implemented by every model provider adapter.
///
/// This is the central abstraction that makes runtime provider switching
/// cheap: add a new implementation and register it in the adapter catalog.
#[async_trait::async_trait]
pub trait ChatProvider: Send + Sync {
    /// Unique provider identifier, e.g. "openai-responses" or "anthropic".
    fn provider_id(&self) -> ProviderId;

    /// Capabilities for this concrete model deployment.
    fn capabilities(&self) -> ProviderCapabilities;

    /// Start a chat turn and return a stream of normalized events.
    async fn chat(&self, request: ChatRequest) -> Result<ChatStream, ChatProviderError>;

    /// Non-streaming completion. Adapters that only support streaming may
    /// collect the stream and return a single completion.
    async fn chat_complete(
        &self,
        request: ChatRequest,
    ) -> Result<ChatCompletion, ChatProviderError> {
        use futures::StreamExt;
        let mut stream = self.chat(request).await?;
        let mut content_parts = Vec::new();
        let mut tool_calls = Vec::new();
        let mut usage = None;
        let mut finish = FinishReason::Stop;
        let mut raw_finish = None;

        while let Some(result) = stream.next().await {
            match result? {
                ChatEvent::Start => {}
                ChatEvent::ContentPart(part) => content_parts.push(part),
                ChatEvent::ToolCall(tc) => tool_calls.push(tc),
                ChatEvent::ReasoningPart(r) => content_parts.push(ContentPart::Reasoning(r)),
                ChatEvent::Usage(u) => usage = Some(u),
                ChatEvent::Finish {
                    reason,
                    raw_reason,
                } => {
                    finish = reason;
                    raw_finish = raw_reason;
                }
                ChatEvent::Raw(_) => {}
                ChatEvent::Error(e) => return Err(e),
            }
        }

        Ok(ChatCompletion {
            message: Message {
                role: Role::Assistant,
                content: content_parts,
                tool_calls,
                tool_call_id: None,
            },
            usage,
            finish_reason: finish,
            raw_finish_reason: raw_finish,
        })
    }
}

pub type ChatStream = BoxStream<'static, Result<ChatEvent, ChatProviderError>>;

impl ChatEvent {
    /// Returns true if this event is a streaming output delta (text or
    /// reasoning) that needs a preceding `OutputItemAdded` frame when mapped
    /// back to the Responses event model.
    pub fn produces_output_delta(&self) -> bool {
        matches!(
            self,
            ChatEvent::ContentPart(ContentPart::Text(_))
                | ChatEvent::ContentPart(ContentPart::Reasoning(_))
                | ChatEvent::ReasoningPart(_)
        )
    }
}

/// A helper to map a generic error to a transport error.
pub fn transport_error<E: std::error::Error + Send + Sync + 'static>(err: E) -> ChatProviderError {
    ChatProviderError::Transport(err.to_string())
}

/// Clamp a thinking effort to the nearest supported level for a provider.
pub fn clamp_thinking_effort(
    desired: ThinkingEffort,
    supported: &[ThinkingEffort],
) -> Option<ThinkingEffort> {
    if desired == ThinkingEffort::Off || supported.is_empty() {
        return Some(ThinkingEffort::Off);
    }
    let order = [
        ThinkingEffort::Off,
        ThinkingEffort::Low,
        ThinkingEffort::Medium,
        ThinkingEffort::High,
        ThinkingEffort::XHigh,
        ThinkingEffort::Max,
    ];
    let desired_index = order.iter().position(|e| *e == desired)?;
    order
        .iter()
        .take(desired_index + 1)
        .rev()
        .find(|e| supported.contains(e))
        .copied()
        .or(Some(ThinkingEffort::Off))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_role_is_system() {
        assert_eq!(Role::default(), Role::System);
    }

    #[test]
    fn default_thinking_effort_is_off() {
        assert_eq!(ThinkingEffort::default(), ThinkingEffort::Off);
    }

    #[test]
    fn clamping_low_to_nearest_supported_at_or_below() {
        // Desired Low is not supported, and there is no supported level below Low (other than Off).
        let supported = vec![ThinkingEffort::Medium, ThinkingEffort::High];
        assert_eq!(
            clamp_thinking_effort(ThinkingEffort::Low, &supported),
            Some(ThinkingEffort::Off)
        );
    }

    #[test]
    fn clamping_returns_off_for_empty_supported() {
        assert_eq!(
            clamp_thinking_effort(ThinkingEffort::High, &[]),
            Some(ThinkingEffort::Off)
        );
    }

    #[test]
    fn clamping_unsupported_returns_highest_lower() {
        let supported = vec![ThinkingEffort::Low, ThinkingEffort::Max];
        assert_eq!(
            clamp_thinking_effort(ThinkingEffort::XHigh, &supported),
            Some(ThinkingEffort::Low)
        );
    }

    #[test]
    fn chat_provider_error_roundtrip() {
        let err = ChatProviderError::Provider {
            code: "rate_limit".into(),
            message: "too many requests".into(),
        };
        let json = serde_json::to_string(&err).expect("serializes");
        let back: ChatProviderError = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(err, back);
    }

    #[test]
    fn provider_capabilities_default_false() {
        let caps = ProviderCapabilities::default();
        assert!(!caps.supports_streaming);
        assert!(!caps.supports_tools);
        assert!(!caps.supports_thinking);
    }
}
