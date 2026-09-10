//! Tests for the Anthropic Messages wire request builder.

use super::*;
use ody_protocol::models::ContentItem;
use ody_protocol::models::FunctionCallOutputPayload;
use ody_protocol::models::ReasoningItemContent;
use ody_protocol::models::ResponseItem;

fn request(input: Vec<ResponseItem>) -> AnthropicMessagesRequest {
    AnthropicMessagesRequest {
        model: "claude-sonnet-4-5".into(),
        input_modalities: vec![InputModality::Text, InputModality::Image],
        instructions: "You are helpful.".into(),
        input,
        tools: vec![],
        max_completion_tokens: None,
        temperature: None,
        top_p: None,
        stop: vec![],
        reasoning_effort: None,
        bedrock: false,
    }
}

fn user(text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: None,
        role: "user".into(),
        content: vec![ContentItem::InputText { text: text.into() }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

fn assistant(text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: None,
        role: "assistant".into(),
        content: vec![ContentItem::OutputText { text: text.into() }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

fn function_call(name: &str, call_id: &str, arguments: &str) -> ResponseItem {
    ResponseItem::FunctionCall {
        id: None,
        name: name.into(),
        arguments: arguments.into(),
        call_id: call_id.into(),
        namespace: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

fn tool_output(call_id: &str, text: &str) -> ResponseItem {
    ResponseItem::FunctionCallOutput {
        id: None,
        call_id: call_id.into(),
        output: FunctionCallOutputPayload::from_text(text.into()),
        internal_chat_message_metadata_passthrough: None,
    }
}

#[test]
fn to_wire_builds_system_messages_and_stream() {
    let body = request(vec![user("hello")]).to_wire();

    assert_eq!(body["model"], json!("claude-sonnet-4-5"));
    assert_eq!(body["stream"], json!(true));
    assert!(!body.as_object().unwrap().contains_key("anthropic_version"));
    assert_eq!(
        body["system"],
        json!([{ "type": "text", "text": "You are helpful." }])
    );
    assert_eq!(
        body["messages"],
        json!([{ "role": "user", "content": [{ "type": "text", "text": "hello" }] }])
    );
    assert_eq!(body["max_tokens"], json!(8192));
}

#[test]
fn to_wire_merges_consecutive_assistant_blocks_and_tool_traffic() {
    let input = vec![
        user("list files"),
        assistant(""),
        function_call("run_command", "call_1", r#"{"command":"ls"}"#),
        function_call("read_file", "call_2", r#"{"path":"a.txt"}"#),
        tool_output("call_1", "a.txt b.txt"),
        tool_output("call_2", "contents"),
        assistant("done"),
    ];
    let body = request(input).to_wire();
    let messages = body["messages"].as_array().expect("messages");

    // 1 user + 1 merged assistant(tool_use x2) + 1 merged user(tool_result x2) + 1 assistant
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[1]["role"], json!("assistant"));
    let blocks = messages[1]["content"].as_array().expect("blocks");
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0]["type"], json!("tool_use"));
    assert_eq!(blocks[0]["id"], json!("call_1"));
    assert_eq!(blocks[0]["name"], json!("run_command"));
    assert_eq!(blocks[0]["input"], json!({ "command": "ls" }));
    assert_eq!(blocks[1]["id"], json!("call_2"));
    assert_eq!(messages[2]["role"], json!("user"));
    let results = messages[2]["content"].as_array().expect("results");
    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["type"], json!("tool_result"));
    assert_eq!(results[0]["tool_use_id"], json!("call_1"));
    assert_eq!(results[0]["content"], json!("a.txt b.txt"));
}

#[test]
fn to_wire_inserts_user_stub_when_first_message_is_not_user() {
    let body = request(vec![assistant("hi there")]).to_wire();
    let messages = body["messages"].as_array().expect("messages");
    assert_eq!(messages[0]["role"], json!("user"));
    assert_eq!(messages[1]["role"], json!("assistant"));
}

#[test]
fn to_wire_drops_reasoning_and_folds_system_roles_into_system_field() {
    let reasoning = ResponseItem::Reasoning {
        id: None,
        summary: vec![],
        content: Some(vec![ReasoningItemContent::ReasoningText {
            text: "private thought".into(),
        }]),
        encrypted_content: None,
        internal_chat_message_metadata_passthrough: None,
    };
    let developer = ResponseItem::Message {
        id: None,
        role: "developer".into(),
        content: vec![ContentItem::InputText {
            text: "dev note".into(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    };
    let body = request(vec![developer, reasoning, user("hi")]).to_wire();

    let system = body["system"].as_array().expect("system");
    assert!(
        system
            .iter()
            .any(|block| block["text"] == json!("dev note")),
        "developer content folds into the system field"
    );
    assert!(
        system
            .iter()
            .all(|block| block["text"] != json!("private thought")),
        "reasoning is not replayed"
    );
}

#[test]
fn to_wire_converts_tools_to_anthropic_shape() {
    let mut req = request(vec![user("hi")]);
    req.tools = vec![json!({
        "type": "function",
        "name": "run_command",
        "description": "Run a shell command",
        "parameters": { "type": "object", "properties": { "command": { "type": "string" } } },
        "namespace": "shell",
    })];
    let body = req.to_wire();

    assert_eq!(
        body["tools"],
        json!([{
            "name": "run_command",
            "description": "Run a shell command",
            "input_schema": { "type": "object", "properties": { "command": { "type": "string" } } },
        }])
    );
}

#[test]
fn to_wire_bedrock_dialect_omits_stream_and_sets_anthropic_version() {
    let mut req = request(vec![user("hello")]);
    req.bedrock = true;
    let body = req.to_wire();

    assert_eq!(body["anthropic_version"], json!("bedrock-2023-05-31"));
    assert!(!body.as_object().unwrap().contains_key("stream"));
    assert_eq!(body["messages"][0]["role"], json!("user"));
}

#[test]
fn to_wire_thinking_budget_stays_below_max_tokens() {
    let mut req = request(vec![user("think!")]);
    req.reasoning_effort = Some("high".into());
    let body = req.to_wire();

    let budget = body["thinking"]["budget_tokens"].as_u64().expect("budget");
    let max_tokens = body["max_tokens"].as_u64().expect("max_tokens");
    assert!(budget >= 1024, "budget {budget}");
    assert!(budget < max_tokens, "budget {budget} < max_tokens {max_tokens}");
    assert_eq!(body["thinking"]["type"], json!("enabled"));
}

#[test]
fn to_wire_explicit_max_tokens_clamps_thinking_budget() {
    let mut req = request(vec![user("think!")]);
    req.reasoning_effort = Some("high".into());
    req.max_completion_tokens = Some(2048);
    let body = req.to_wire();

    let budget = body["thinking"]["budget_tokens"].as_u64().expect("budget");
    let max_tokens = body["max_tokens"].as_u64().expect("max_tokens");
    assert_eq!(max_tokens, 2048);
    assert!(budget < max_tokens, "budget {budget} < max_tokens {max_tokens}");
}

#[test]
fn to_wire_passes_sampling_and_stop_through() {
    let mut req = request(vec![user("hi")]);
    req.temperature = Some(0.5);
    req.top_p = Some(0.9);
    req.stop = vec!["END".into()];
    let body = req.to_wire();

    assert_eq!(body["temperature"], json!(0.5));
    assert_eq!(body["top_p"], json!(0.9));
    assert_eq!(body["stop_sequences"], json!(["END"]));
}

#[test]
fn to_wire_degrades_unsupported_media_to_text() {
    let req = request(vec![ResponseItem::Message {
        id: None,
        role: "user".into(),
        content: vec![
            ContentItem::InputText { text: "look".into() },
            ContentItem::InputAudio { audio_url: "https://x/a.mp3".into() },
        ],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }]);
    let body = req.to_wire();
    let blocks = body["messages"][0]["content"].as_array().expect("blocks");
    assert_eq!(blocks[1]["type"], json!("text"));
    assert!(
        blocks[1]["text"]
            .as_str()
            .expect("placeholder")
            .contains("omitted")
    );
}
