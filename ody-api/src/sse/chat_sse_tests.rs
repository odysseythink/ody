use super::*;
use futures::StreamExt;
use ody_client::TransportError;
use pretty_assertions::assert_eq;
use tokio::sync::mpsc;
use tokio_util::io::ReaderStream;

fn idle_timeout() -> Duration {
    Duration::from_secs(30)
}

/// Run the chat SSE parser over a body made of `data: ...` lines and collect
/// the resulting events.
async fn run_chat_sse(data_lines: &[&str], vendor: ChatVendor) -> Vec<ResponseEvent> {
    let mut body = String::new();
    for line in data_lines {
        body.push_str("data: ");
        body.push_str(line);
        body.push_str("\n\n");
    }

    let (tx, mut rx) = mpsc::channel::<Result<ResponseEvent, ApiError>>(64);
    let stream = ReaderStream::new(std::io::Cursor::new(body))
        .map(|chunk| chunk.map_err(|err| TransportError::Network(err.to_string())));
    tokio::spawn(process_chat_sse(
        Box::pin(stream),
        tx,
        idle_timeout(),
        /*telemetry*/ None,
        vendor,
    ));

    let mut out = Vec::new();
    while let Some(ev) = rx.recv().await {
        out.push(ev.expect("stream produced error"));
    }
    out
}

#[tokio::test]
async fn parses_text_and_completion() {
    let events = run_chat_sse(
        &[
            r#"{"id":"resp_1","choices":[{"delta":{"role":"assistant","content":"Hel"}}]}"#,
            r#"{"id":"resp_1","choices":[{"delta":{"content":"lo"}}]}"#,
            r#"{"id":"resp_1","choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            r#"{"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}}"#,
            "[DONE]",
        ],
        ChatVendor::Generic,
    )
    .await;

    assert!(matches!(events.first(), Some(ResponseEvent::Created)));
    let deltas: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            ResponseEvent::OutputTextDelta(d) => Some(d.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(deltas, vec!["Hel", "lo"]);

    let message = events.iter().find_map(|e| match e {
        ResponseEvent::OutputItemDone(ResponseItem::Message { content, .. }) => Some(content),
        _ => None,
    });
    assert!(message.is_some(), "expected an assistant message item");

    match events.last() {
        Some(ResponseEvent::Completed {
            response_id,
            token_usage,
            end_turn,
        }) => {
            assert_eq!(response_id, "resp_1");
            assert_eq!(*end_turn, Some(true));
            let usage = token_usage.as_ref().expect("usage present");
            assert_eq!(usage.input_tokens, 3);
            assert_eq!(usage.output_tokens, 2);
            assert_eq!(usage.total_tokens, 5);
        }
        other => panic!("expected Completed, got {other:?}"),
    }
}

#[tokio::test]
async fn parses_reasoning_content() {
    let events = run_chat_sse(
        &[
            r#"{"choices":[{"delta":{"reasoning_content":"thinking..."}}]}"#,
            r#"{"choices":[{"delta":{"content":"answer"}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            "[DONE]",
        ],
        ChatVendor::DeepSeek,
    )
    .await;

    let reasoning_delta = events.iter().any(|e| {
        matches!(e, ResponseEvent::ReasoningContentDelta { delta, .. } if delta == "thinking...")
    });
    assert!(reasoning_delta, "expected reasoning content delta");

    let reasoning_item = events.iter().any(|e| {
        matches!(
            e,
            ResponseEvent::OutputItemDone(ResponseItem::Reasoning { .. })
        )
    });
    assert!(reasoning_item, "expected a reasoning output item");
}

#[tokio::test]
async fn emits_output_item_added_before_deltas() {
    let events = run_chat_sse(
        &[
            r#"{"id":"resp_1","choices":[{"delta":{"content":"Hel"}}]}"#,
            r#"{"id":"resp_1","choices":[{"delta":{"content":"lo"}}]}"#,
            r#"{"choices":[{"delta":{"reasoning_content":"think"}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            "[DONE]",
        ],
        ChatVendor::Kimi,
    )
    .await;

    let text_delta_index = events
        .iter()
        .position(|e| matches!(e, ResponseEvent::OutputTextDelta(d) if d == "Hel"))
        .expect("expected first text delta");
    let reasoning_delta_index = events
        .iter()
        .position(|e| matches!(e, ResponseEvent::ReasoningContentDelta { delta, .. } if delta == "think"))
        .expect("expected reasoning delta");

    assert!(
        matches!(
            &events[text_delta_index - 1],
            ResponseEvent::OutputItemAdded(ResponseItem::Message { role, .. }) if role == "assistant"
        ),
        "expected OutputItemAdded(Message) before first text delta"
    );
    assert!(
        matches!(
            events[reasoning_delta_index - 1],
            ResponseEvent::OutputItemAdded(ResponseItem::Reasoning { .. })
        ),
        "expected OutputItemAdded(Reasoning) before first reasoning delta"
    );
}

#[tokio::test]
async fn aggregates_tool_call_across_chunks() {
    let events = run_chat_sse(
        &[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_42","function":{"name":"shell","arguments":"{\"cmd\":"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"ls\"}"}}]}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
            "[DONE]",
        ],
        ChatVendor::Generic,
    )
    .await;

    let func = events.iter().find_map(|e| match e {
        ResponseEvent::OutputItemDone(ResponseItem::FunctionCall {
            name,
            arguments,
            call_id,
            ..
        }) => Some((name.clone(), arguments.clone(), call_id.clone())),
        _ => None,
    });
    let (name, arguments, call_id) = func.expect("expected a function call item");
    assert_eq!(name, "shell");
    assert_eq!(arguments, "{\"cmd\":\"ls\"}");
    assert_eq!(call_id, "call_42");

    match events.last() {
        Some(ResponseEvent::Completed { end_turn, .. }) => {
            assert_eq!(*end_turn, Some(false), "tool_calls turn should not end");
        }
        other => panic!("expected Completed, got {other:?}"),
    }
}

#[tokio::test]
async fn surfaces_error_object() {
    let (tx, mut rx) = mpsc::channel::<Result<ResponseEvent, ApiError>>(8);
    let body = "data: {\"error\":{\"message\":\"bad request\",\"code\":\"invalid\"}}\n\n".to_string();
    let stream = ReaderStream::new(std::io::Cursor::new(body))
        .map(|chunk| chunk.map_err(|err| TransportError::Network(err.to_string())));
    tokio::spawn(process_chat_sse(
        Box::pin(stream),
        tx,
        idle_timeout(),
        None,
        ChatVendor::Generic,
    ));

    let mut saw_error = false;
    while let Some(ev) = rx.recv().await {
        if let Err(ApiError::Stream(msg)) = ev {
            assert!(msg.contains("bad request"));
            saw_error = true;
        }
    }
    assert!(saw_error, "expected a stream error");
}
