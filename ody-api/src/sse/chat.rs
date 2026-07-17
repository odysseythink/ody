//! Streaming parser for the Chat Completions wire protocol.
//!
//! Translates `chat.completion.chunk` SSE deltas into the internal
//! [`ResponseEvent`] model so the Chat path is interchangeable with the
//! Responses path from `core`'s point of view.

use crate::chat::ChatVendor;
use crate::common::ResponseEvent;
use crate::common::ResponseStream;
use crate::error::ApiError;
use crate::telemetry::SseTelemetry;
use ody_client::ByteStream;
use ody_client::StreamResponse;
use ody_protocol::models::ContentItem;
use ody_protocol::models::ReasoningItemContent;
use ody_protocol::models::ResponseItem;
use ody_protocol::protocol::TokenUsage;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio::time::timeout;
use tracing::debug;
use tracing::trace;

const REQUEST_ID_HEADER: &str = "x-request-id";

pub fn spawn_chat_stream(
    stream_response: StreamResponse,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
    vendor: ChatVendor,
) -> ResponseStream {
    let upstream_request_id = stream_response
        .headers
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    let (tx_event, rx_event) = mpsc::channel::<Result<ResponseEvent, ApiError>>(1600);
    tokio::spawn(async move {
        process_chat_sse(stream_response.bytes, tx_event, idle_timeout, telemetry, vendor).await;
    });

    ResponseStream {
        rx_event,
        upstream_request_id,
    }
}

#[derive(Debug, Deserialize)]
struct ChatChunk {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    choices: Vec<ChatChoice>,
    #[serde(default)]
    usage: Option<ChatUsage>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    #[serde(default)]
    delta: ChatDelta,
    #[serde(default)]
    finish_reason: Option<String>,
    // Kimi sometimes attaches usage to the choice rather than the top level.
    #[serde(default)]
    usage: Option<ChatUsage>,
}

#[derive(Debug, Default, Deserialize)]
struct ChatDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ChatToolCallDelta>>,
}

#[derive(Debug, Deserialize)]
struct ChatToolCallDelta {
    #[serde(default)]
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<ChatFunctionDelta>,
}

#[derive(Debug, Deserialize)]
struct ChatFunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ChatUsage {
    #[serde(default)]
    prompt_tokens: i64,
    #[serde(default)]
    completion_tokens: i64,
    #[serde(default)]
    total_tokens: i64,
    // Moonshot reports cache reads here rather than under prompt_tokens_details.
    #[serde(default)]
    cached_tokens: Option<i64>,
    #[serde(default)]
    prompt_tokens_details: Option<ChatPromptTokensDetails>,
    #[serde(default)]
    completion_tokens_details: Option<ChatCompletionTokensDetails>,
}

#[derive(Debug, Default, Deserialize)]
struct ChatPromptTokensDetails {
    #[serde(default)]
    cached_tokens: i64,
}

#[derive(Debug, Default, Deserialize)]
struct ChatCompletionTokensDetails {
    #[serde(default)]
    reasoning_tokens: i64,
}

impl From<ChatUsage> for TokenUsage {
    fn from(usage: ChatUsage) -> Self {
        TokenUsage {
            input_tokens: usage.prompt_tokens,
            cached_input_tokens: match usage.cached_tokens {
                Some(cached) => cached,
                None => usage
                    .prompt_tokens_details
                    .map(|d| d.cached_tokens)
                    .unwrap_or(0),
            },
            output_tokens: usage.completion_tokens,
            reasoning_output_tokens: usage
                .completion_tokens_details
                .map(|d| d.reasoning_tokens)
                .unwrap_or(0),
            total_tokens: usage.total_tokens,
        }
    }
}

#[derive(Debug, Default)]
struct ToolCallAccumulator {
    id: Option<String>,
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct ChatErrorEnvelope {
    error: ChatError,
}

#[derive(Debug, Deserialize)]
struct ChatError {
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    code: Option<String>,
}

async fn process_chat_sse(
    stream: ByteStream,
    tx_event: mpsc::Sender<Result<ResponseEvent, ApiError>>,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
    vendor: ChatVendor,
) {
    let mut stream = stream.eventsource();
    let mut response_id: Option<String> = None;
    let mut usage: Option<ChatUsage> = None;
    let mut finish_reason: Option<String> = None;
    let mut assistant_text = String::new();
    let mut reasoning_text = String::new();
    let mut tool_calls: BTreeMap<usize, ToolCallAccumulator> = BTreeMap::new();
    let mut content_item_started = false;
    let mut reasoning_item_started = false;

    if tx_event.send(Ok(ResponseEvent::Created)).await.is_err() {
        return;
    }

    loop {
        let start = Instant::now();
        let response = timeout(idle_timeout, stream.next()).await;
        if let Some(t) = telemetry.as_ref() {
            t.on_sse_poll(&response, start.elapsed());
        }
        let sse = match response {
            Ok(Some(Ok(sse))) => sse,
            Ok(Some(Err(e))) => {
                debug!("chat SSE error: {e:#}");
                let _ = tx_event.send(Err(ApiError::Stream(e.to_string()))).await;
                return;
            }
            Ok(None) => break,
            Err(_) => {
                let _ = tx_event
                    .send(Err(ApiError::Stream("idle timeout waiting for SSE".into())))
                    .await;
                return;
            }
        };

        trace!("chat SSE event: {}", &sse.data);
        if sse.data.trim() == "[DONE]" {
            break;
        }

        let value: Value = match serde_json::from_str(&sse.data) {
            Ok(value) => value,
            Err(parse_err) => {
                debug!("failed to parse chat SSE data: {parse_err}, data: {}", &sse.data);
                continue;
            }
        };
        // The server may send a JSON error object instead of a chunk. `ChatChunk`
        // has all-optional fields, so an error object would otherwise parse as an
        // empty chunk; check for `error` explicitly first.
        if value.get("error").is_some() {
            if let Ok(envelope) = serde_json::from_value::<ChatErrorEnvelope>(value) {
                let _ = tx_event.send(Err(chat_error(envelope.error))).await;
                return;
            }
            continue;
        }
        let chunk: ChatChunk = match serde_json::from_value(value) {
            Ok(chunk) => chunk,
            Err(parse_err) => {
                debug!("failed to parse chat SSE chunk: {parse_err}, data: {}", &sse.data);
                continue;
            }
        };

        if let Some(id) = chunk.id
            && !id.is_empty()
        {
            response_id = Some(id);
        }
        if let Some(chunk_usage) = chunk.usage {
            usage = Some(chunk_usage);
        }

        for choice in chunk.choices {
            if let Some(choice_usage) = choice.usage {
                usage = Some(choice_usage);
            }
            if let Some(reason) = choice.finish_reason {
                finish_reason = Some(reason);
            }

            let delta = choice.delta;
            if let Some(content) = delta.content
                && !content.is_empty()
            {
                if !content_item_started {
                    content_item_started = true;
                    if tx_event
                        .send(Ok(ResponseEvent::OutputItemAdded(ResponseItem::Message {
                            id: response_id.clone(),
                            role: "assistant".to_string(),
                            content: vec![],
                            phase: None,
                            internal_chat_message_metadata_passthrough: None,
                        })))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                assistant_text.push_str(&content);
                if tx_event
                    .send(Ok(ResponseEvent::OutputTextDelta(content)))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            if vendor.supports_reasoning_content()
                && let Some(reasoning) = delta.reasoning_content
                && !reasoning.is_empty()
            {
                if !reasoning_item_started {
                    reasoning_item_started = true;
                    if tx_event
                        .send(Ok(ResponseEvent::OutputItemAdded(ResponseItem::Reasoning {
                            id: None,
                            summary: vec![],
                            content: None,
                            encrypted_content: None,
                            internal_chat_message_metadata_passthrough: None,
                        })))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                reasoning_text.push_str(&reasoning);
                if tx_event
                    .send(Ok(ResponseEvent::ReasoningContentDelta {
                        delta: reasoning,
                        content_index: 0,
                    }))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            if let Some(call_deltas) = delta.tool_calls {
                for call in call_deltas {
                    let entry = tool_calls.entry(call.index).or_default();
                    if let Some(id) = call.id
                        && !id.is_empty()
                    {
                        entry.id = Some(id);
                    }
                    if let Some(function) = call.function {
                        if let Some(name) = function.name
                            && !name.is_empty()
                        {
                            entry.name = name;
                        }
                        if let Some(arguments) = function.arguments
                            && !arguments.is_empty()
                        {
                            entry.arguments.push_str(&arguments);
                            let item_id = entry
                                .id
                                .clone()
                                .unwrap_or_else(|| format!("call_{}", call.index));
                            if tx_event
                                .send(Ok(ResponseEvent::ToolCallInputDelta {
                                    item_id,
                                    call_id: entry.id.clone(),
                                    delta: arguments,
                                }))
                                .await
                                .is_err()
                            {
                                return;
                            }
                        }
                    }
                }
            }
        }
    }

    // Emit accumulated output items, then the terminal Completed event.
    let produced_text = !assistant_text.is_empty();
    let produced_reasoning = !reasoning_text.is_empty();
    if !reasoning_text.is_empty() {
        let item = ResponseItem::Reasoning {
            id: None,
            summary: Vec::new(),
            content: Some(vec![ReasoningItemContent::ReasoningText {
                text: reasoning_text,
            }]),
            encrypted_content: None,
            internal_chat_message_metadata_passthrough: None,
        };
        if tx_event
            .send(Ok(ResponseEvent::OutputItemDone(item)))
            .await
            .is_err()
        {
            return;
        }
    }

    if !assistant_text.is_empty() {
        let item = ResponseItem::Message {
            id: None,
            role: "assistant".to_string(),
            content: vec![ContentItem::OutputText {
                text: assistant_text,
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        };
        if tx_event
            .send(Ok(ResponseEvent::OutputItemDone(item)))
            .await
            .is_err()
        {
            return;
        }
    }

    let has_tool_calls = !tool_calls.is_empty();
    for (index, call) in tool_calls {
        let call_id = call
            .id
            .unwrap_or_else(|| format!("call_{index}"));
        let item = ResponseItem::FunctionCall {
            id: None,
            name: call.name,
            namespace: None,
            arguments: call.arguments,
            call_id,
            internal_chat_message_metadata_passthrough: None,
        };
        if tx_event
            .send(Ok(ResponseEvent::OutputItemDone(item)))
            .await
            .is_err()
        {
            return;
        }
    }

    // Flag degenerate completions: the stream closed without any content,
    // reasoning, or tool call. This is the shape that silently ends a turn
    // (e.g. Kimi returning a near-empty completion), so surface it for
    // diagnosis without needing full SSE tracing.
    if !produced_text && !produced_reasoning && !has_tool_calls {
        tracing::warn!(
            target: "ody_api::sse::chat",
            vendor = ?vendor,
            finish_reason = ?finish_reason,
            response_id = ?response_id,
            "chat stream completed with no output items (empty completion): model \
             returned neither content, reasoning, nor tool calls"
        );
    }

    // A turn ends unless the model requested tool calls (in which case the
    // agent must run them and continue). Some providers omit finish_reason.
    let end_turn = match finish_reason.as_deref() {
        Some("tool_calls") => Some(false),
        Some(_) => Some(true),
        None => Some(!has_tool_calls),
    };

    let _ = tx_event
        .send(Ok(ResponseEvent::Completed {
            response_id: response_id.unwrap_or_default(),
            token_usage: usage.map(Into::into),
            end_turn,
            finish_reason: finish_reason.clone(),
        }))
        .await;
}

fn chat_error(error: ChatError) -> ApiError {
    let message = error.message.unwrap_or_else(|| "chat completion error".into());
    match error.code.as_deref() {
        Some("context_length_exceeded") => ApiError::ContextWindowExceeded,
        Some("insufficient_quota") => ApiError::QuotaExceeded,
        _ => ApiError::Stream(message),
    }
}

#[cfg(test)]
#[path = "chat_sse_tests.rs"]
mod chat_sse_tests;
