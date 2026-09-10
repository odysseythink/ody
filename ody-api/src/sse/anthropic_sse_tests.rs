//! Tests for the Anthropic streaming event parser.

use super::*;

fn event(json: &str) -> Value {
    serde_json::from_str(json).expect("valid event json")
}

fn handle(json: &str, state: &mut AnthropicStreamState) -> Vec<ResponseEvent> {
    handle_anthropic_event(&event(json), state).expect("event handled")
}

#[test]
fn message_start_emits_created_and_captures_input_usage() {
    let mut state = AnthropicStreamState::default();
    let events = handle(
        r#"{"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":12,"cache_read_input_tokens":7,"output_tokens":1}}}"#,
        &mut state,
    );
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0], ResponseEvent::Created));
    assert_eq!(state.response_id.as_deref(), Some("msg_1"));
    assert_eq!(state.input_tokens, 12);
    assert_eq!(state.cached_input_tokens, 7);
}

#[test]
fn text_deltas_accumulate_and_stream() {
    let mut state = AnthropicStreamState::default();
    handle(
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"text"}}"#,
        &mut state,
    );
    let events = handle(
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hel"}}"#,
        &mut state,
    );
    assert!(matches!(
        &events[0],
        ResponseEvent::OutputTextDelta(text) if text == "hel"
    ));
    handle(
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"lo"}}"#,
        &mut state,
    );
    assert_eq!(state.text, "hello");
}

#[test]
fn thinking_deltas_map_to_reasoning_events() {
    let mut state = AnthropicStreamState::default();
    handle(
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking"}}"#,
        &mut state,
    );
    let events = handle(
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"hmm"}}"#,
        &mut state,
    );
    match &events[0] {
        ResponseEvent::ReasoningContentDelta { delta, content_index } => {
            assert_eq!(delta, "hmm");
            assert_eq!(*content_index, 0);
        }
        other => panic!("expected reasoning delta, got {other:?}"),
    }
    assert_eq!(state.reasoning, "hmm");
}

#[test]
fn tool_use_blocks_accumulate_arguments() {
    let mut state = AnthropicStreamState::default();
    handle(
        r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"run_command"}}"#,
        &mut state,
    );
    let events = handle(
        r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"command\":"}}"#,
        &mut state,
    );
    match &events[0] {
        ResponseEvent::ToolCallInputDelta { item_id, call_id, delta } => {
            assert_eq!(item_id, "call_1");
            assert_eq!(call_id.as_deref(), Some("toolu_1"));
            assert_eq!(delta, "{\"command\":");
        }
        other => panic!("expected tool call delta, got {other:?}"),
    }
    handle(
        r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\"ls\"}"}}"#,
        &mut state,
    );
    assert_eq!(state.tool_calls[&1].arguments, "{\"command\":\"ls\"}");
}

#[test]
fn message_delta_captures_stop_reason_and_output_usage() {
    let mut state = AnthropicStreamState::default();
    handle(r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":9}}"#, &mut state);
    assert_eq!(state.finish_reason.as_deref(), Some("end_turn"));
    assert_eq!(state.output_tokens, 9);
}

#[test]
fn error_event_aborts_the_stream() {
    let mut state = AnthropicStreamState::default();
    let err = handle_anthropic_event(
        &event(r#"{"type":"error","error":{"type":"overloaded_error","message":"try again"}}"#),
        &mut state,
    )
    .expect_err("error events abort");
    assert!(err.to_string().contains("try again"), "{err}");
}

#[test]
fn ping_and_unknown_events_are_ignored() {
    let mut state = AnthropicStreamState::default();
    assert!(handle(r#"{"type":"ping"}"#, &mut state).is_empty());
    assert!(handle(r#"{"type":"future_event","x":1}"#, &mut state).is_empty());
}

#[test]
fn finish_emits_items_and_completed_with_mapped_finish_reason() {
    let mut state = AnthropicStreamState::default();
    state.response_id = Some("msg_9".into());
    state.text = "answer".into();
    state.reasoning = "thought".into();
    state.input_tokens = 10;
    state.cached_input_tokens = 3;
    state.output_tokens = 5;
    state.finish_reason = Some("end_turn".into());
    let mut acc = super::ToolCallAccumulator::default();
    acc.id = Some("toolu_1".into());
    acc.name = "run_command".into();
    acc.arguments = "{}".into();
    state.tool_calls.insert(0, acc);

    let events = state.finish();
    assert_eq!(events.len(), 4);
    assert!(matches!(events[0], ResponseEvent::OutputItemDone(ResponseItem::Reasoning { .. })));
    assert!(matches!(events[1], ResponseEvent::OutputItemDone(ResponseItem::Message { .. })));
    match &events[2] {
        ResponseEvent::OutputItemDone(ResponseItem::FunctionCall { name, call_id, arguments, .. }) => {
            assert_eq!(name, "run_command");
            assert_eq!(call_id, "toolu_1");
            assert_eq!(arguments, "{}");
        }
        other => panic!("expected function call item, got {other:?}"),
    }
    match &events[3] {
        ResponseEvent::Completed { response_id, token_usage, end_turn, finish_reason } => {
            assert_eq!(response_id, "msg_9");
            let usage = token_usage.as_ref().expect("usage");
            assert_eq!(usage.input_tokens, 10);
            assert_eq!(usage.cached_input_tokens, 3);
            assert_eq!(usage.output_tokens, 5);
            assert_eq!(usage.total_tokens, 15);
            assert_eq!(*end_turn, Some(true));
            assert_eq!(finish_reason.as_deref(), Some("stop"));
        }
        other => panic!("expected completed, got {other:?}"),
    }
}

#[test]
fn tool_use_stop_reason_keeps_the_turn_open() {
    let mut state = AnthropicStreamState::default();
    state.finish_reason = Some("tool_use".into());
    let mut acc = super::ToolCallAccumulator::default();
    acc.id = Some("toolu_2".into());
    acc.name = "read".into();
    acc.arguments = "{}".into();
    state.tool_calls.insert(0, acc);

    let events = state.finish();
    match events.last().expect("completed") {
        ResponseEvent::Completed { end_turn, finish_reason, .. } => {
            assert_eq!(*end_turn, Some(false));
            assert_eq!(finish_reason.as_deref(), Some("tool_calls"));
        }
        other => panic!("expected completed, got {other:?}"),
    }
}

#[test]
fn missing_stop_reason_ends_turn_when_no_tool_calls() {
    let mut state = AnthropicStreamState::default();
    state.text = "hi".into();
    let events = state.finish();
    match events.last().expect("completed") {
        ResponseEvent::Completed { end_turn, finish_reason, .. } => {
            assert_eq!(*end_turn, Some(true));
            assert!(finish_reason.is_none());
        }
        other => panic!("expected completed, got {other:?}"),
    }
}

#[test]
fn max_tokens_maps_to_length_finish_reason() {
    let mut state = AnthropicStreamState::default();
    state.text = "partial".into();
    state.finish_reason = Some("max_tokens".into());
    let events = state.finish();
    match events.last().expect("completed") {
        ResponseEvent::Completed { finish_reason, .. } => {
            assert_eq!(finish_reason.as_deref(), Some("length"));
        }
        other => panic!("expected completed, got {other:?}"),
    }
}
