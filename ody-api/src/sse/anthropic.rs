//! Streaming parser for the Anthropic Messages wire protocol.
//!
//! Translates Anthropic streaming events (direct SSE, or AWS event stream
//! frames on Bedrock) into the internal [`ResponseEvent`] model so the
//! Anthropic path is interchangeable with the Chat/Responses paths from
//! `core`'s point of view.

use crate::common::ResponseEvent;
use crate::common::ResponseStream;
use crate::error::ApiError;
use crate::eventstream::EventStreamDecoder;
use crate::eventstream::eventstream_payload_bytes;
use crate::telemetry::SseTelemetry;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use ody_client::ByteStream;
use ody_client::StreamResponse;
use ody_protocol::models::ContentItem;
use ody_protocol::models::ReasoningItemContent;
use ody_protocol::models::ResponseItem;
use ody_protocol::protocol::TokenUsage;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio::time::timeout;
use tracing::debug;
use tracing::trace;

/// Spawn a task that parses a direct-Anthropic SSE response into events.
pub fn spawn_anthropic_stream(
    stream_response: StreamResponse,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
) -> ResponseStream {
    let upstream_request_id = header_request_id(&stream_response);
    let (tx_event, rx_event) = mpsc::channel::<Result<ResponseEvent, ApiError>>(1600);
    tokio::spawn(async move {
        process_anthropic_sse(stream_response.bytes, tx_event, idle_timeout, telemetry).await;
    });
    ResponseStream {
        rx_event,
        upstream_request_id,
    }
}

/// Spawn a task that parses a Bedrock event stream response into events.
pub fn spawn_anthropic_eventstream(
    stream_response: StreamResponse,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
) -> ResponseStream {
    let upstream_request_id = header_request_id(&stream_response);
    let (tx_event, rx_event) = mpsc::channel::<Result<ResponseEvent, ApiError>>(1600);
    tokio::spawn(async move {
        process_anthropic_eventstream(stream_response.bytes, tx_event, idle_timeout, telemetry)
            .await;
    });
    ResponseStream {
        rx_event,
        upstream_request_id,
    }
}

fn header_request_id(stream_response: &StreamResponse) -> Option<String> {
    stream_response
        .headers
        .get("x-request-id")
        .or_else(|| stream_response.headers.get("x-amzn-requestid"))
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

async fn process_anthropic_sse(
    bytes: ByteStream,
    tx_event: mpsc::Sender<Result<ResponseEvent, ApiError>>,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
) {
    let mut stream = bytes.eventsource();
    let mut state = AnthropicStreamState::default();
    loop {
        let start = Instant::now();
        let response = timeout(idle_timeout, stream.next()).await;
        if let Some(t) = telemetry.as_ref() {
            t.on_sse_poll(&response, start.elapsed());
        }
        let sse = match response {
            Ok(Some(Ok(sse))) => sse,
            Ok(Some(Err(e))) => {
                debug!("anthropic SSE error: {e:#}");
                let _ = tx_event.send(Err(ApiError::Stream(e.to_string()))).await;
                return;
            }
            Ok(None) => break,
            Err(_) => {
                let _ = tx_event
                    .send(Err(ApiError::Stream(
                        "idle timeout waiting for SSE".into(),
                    )))
                    .await;
                return;
            }
        };
        if !handle_payload(&sse.data, &mut state, &tx_event).await {
            return;
        }
    }
    finalize(state, tx_event).await;
}

async fn process_anthropic_eventstream(
    bytes: ByteStream,
    tx_event: mpsc::Sender<Result<ResponseEvent, ApiError>>,
    idle_timeout: Duration,
    telemetry: Option<Arc<dyn SseTelemetry>>,
) {
    let mut stream = Box::pin(bytes);
    let mut decoder = EventStreamDecoder::new();
    let mut state = AnthropicStreamState::default();
    // SseTelemetry::on_sse_poll is typed for the SSE event shape; this path
    // polls raw byte chunks, so telemetry is skipped rather than faking an
    // event type.
    let _ = telemetry;
    loop {
        let response = timeout(idle_timeout, stream.next()).await;
        let chunk = match response {
            Ok(Some(Ok(chunk))) => chunk,
            Ok(Some(Err(e))) => {
                debug!("anthropic event stream error: {e:#}");
                let _ = tx_event.send(Err(ApiError::Stream(e.to_string()))).await;
                return;
            }
            Ok(None) => break,
            Err(_) => {
                let _ = tx_event
                    .send(Err(ApiError::Stream(
                        "idle timeout waiting for event stream".into(),
                    )))
                    .await;
                return;
            }
        };
        let payloads = match decoder.feed(&chunk) {
            Ok(payloads) => payloads,
            Err(e) => {
                let _ = tx_event.send(Err(e)).await;
                return;
            }
        };
        for payload in payloads {
            let event_bytes = match eventstream_payload_bytes(&payload) {
                Ok(bytes) => bytes,
                Err(e) => {
                    let _ = tx_event.send(Err(e)).await;
                    return;
                }
            };
            let Ok(text) = String::from_utf8(event_bytes) else {
                continue;
            };
            if !handle_payload(&text, &mut state, &tx_event).await {
                return;
            }
        }
    }
    finalize(state, tx_event).await;
}

/// Handle one raw Anthropic event JSON payload. Returns false when the
/// stream must abort (channel closed or wire error delivered).
async fn handle_payload(
    payload: &str,
    state: &mut AnthropicStreamState,
    tx_event: &mpsc::Sender<Result<ResponseEvent, ApiError>>,
) -> bool {
    trace!("anthropic event: {payload}");
    let value: Value = match serde_json::from_str(payload) {
        Ok(value) => value,
        Err(parse_err) => {
            debug!("failed to parse anthropic event: {parse_err}, data: {payload}");
            return true;
        }
    };
    match handle_anthropic_event(&value, state) {
        Ok(events) => {
            for event in events {
                if tx_event.send(Ok(event)).await.is_err() {
                    return false;
                }
            }
            true
        }
        Err(error) => {
            let _ = tx_event.send(Err(error)).await;
            false
        }
    }
}

async fn finalize(
    state: AnthropicStreamState,
    tx_event: mpsc::Sender<Result<ResponseEvent, ApiError>>,
) {
    for event in state.finish() {
        if tx_event.send(Ok(event)).await.is_err() {
            return;
        }
    }
}

/// Accumulated state for one Anthropic streaming response.
#[derive(Debug, Default)]
struct AnthropicStreamState {
    response_id: Option<String>,
    text: String,
    reasoning: String,
    finish_reason: Option<String>,
    input_tokens: i64,
    output_tokens: i64,
    cached_input_tokens: i64,
    tool_calls: BTreeMap<usize, ToolCallAccumulator>,
    current_block: Option<CurrentBlock>,
}

#[derive(Debug)]
enum CurrentBlock {
    Text { index: usize },
    Thinking { index: usize },
    ToolUse { index: usize },
}

#[derive(Debug, Default)]
struct ToolCallAccumulator {
    id: Option<String>,
    name: String,
    arguments: String,
}

/// Map one Anthropic event onto zero or more internal events. Pure and
/// unit-testable; the async wrappers only deal with framing and timeouts.
fn handle_anthropic_event(
    value: &Value,
    state: &mut AnthropicStreamState,
) -> Result<Vec<ResponseEvent>, ApiError> {
    let event_type = value.get("type").and_then(Value::as_str).unwrap_or("");
    match event_type {
        "message_start" => {
            if let Some(message) = value.get("message") {
                if let Some(id) = message.get("id").and_then(Value::as_str) {
                    state.response_id = Some(id.to_string());
                }
                if let Some(usage) = message.get("usage") {
                    state.input_tokens = usage
                        .get("input_tokens")
                        .and_then(Value::as_i64)
                        .unwrap_or(0);
                    state.cached_input_tokens = usage
                        .get("cache_read_input_tokens")
                        .and_then(Value::as_i64)
                        .unwrap_or(0);
                }
            }
            Ok(vec![ResponseEvent::Created])
        }
        "content_block_start" => {
            let index = value
                .get("index")
                .and_then(Value::as_u64)
                .unwrap_or_default() as usize;
            let block = value.get("content_block").cloned().unwrap_or(Value::Null);
            match block.get("type").and_then(Value::as_str) {
                Some("text") => state.current_block = Some(CurrentBlock::Text { index }),
                Some("thinking") => state.current_block = Some(CurrentBlock::Thinking { index }),
                Some("tool_use") | Some("server_tool_use") => {
                    let entry = state.tool_calls.entry(index).or_default();
                    entry.id = block.get("id").and_then(Value::as_str).map(str::to_string);
                    entry.name = block
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    state.current_block = Some(CurrentBlock::ToolUse { index });
                }
                _ => state.current_block = None,
            }
            Ok(Vec::new())
        }
        "content_block_delta" => {
            let index = match state.current_block {
                Some(CurrentBlock::Text { index })
                | Some(CurrentBlock::Thinking { index })
                | Some(CurrentBlock::ToolUse { index }) => index,
                None => return Ok(Vec::new()),
            };
            let delta = value.get("delta").cloned().unwrap_or(Value::Null);
            match delta.get("type").and_then(Value::as_str) {
                Some("text_delta") => {
                    let text = delta_text(&delta);
                    state.text.push_str(&text);
                    Ok(vec![ResponseEvent::OutputTextDelta(text)])
                }
                Some("thinking_delta") => {
                    let text = delta_text(&delta);
                    state.reasoning.push_str(&text);
                    Ok(vec![ResponseEvent::ReasoningContentDelta {
                        delta: text,
                        content_index: index as i64,
                    }])
                }
                Some("input_json_delta") => {
                    let partial = delta_text(&delta);
                    let entry = state.tool_calls.entry(index).or_default();
                    entry.arguments.push_str(&partial);
                    Ok(vec![ResponseEvent::ToolCallInputDelta {
                        item_id: format!("call_{index}"),
                        call_id: entry.id.clone(),
                        delta: partial,
                    }])
                }
                // Signature deltas close out thinking blocks; there is no
                // internal event to map them onto.
                _ => Ok(Vec::new()),
            }
        }
        "content_block_stop" => {
            state.current_block = None;
            Ok(Vec::new())
        }
        "message_delta" => {
            if let Some(delta) = value.get("delta") {
                if let Some(stop_reason) = delta.get("stop_reason").and_then(Value::as_str) {
                    state.finish_reason = Some(stop_reason.to_string());
                }
            }
            if let Some(usage) = value.get("usage") {
                state.output_tokens = usage
                    .get("output_tokens")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
            }
            Ok(Vec::new())
        }
        "message_stop" => Ok(Vec::new()),
        "ping" => Ok(Vec::new()),
        "error" => Err(anthropic_error(value)),
        other => {
            debug!("ignoring unknown anthropic event type: {other}");
            Ok(Vec::new())
        }
    }
}

fn delta_text(delta: &Value) -> String {
    delta
        .get("text")
        .or_else(|| delta.get("thinking"))
        .or_else(|| delta.get("partial_json"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn anthropic_error(value: &Value) -> ApiError {
    let error = value.get("error").cloned().unwrap_or(Value::Null);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("anthropic stream error")
        .to_string();
    match error.get("type").and_then(Value::as_str) {
        Some("overloaded_error") => ApiError::Stream(format!("anthropic overloaded: {message}")),
        _ => ApiError::Stream(message),
    }
}

impl AnthropicStreamState {
    /// Terminal events: accumulated output items, then `Completed`.
    fn finish(self) -> Vec<ResponseEvent> {
        let mut events = Vec::new();
        let produced_reasoning = !self.reasoning.is_empty();
        let produced_text = !self.text.is_empty();
        let has_tool_calls = !self.tool_calls.is_empty();

        if produced_reasoning {
            events.push(ResponseEvent::OutputItemDone(ResponseItem::Reasoning {
                id: None,
                summary: Vec::new(),
                content: Some(vec![ReasoningItemContent::ReasoningText {
                    text: self.reasoning,
                }]),
                encrypted_content: None,
                internal_chat_message_metadata_passthrough: None,
            }));
        }
        if produced_text {
            events.push(ResponseEvent::OutputItemDone(ResponseItem::Message {
                id: None,
                role: "assistant".to_string(),
                content: vec![ContentItem::OutputText { text: self.text }],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            }));
        }
        for (index, call) in self.tool_calls {
            let call_id = call.id.unwrap_or_else(|| format!("call_{index}"));
            events.push(ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
                id: None,
                name: call.name,
                namespace: None,
                arguments: call.arguments,
                call_id,
                internal_chat_message_metadata_passthrough: None,
            }));
        }

        if !produced_text && !produced_reasoning && !has_tool_calls {
            tracing::warn!(
                target: "ody_api::sse::anthropic",
                finish_reason = ?self.finish_reason,
                "anthropic stream completed with no output items (empty completion)"
            );
        }

        let end_turn = match self.finish_reason.as_deref() {
            Some("tool_use") => Some(false),
            Some(_) => Some(true),
            None => Some(!has_tool_calls),
        };
        let finish_reason = self.finish_reason.map(|reason| match reason.as_str() {
            "end_turn" => "stop".to_string(),
            "max_tokens" => "length".to_string(),
            "tool_use" => "tool_calls".to_string(),
            other => other.to_string(),
        });
        events.push(ResponseEvent::Completed {
            response_id: self.response_id.unwrap_or_default(),
            token_usage: Some(TokenUsage {
                input_tokens: self.input_tokens,
                cached_input_tokens: self.cached_input_tokens,
                output_tokens: self.output_tokens,
                reasoning_output_tokens: 0,
                total_tokens: self.input_tokens + self.output_tokens,
            }),
            end_turn,
            finish_reason,
        });
        events
    }
}

#[cfg(test)]
#[path = "anthropic_sse_tests.rs"]
mod anthropic_sse_tests;
