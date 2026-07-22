use super::*;
use ody_protocol::models::ContentItem;
use ody_protocol::models::FunctionCallOutputPayload;
use ody_protocol::models::ReasoningItemContent;
use ody_protocol::models::ResponseItem;
use pretty_assertions::assert_eq;
use serde_json::json;

fn user_message(text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: None,
        role: "user".to_string(),
        content: vec![ContentItem::InputText {
            text: text.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

fn base_request(vendor: ChatVendor) -> ChatCompletionsRequest {
    ChatCompletionsRequest {
        model: "test-model".to_string(),
        instructions: "be helpful".to_string(),
        input: vec![user_message("hello")],
        tools: Vec::new(),
        parallel_tool_calls: true,
        reasoning_effort: None,
        max_completion_tokens: None,
        temperature: None,
        top_p: None,
        stop: Vec::new(),
        vendor,
    }
}

#[test]
fn builds_system_and_user_messages() {
    let body = base_request(ChatVendor::Generic).to_wire();
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(messages[0]["content"], "be helpful");
    assert_eq!(messages[1]["role"], "user");
    assert_eq!(messages[1]["content"], "hello");
    assert_eq!(body["stream"], true);
    assert_eq!(body["stream_options"]["include_usage"], true);
}

#[test]
fn omits_system_message_when_instructions_empty() {
    let mut request = base_request(ChatVendor::Generic);
    request.instructions = String::new();
    let body = request.to_wire();
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"], "user");
}

#[test]
fn function_call_and_output_become_tool_messages() {
    let mut request = base_request(ChatVendor::Generic);
    request.input = vec![
        ResponseItem::FunctionCall {
            id: None,
            name: "do_thing".to_string(),
            namespace: None,
            arguments: "{\"x\":1}".to_string(),
            call_id: "call_1".to_string(),
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::FunctionCallOutput {
            id: None,
            call_id: "call_1".to_string(),
            output: FunctionCallOutputPayload::from_text("result".to_string()),
            internal_chat_message_metadata_passthrough: None,
        },
    ];
    let body = request.to_wire();
    let messages = body["messages"].as_array().unwrap();
    // system + assistant(tool_calls) + tool
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(messages[1]["tool_calls"][0]["id"], "call_1");
    assert_eq!(messages[1]["tool_calls"][0]["function"]["name"], "do_thing");
    assert_eq!(
        messages[1]["tool_calls"][0]["function"]["arguments"],
        "{\"x\":1}"
    );
    assert_eq!(messages[2]["role"], "tool");
    assert_eq!(messages[2]["tool_call_id"], "call_1");
    assert_eq!(messages[2]["content"], "result");
}

#[test]
fn function_tool_converted_to_chat_shape() {
    let mut request = base_request(ChatVendor::Generic);
    request.tools = vec![json!({
        "type": "function",
        "name": "shell",
        "description": "run a shell command",
        "parameters": { "type": "object", "properties": {} },
    })];
    let body = request.to_wire();
    let tools = body["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["type"], "function");
    assert_eq!(tools[0]["function"]["name"], "shell");
    assert_eq!(tools[0]["function"]["description"], "run a shell command");
    assert_eq!(body["tool_choice"], "auto");
    assert_eq!(body["parallel_tool_calls"], true);
}

#[test]
fn reasoning_effort_emitted_for_kimi_but_not_glm() {
    let mut kimi = base_request(ChatVendor::Kimi);
    kimi.reasoning_effort = Some("high".to_string());
    assert_eq!(kimi.to_wire()["reasoning_effort"], "high");

    let mut glm = base_request(ChatVendor::Glm);
    glm.reasoning_effort = Some("high".to_string());
    assert!(glm.to_wire().get("reasoning_effort").is_none());
}

#[test]
fn glm_disables_thinking_but_others_do_not() {
    // GLM defaults thinking ON server-side unless told otherwise; ody treats
    // GLM as non-thinking, so the wire must explicitly disable it.
    let glm = base_request(ChatVendor::Glm).to_wire();
    assert_eq!(glm["thinking"], json!({ "type": "disabled" }));

    for vendor in [ChatVendor::Generic, ChatVendor::Kimi, ChatVendor::DeepSeek] {
        assert!(
            base_request(vendor).to_wire().get("thinking").is_none(),
            "{vendor:?} must not carry a thinking field"
        );
    }
}

#[test]
fn glm_uses_max_tokens_field() {
    let mut glm = base_request(ChatVendor::Glm);
    glm.max_completion_tokens = Some(1024);
    let body = glm.to_wire();
    assert_eq!(body["max_tokens"], 1024);
    assert!(body.get("max_completion_tokens").is_none());

    let mut kimi = base_request(ChatVendor::Kimi);
    kimi.max_completion_tokens = Some(1024);
    assert_eq!(kimi.to_wire()["max_completion_tokens"], 1024);
}

#[test]
fn kimi_builtin_function_tool() {
    let mut request = base_request(ChatVendor::Kimi);
    request.tools = vec![json!({
        "type": "function",
        "name": "$web_search",
        "description": "search",
        "parameters": { "type": "object" },
    })];
    let body = request.to_wire();
    let tools = body["tools"].as_array().unwrap();
    assert_eq!(tools[0]["type"], "builtin_function");
    assert_eq!(tools[0]["function"]["name"], "web_search");
}

#[test]
fn kimi_sanitizes_long_tool_call_ids() {
    let long_id = "x".repeat(100);
    let mut request = base_request(ChatVendor::Kimi);
    request.input = vec![ResponseItem::FunctionCall {
        id: None,
        name: "f".to_string(),
        namespace: None,
        arguments: "{}".to_string(),
        call_id: long_id,
        internal_chat_message_metadata_passthrough: None,
    }];
    let body = request.to_wire();
    let id = body["messages"][1]["tool_calls"][0]["id"].as_str().unwrap();
    assert_eq!(id.chars().count(), 64);
}

#[test]
fn kimi_normalizes_tool_schema_missing_type() {
    let mut request = base_request(ChatVendor::Kimi);
    request.tools = vec![json!({
        "type": "function",
        "name": "f",
        "parameters": {
            "type": "object",
            "properties": { "name": { "description": "a name" } },
        },
    })];
    let body = request.to_wire();
    assert_eq!(
        body["tools"][0]["function"]["parameters"]["properties"]["name"]["type"],
        "string"
    );
}

#[test]
fn multiple_function_calls_merge_into_single_assistant_message() {
    let mut request = base_request(ChatVendor::Generic);
    request.input = vec![
        ResponseItem::FunctionCall {
            id: None,
            name: "first".to_string(),
            namespace: None,
            arguments: "{\"x\":1}".to_string(),
            call_id: "call_1".to_string(),
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::FunctionCall {
            id: None,
            name: "second".to_string(),
            namespace: None,
            arguments: "{\"y\":2}".to_string(),
            call_id: "call_2".to_string(),
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::FunctionCallOutput {
            id: None,
            call_id: "call_1".to_string(),
            output: FunctionCallOutputPayload::from_text("result1".to_string()),
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::FunctionCallOutput {
            id: None,
            call_id: "call_2".to_string(),
            output: FunctionCallOutputPayload::from_text("result2".to_string()),
            internal_chat_message_metadata_passthrough: None,
        },
    ];
    let body = request.to_wire();
    let messages = body["messages"].as_array().unwrap();
    // system + assistant(with both tool_calls) + tool + tool
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(messages[1]["tool_calls"].as_array().unwrap().len(), 2);
    assert_eq!(messages[1]["tool_calls"][0]["id"], "call_1");
    assert_eq!(messages[1]["tool_calls"][1]["id"], "call_2");
    assert_eq!(messages[2]["role"], "tool");
    assert_eq!(messages[3]["role"], "tool");
}

#[test]
fn assistant_content_followed_by_function_calls_merges() {
    let mut request = base_request(ChatVendor::Generic);
    request.input = vec![
        ResponseItem::Message {
            id: None,
            role: "assistant".to_string(),
            content: vec![ContentItem::OutputText {
                text: "I will run both tools.".to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::FunctionCall {
            id: None,
            name: "first".to_string(),
            namespace: None,
            arguments: "{}".to_string(),
            call_id: "call_1".to_string(),
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::FunctionCallOutput {
            id: None,
            call_id: "call_1".to_string(),
            output: FunctionCallOutputPayload::from_text("result1".to_string()),
            internal_chat_message_metadata_passthrough: None,
        },
    ];
    let body = request.to_wire();
    let messages = body["messages"].as_array().unwrap();
    // system + assistant(content + tool_calls) + tool
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(messages[1]["content"], "I will run both tools.");
    assert_eq!(messages[1]["tool_calls"].as_array().unwrap().len(), 1);
    assert_eq!(messages[1]["tool_calls"][0]["id"], "call_1");
    assert_eq!(messages[2]["role"], "tool");
}

#[test]
fn vendor_resolution_from_base_url() {
    assert_eq!(
        ChatVendor::from_provider("Kimi", Some("https://api.moonshot.ai/v1")),
        ChatVendor::Kimi
    );
    assert_eq!(
        ChatVendor::from_provider("custom", Some("https://api.kimi.com/coding/v1")),
        ChatVendor::Kimi
    );
    assert_eq!(
        ChatVendor::from_provider("custom", Some("https://api.deepseek.com/v1")),
        ChatVendor::DeepSeek
    );
    assert_eq!(ChatVendor::from_provider("GLM", None), ChatVendor::Glm);
    assert_eq!(
        ChatVendor::from_provider("whatever", None),
        ChatVendor::Generic
    );
}

fn reasoning(text: &str) -> ResponseItem {
    ResponseItem::Reasoning {
        id: None,
        summary: Vec::new(),
        content: Some(vec![ReasoningItemContent::ReasoningText {
            text: text.to_string(),
        }]),
        encrypted_content: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

fn function_call(name: &str, call_id: &str) -> ResponseItem {
    ResponseItem::FunctionCall {
        id: None,
        name: name.to_string(),
        namespace: None,
        arguments: "{}".to_string(),
        call_id: call_id.to_string(),
        internal_chat_message_metadata_passthrough: None,
    }
}

fn assistant_message(text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: None,
        role: "assistant".to_string(),
        content: vec![ContentItem::OutputText {
            text: text.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

fn messages_of(request: &ChatCompletionsRequest) -> Vec<Value> {
    request.to_wire()["messages"].as_array().unwrap().clone()
}

/// Kimi is a thinking model and conditions on its own prior reasoning: a probe
/// against `api.kimi.com` recovered a secret planted *only* in an inbound
/// `reasoning_content`. Dropping it makes the model re-derive its plan from the
/// tool output every turn.
#[test]
fn reasoning_is_replayed_on_the_assistant_message_it_precedes() {
    let mut request = base_request(ChatVendor::Kimi);
    request.input = vec![
        user_message("hi"),
        reasoning("I should call the tool."),
        function_call("do_it", "call_1"),
    ];

    let messages = messages_of(&request);
    let assistant = messages.iter().find(|m| is_assistant(m)).unwrap();
    assert_eq!(
        assistant["reasoning_content"],
        json!("I should call the tool.")
    );
    assert!(
        assistant.get("tool_calls").is_some(),
        "reasoning must ride on the tool_calls message, not a separate one: {assistant}"
    );
}

/// The unknown field is only safe where we know it is accepted.
#[test]
fn reasoning_is_not_replayed_to_generic_providers() {
    let mut request = base_request(ChatVendor::Generic);
    request.input = vec![reasoning("thinking"), function_call("do_it", "call_1")];

    let messages = messages_of(&request);
    assert!(
        !request.to_wire().to_string().contains("reasoning_content"),
        "generic gateways may reject the field: {messages:?}"
    );
}

/// Reasoning with no assistant message after it has nothing to ride on; it must
/// not synthesize a bare assistant message.
#[test]
fn trailing_reasoning_without_an_assistant_message_is_dropped() {
    let mut request = base_request(ChatVendor::Kimi);
    request.input = vec![user_message("hi"), reasoning("thinking")];

    let messages = messages_of(&request);
    assert_eq!(
        messages.iter().filter(|m| is_assistant(m)).count(),
        0,
        "expected no assistant message: {messages:?}"
    );
    assert!(!request.to_wire().to_string().contains("reasoning_content"));
}

/// `merge_consecutive_assistant_messages` collapses a content-only assistant
/// message into the tool_calls message that follows. The reasoning attached to
/// the collapsed message must survive.
#[test]
fn reasoning_survives_assistant_message_merging() {
    let mut request = base_request(ChatVendor::Kimi);
    request.input = vec![
        user_message("hi"),
        reasoning("first thought."),
        assistant_message("Working on it."),
        reasoning("second thought."),
        function_call("do_it", "call_1"),
    ];

    let messages = messages_of(&request);
    let assistants: Vec<_> = messages.iter().filter(|m| is_assistant(m)).collect();
    assert_eq!(assistants.len(), 1, "expected one merged assistant message");
    assert_eq!(
        assistants[0]["reasoning_content"],
        json!("first thought.second thought."),
        "both thoughts must survive the merge: {:?}",
        assistants[0]
    );
}
