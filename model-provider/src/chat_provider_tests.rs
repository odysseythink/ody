//! Additional tests for the unified chat provider types.

use crate::chat_provider::{
    ChatCompletion, ChatEvent, ChatProviderError, ChatRequest, ContentPart, FinishReason, Message,
    ProviderCapabilities, Role, ThinkingEffort, ToolCall, Usage, clamp_thinking_effort,
};

#[test]
fn chat_request_serializes_with_defaults() {
    let req = ChatRequest {
        model: "kimi-k2".into(),
        ..Default::default()
    };
    let json = serde_json::to_value(&req).expect("serializes");
    assert_eq!(json["model"], "kimi-k2");
    assert_eq!(json["messages"], serde_json::json!([]));
}

#[test]
fn message_with_text_part_roundtrips() {
    let msg = Message {
        role: Role::User,
        content: vec![ContentPart::Text("hello".into())],
        tool_calls: vec![],
        tool_call_id: None,
    };
    let json = serde_json::to_string(&msg).expect("serializes");
    let back: Message = serde_json::from_str(&json).expect("deserializes");
    assert_eq!(back.role, Role::User);
    assert_eq!(back.content, vec![ContentPart::Text("hello".into())]);
}

#[test]
fn finish_reason_defaults_to_stop() {
    assert_eq!(FinishReason::default(), FinishReason::Stop);
}

#[test]
fn usage_with_reasoning_roundtrips() {
    let usage = Usage {
        input_tokens: 10,
        output_tokens: 20,
        reasoning_tokens: Some(5),
    };
    let json = serde_json::to_string(&usage).expect("serializes");
    let back: Usage = serde_json::from_str(&json).expect("deserializes");
    assert_eq!(back.reasoning_tokens, Some(5));
}

#[test]
fn chat_event_error_contains_message() {
    let event = ChatEvent::Error(ChatProviderError::Unsupported {
        capability: "vision".into(),
    });
    let json = serde_json::to_string(&event).expect("serializes");
    assert!(json.contains("vision"));
}

#[test]
fn chat_completion_roundtrip() {
    let completion = ChatCompletion {
        message: Message {
            role: Role::Assistant,
            content: vec![ContentPart::Text("hi".into())],
            tool_calls: vec![ToolCall {
                id: "call_1".into(),
                name: "read_file".into(),
                arguments: serde_json::json!({"path": "/tmp"}),
            }],
            tool_call_id: None,
        },
        usage: Some(Usage {
            input_tokens: 4,
            output_tokens: 8,
            reasoning_tokens: None,
        }),
        finish_reason: FinishReason::ToolCalls,
        raw_finish_reason: Some("tool_calls".into()),
    };
    let json = serde_json::to_string(&completion).expect("serializes");
    let back: ChatCompletion = serde_json::from_str(&json).expect("deserializes");
    assert_eq!(back.finish_reason, FinishReason::ToolCalls);
    assert_eq!(back.message.tool_calls.len(), 1);
}

#[test]
fn provider_capabilities_thinking_effort_clamp() {
    let caps = ProviderCapabilities {
        thinking_effort: vec![ThinkingEffort::Low, ThinkingEffort::High],
        ..Default::default()
    };
    let clamped = clamp_thinking_effort(ThinkingEffort::Max, &caps.thinking_effort);
    assert_eq!(clamped, Some(ThinkingEffort::High));
}
