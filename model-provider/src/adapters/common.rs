//! Shared helpers for normalizing `ody_api::ResponseEvent` streams into the
//! provider-neutral `ChatEvent` model used by `ChatProvider`.

use crate::chat_provider::{
    ChatEvent, ChatProviderError, ContentPart, FinishReason, RawFrame, ToolCall, Usage,
};
use http::StatusCode;
use ody_api::{ApiError, ResponseEvent};
use ody_client::TransportError;
use ody_protocol::models::{ContentItem, ResponseItem};
use serde_json::Value;

/// Per-response state used while normalizing `ResponseEvent`s into `ChatEvent`s.
///
/// Some wire APIs emit an `OutputItemDone` snapshot that repeats the full item
/// text. When we have already streamed that text as `OutputTextDelta`s, the
/// snapshot must be dropped to avoid duplicating the message downstream.
#[derive(Debug, Default)]
pub(crate) struct NormalizeState {
    pub saw_text_delta: bool,
    pub saw_message_added: bool,
    pub saw_output: bool,
}

/// Normalize a single `ResponseEvent` into zero or more provider-neutral events.
///
/// Most events map 1:1, but a `Completed` event may produce both a `Usage` event
/// and a `Finish` event, so the return type is a vector.
///
/// This overload starts with a fresh state; streaming adapters should use
/// [`normalize_response_event_with_state`] so state is carried across events.
pub(crate) fn normalize_response_event(
    event: ResponseEvent,
) -> Result<Vec<ChatEvent>, ChatProviderError> {
    normalize_response_event_with_state(event, &mut NormalizeState::default())
}

pub(crate) fn normalize_response_event_with_state(
    event: ResponseEvent,
    state: &mut NormalizeState,
) -> Result<Vec<ChatEvent>, ChatProviderError> {
    match event {
        ResponseEvent::Created => Ok(vec![ChatEvent::Start]),
        ResponseEvent::OutputTextDelta(delta) => {
            state.saw_text_delta = true;
            if !delta.is_empty() {
                state.saw_output = true;
            }
            Ok(vec![ChatEvent::ContentPart(ContentPart::Text(delta))])
        }
        ResponseEvent::OutputItemAdded(item) => {
            if matches!(&item, ResponseItem::Message { .. }) {
                state.saw_message_added = true;
            }
            normalize_response_item(item)
        }
        ResponseEvent::OutputItemDone(item) => {
            // Any delivered item (message snapshot, tool call, reasoning, ...)
            // represents real model output, so a subsequent terminal `Completed`
            // must not be classified as an empty completion.
            state.saw_output = true;
            // A message-level `OutputItemDone` carries the full snapshot. If we
            // already received text for this item (either as deltas or in the
            // added event), downstream will reconstruct the final text from the
            // streamed deltas, so emitting this as another delta would duplicate
            // the message. Only emit it when it is the sole source of text.
            if matches!(&item, ResponseItem::Message { .. })
                && (state.saw_text_delta || state.saw_message_added)
            {
                return Ok(vec![]);
            }
            normalize_response_item(item)
        }
        // Tool-call argument deltas are partial JSON fragments. The complete tool
        // call is delivered by `OutputItemDone(ResponseItem::FunctionCall)`, which
        // is normalized above. Emit the delta as a raw frame so it does not produce
        // a malformed `ChatEvent::ToolCall`.
        ResponseEvent::ToolCallInputDelta { item_id, delta, .. } => {
            Ok(vec![ChatEvent::Raw(RawFrame::Json(serde_json::json!({
                "event": "tool_call_input_delta",
                "item_id": item_id,
                "delta": delta,
            })))])
        }
        ResponseEvent::ReasoningContentDelta { delta, .. } => {
            if !delta.is_empty() {
                state.saw_output = true;
            }
            Ok(vec![ChatEvent::ReasoningPart(delta)])
        }
        ResponseEvent::Completed {
            token_usage,
            end_turn,
            finish_reason,
            ..
        } => {
            // Detect a degenerate empty completion: the turn finished (or the
            // API did not signal an incomplete/paused turn) without producing
            // any text, reasoning, or tool call. Chat-wire providers such as
            // Kimi occasionally do this right when the model should have acted;
            // surfacing it as a retryable error lets the request- and turn-level
            // retry loops recover instead of silently ending the turn.
            // `end_turn == Some(false)` means the response is incomplete/paused
            // (more output is expected), so it is intentionally excluded. Both
            // `Some(true)` and `None` (APIs that do not populate `end_turn`)
            // are treated as terminal stops.
            if end_turn != Some(false) && !state.saw_output {
                return Err(ChatProviderError::Provider {
                    code: "empty_completion".into(),
                    message: "assistant returned no text and no tool call".into(),
                });
            }
            let mut events = Vec::new();
            if let Some(u) = token_usage {
                let usage = Usage {
                    input_tokens: u.input_tokens as u32,
                    output_tokens: u.output_tokens as u32,
                    reasoning_tokens: Some(u.reasoning_output_tokens as u32).filter(|v| *v > 0),
                    cached_input_tokens: Some(u.cached_input_tokens as u32).filter(|v| *v > 0),
                };
                events.push(ChatEvent::Usage(usage));
            }
            let reason = match finish_reason.as_deref() {
                Some("length") | Some("max_tokens") => FinishReason::MaxTokens,
                _ => match end_turn {
                    Some(true) => FinishReason::Stop,
                    Some(false) => FinishReason::Other("incomplete".into()),
                    None => FinishReason::Stop,
                },
            };
            events.push(ChatEvent::Finish {
                reason,
                raw_reason: finish_reason.or_else(|| {
                    end_turn.map(|e| {
                        if e {
                            "stop".into()
                        } else {
                            "incomplete".into()
                        }
                    })
                }),
            });
            Ok(events)
        }
        ResponseEvent::ModelsEtag(etag) => Ok(vec![ChatEvent::Raw(RawFrame::Json(
            serde_json::json!({"etag": etag}),
        ))]),
        ResponseEvent::ServerModel(model) => Ok(vec![ChatEvent::Raw(RawFrame::Json(
            serde_json::json!({"model": model}),
        ))]),
        ResponseEvent::SafetyBuffering(_) => Ok(vec![ChatEvent::Raw(RawFrame::Json(
            serde_json::json!({"event": "safety_buffering"}),
        ))]),
        ResponseEvent::ServerReasoningIncluded(v) => Ok(vec![ChatEvent::Raw(RawFrame::Json(
            serde_json::json!({"reasoning_included": v}),
        ))]),
        ResponseEvent::ModelVerifications(v) => Ok(vec![ChatEvent::Raw(RawFrame::Json(
            serde_json::to_value(v).unwrap_or_default(),
        ))]),
        ResponseEvent::TurnModerationMetadata(m) => Ok(vec![ChatEvent::Raw(RawFrame::Json(
            serde_json::to_value(m).unwrap_or_default(),
        ))]),
        ResponseEvent::ReasoningSummaryDelta { delta, .. } => {
            Ok(vec![ChatEvent::ReasoningPart(delta)])
        }
        ResponseEvent::ReasoningSummaryPartAdded { .. } => Ok(vec![ChatEvent::Raw(
            RawFrame::Json(serde_json::json!({"event": "reasoning_summary_part_added"})),
        )]),
    }
}

fn normalize_response_item(item: ResponseItem) -> Result<Vec<ChatEvent>, ChatProviderError> {
    match item {
        ResponseItem::Message { content, .. } => {
            let text = content
                .into_iter()
                .map(|c| match c {
                    ContentItem::InputText { text } | ContentItem::OutputText { text } => Ok(text),
                    ContentItem::InputImage { .. } => Err(ChatProviderError::Unsupported {
                        capability: "image content type".into(),
                    }),
                })
                .collect::<Result<Vec<_>, _>>()?
                .join("");
            Ok(vec![ChatEvent::ContentPart(ContentPart::Text(text))])
        }
        ResponseItem::FunctionCall {
            call_id,
            name,
            namespace,
            arguments,
            ..
        } => {
            let parsed = parse_or_string(arguments);
            Ok(vec![ChatEvent::ToolCall(ToolCall {
                id: call_id,
                name,
                namespace,
                arguments: parsed,
            })])
        }
        _ => Ok(vec![ChatEvent::Raw(RawFrame::Json(
            serde_json::to_value(&item).unwrap_or_default(),
        ))]),
    }
}

fn parse_or_string(value: String) -> serde_json::Value {
    serde_json::from_str(&value).unwrap_or_else(|_| serde_json::Value::String(value))
}

/// Map an `ody_api::ApiError` to a `ChatProviderError` while preserving the
/// error category (server overloaded, invalid request, etc.) so that downstream
/// code can recover the right `OdyErr`/`OdyErrorInfo` for retries and UX.
pub(crate) fn chat_provider_error_from_api_error(err: ApiError) -> ChatProviderError {
    match err {
        ApiError::ServerOverloaded => ChatProviderError::Provider {
            code: "server_overloaded".into(),
            message: "server overloaded".into(),
        },
        ApiError::InvalidRequest { message } => ChatProviderError::Provider {
            code: "invalid_request".into(),
            message,
        },
        ApiError::ContextWindowExceeded => ChatProviderError::Provider {
            code: "context_window_exceeded".into(),
            message: "context window exceeded".into(),
        },
        ApiError::QuotaExceeded => ChatProviderError::Provider {
            code: "quota_exceeded".into(),
            message: "quota exceeded".into(),
        },
        ApiError::UsageNotIncluded => ChatProviderError::Provider {
            code: "usage_not_included".into(),
            message: "usage not included".into(),
        },
        ApiError::CyberPolicy { message } => ChatProviderError::Provider {
            code: "cyber_policy".into(),
            message,
        },
        ApiError::Retryable { message, .. } => ChatProviderError::Provider {
            code: "retryable".into(),
            message,
        },
        ApiError::Stream(message) => ChatProviderError::Provider {
            code: "stream".into(),
            message,
        },
        ApiError::Api { status, message } => api_error_to_provider_error(status, message),
        ApiError::Transport(TransportError::Http { status, body, .. }) => {
            http_error_to_provider_error(status, body.unwrap_or_default())
        }
        ApiError::Transport(TransportError::RetryLimit) => ChatProviderError::Provider {
            code: "retry_limit".into(),
            message: "retry limit reached".into(),
        },
        ApiError::Transport(TransportError::Timeout) => {
            ChatProviderError::Transport("request timed out".into())
        }
        ApiError::Transport(TransportError::Network(message))
        | ApiError::Transport(TransportError::Build(message)) => {
            ChatProviderError::Transport(message)
        }
    }
}

fn api_error_to_provider_error(status: StatusCode, message: String) -> ChatProviderError {
    http_error_to_provider_error(status, message)
}

fn http_error_to_provider_error(status: StatusCode, body: String) -> ChatProviderError {
    if status == StatusCode::BAD_REQUEST {
        // Mirror `ody_api::map_api_error`: 400s are invalid requests unless they
        // carry the cyber_policy error code.
        if let Ok(parsed) = serde_json::from_str::<Value>(&body)
            && let Some(error) = parsed.get("error")
            && error.get("code").and_then(Value::as_str) == Some("cyber_policy")
        {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .filter(|message| !message.trim().is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| {
                    "This request has been flagged for possible cybersecurity risk.".to_string()
                });
            return ChatProviderError::Provider {
                code: "cyber_policy".into(),
                message,
            };
        }
        return ChatProviderError::Provider {
            code: "invalid_request".into(),
            message: body,
        };
    }

    if status == StatusCode::INTERNAL_SERVER_ERROR {
        return ChatProviderError::Provider {
            code: "internal_server_error".into(),
            message: body,
        };
    }

    if status == StatusCode::TOO_MANY_REQUESTS {
        return ChatProviderError::Provider {
            code: "rate_limit".into(),
            message: body,
        };
    }

    if status == StatusCode::SERVICE_UNAVAILABLE {
        return ChatProviderError::Provider {
            code: "server_overloaded".into(),
            message: body,
        };
    }

    ChatProviderError::Provider {
        code: status.as_u16().to_string(),
        message: body,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_call_input_delta_emits_raw_frame() {
        let event = ResponseEvent::ToolCallInputDelta {
            item_id: "ctc_1".into(),
            call_id: Some("call_1".into()),
            delta: r#"{"path":"#.into(),
        };
        let chat = normalize_response_event(event).expect("normalizes");
        assert_eq!(chat.len(), 1);
        assert!(
            matches!(&chat[0], ChatEvent::Raw(RawFrame::Json(v)) if v.get("event") == Some(&serde_json::json!("tool_call_input_delta")))
        );
    }

    #[test]
    fn output_item_done_message_dropped_after_text_delta() {
        let mut state = NormalizeState::default();
        let delta = normalize_response_event_with_state(
            ResponseEvent::OutputTextDelta("hello ".into()),
            &mut state,
        )
        .expect("normalizes");
        assert!(matches!(&delta[0], ChatEvent::ContentPart(ContentPart::Text(t)) if t == "hello "));

        let done = normalize_response_event_with_state(
            ResponseEvent::OutputItemDone(ResponseItem::Message {
                id: Some("msg-1".into()),
                role: "assistant".into(),
                content: vec![ContentItem::OutputText {
                    text: "hello world".into(),
                }],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            }),
            &mut state,
        )
        .expect("normalizes");
        assert!(done.is_empty());
    }

    #[test]
    fn standalone_output_item_done_message_emits_text() {
        let mut state = NormalizeState::default();
        let done = normalize_response_event_with_state(
            ResponseEvent::OutputItemDone(ResponseItem::Message {
                id: Some("msg-1".into()),
                role: "assistant".into(),
                content: vec![ContentItem::OutputText { text: "done".into() }],
                phase: None,
                internal_chat_message_metadata_passthrough: None,
            }),
            &mut state,
        )
        .expect("normalizes");
        assert_eq!(done.len(), 1);
        assert!(matches!(&done[0], ChatEvent::ContentPart(ContentPart::Text(t)) if t == "done"));
    }

    #[test]
    fn empty_completion_when_no_output_and_end_turn() {
        let mut state = NormalizeState::default();
        let created = normalize_response_event_with_state(ResponseEvent::Created, &mut state)
            .expect("created normalizes");
        assert_eq!(created.len(), 1);

        let err = normalize_response_event_with_state(
            ResponseEvent::Completed {
                response_id: "resp_1".into(),
                token_usage: None,
                end_turn: Some(true),
                finish_reason: None,
            },
            &mut state,
        )
        .expect_err("empty completion with end_turn=true should error");
        assert_eq!(
            err,
            ChatProviderError::Provider {
                code: "empty_completion".into(),
                message: "assistant returned no text and no tool call".into(),
            }
        );
    }

    #[test]
    fn empty_completion_when_no_output_and_end_turn_unspecified() {
        // Chat-wire providers (and the test SSE helpers) often omit `end_turn`
        // on `response.completed`, leaving it as `None`. That terminal-but-
        // unspecified case must still be detected as an empty completion.
        let mut state = NormalizeState::default();
        normalize_response_event_with_state(ResponseEvent::Created, &mut state)
            .expect("created normalizes");
        let err = normalize_response_event_with_state(
            ResponseEvent::Completed {
                response_id: "resp_1".into(),
                token_usage: None,
                end_turn: None,
                finish_reason: None,
            },
            &mut state,
        )
        .expect_err("empty completion with end_turn=None should error");
        assert_eq!(
            err,
            ChatProviderError::Provider {
                code: "empty_completion".into(),
                message: "assistant returned no text and no tool call".into(),
            }
        );
    }

    #[test]
    fn non_empty_completion_with_text_delta_is_allowed() {
        let mut state = NormalizeState::default();
        normalize_response_event_with_state(
            ResponseEvent::OutputTextDelta("hi".into()),
            &mut state,
        )
        .expect("text delta normalizes");
        let completed = normalize_response_event_with_state(
            ResponseEvent::Completed {
                response_id: "resp_1".into(),
                token_usage: None,
                end_turn: Some(true),
                finish_reason: None,
            },
            &mut state,
        )
        .expect("completion with text delta is normal");
        assert!(matches!(
            completed.as_slice(),
            [ChatEvent::Finish {
                reason: FinishReason::Stop,
                ..
            }]
        ));
    }

    #[test]
    fn non_empty_completion_with_tool_call_is_allowed() {
        let mut state = NormalizeState::default();
        normalize_response_event_with_state(
            ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
                id: None,
                name: "read_file".into(),
                namespace: None,
                arguments: r#"{"path":"/tmp"}"#.into(),
                call_id: "call_1".into(),
                internal_chat_message_metadata_passthrough: None,
            }),
            &mut state,
        )
        .expect("tool call normalizes");
        let completed = normalize_response_event_with_state(
            ResponseEvent::Completed {
                response_id: "resp_1".into(),
                token_usage: None,
                end_turn: Some(true),
                finish_reason: None,
            },
            &mut state,
        )
        .expect("completion with tool call is normal");
        assert!(matches!(
            completed.as_slice(),
            [ChatEvent::Finish {
                reason: FinishReason::Stop,
                ..
            }]
        ));
    }

    #[test]
    fn incomplete_end_turn_does_not_trigger_empty_completion() {
        let mut state = NormalizeState::default();
        let completed = normalize_response_event_with_state(
            ResponseEvent::Completed {
                response_id: "resp_1".into(),
                token_usage: None,
                end_turn: Some(false),
                finish_reason: None,
            },
            &mut state,
        )
        .expect("end_turn=false should not be treated as empty completion");
        assert!(matches!(
            completed.as_slice(),
            [ChatEvent::Finish {
                reason: FinishReason::Other(_),
                ..
            }]
        ));
    }

    #[test]
    fn length_finish_reason_maps_to_max_tokens() {
        let mut state = NormalizeState::default();
        normalize_response_event_with_state(
            ResponseEvent::OutputTextDelta("hi".into()),
            &mut state,
        )
        .expect("text delta normalizes");
        let completed = normalize_response_event_with_state(
            ResponseEvent::Completed {
                response_id: "resp_1".into(),
                token_usage: None,
                end_turn: Some(true),
                finish_reason: Some("length".into()),
            },
            &mut state,
        )
        .expect("completion with text delta is normal");
        assert_eq!(
            completed.as_slice(),
            &[ChatEvent::Finish {
                reason: FinishReason::MaxTokens,
                raw_reason: Some("length".into()),
            }]
        );
    }

    #[test]
    fn max_tokens_finish_reason_variant_maps_to_max_tokens() {
        let mut state = NormalizeState::default();
        normalize_response_event_with_state(
            ResponseEvent::OutputTextDelta("hi".into()),
            &mut state,
        )
        .expect("text delta normalizes");
        let completed = normalize_response_event_with_state(
            ResponseEvent::Completed {
                response_id: "resp_1".into(),
                token_usage: None,
                end_turn: Some(true),
                finish_reason: Some("max_tokens".into()),
            },
            &mut state,
        )
        .expect("completion with text delta is normal");
        assert_eq!(
            completed.as_slice(),
            &[ChatEvent::Finish {
                reason: FinishReason::MaxTokens,
                raw_reason: Some("max_tokens".into()),
            }]
        );
    }

    #[test]
    fn tool_calls_finish_reason_does_not_map_to_max_tokens() {
        let mut state = NormalizeState::default();
        normalize_response_event_with_state(
            ResponseEvent::OutputTextDelta("hi".into()),
            &mut state,
        )
        .expect("text delta normalizes");
        let completed = normalize_response_event_with_state(
            ResponseEvent::Completed {
                response_id: "resp_1".into(),
                token_usage: None,
                end_turn: Some(false),
                finish_reason: Some("tool_calls".into()),
            },
            &mut state,
        )
        .expect("completion with text delta is normal");
        assert_eq!(
            completed.as_slice(),
            &[ChatEvent::Finish {
                reason: FinishReason::Other("incomplete".into()),
                raw_reason: Some("tool_calls".into()),
            }]
        );
    }

    #[test]
    fn stop_finish_reason_still_maps_to_stop() {
        let mut state = NormalizeState::default();
        normalize_response_event_with_state(
            ResponseEvent::OutputTextDelta("hi".into()),
            &mut state,
        )
        .expect("text delta normalizes");
        let completed = normalize_response_event_with_state(
            ResponseEvent::Completed {
                response_id: "resp_1".into(),
                token_usage: None,
                end_turn: Some(true),
                finish_reason: Some("stop".into()),
            },
            &mut state,
        )
        .expect("completion with text delta is normal");
        assert_eq!(
            completed.as_slice(),
            &[ChatEvent::Finish {
                reason: FinishReason::Stop,
                raw_reason: Some("stop".into()),
            }]
        );
    }
}
