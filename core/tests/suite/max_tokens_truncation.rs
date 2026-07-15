use ody_model_provider_info::ModelProviderInfo;
use ody_model_provider_info::ProviderCapabilities;
use ody_model_provider_info::WireApi;
use ody_protocol::protocol::EventMsg;
use ody_protocol::protocol::Op;
use ody_protocol::user_input::UserInput;
use core_test_support::skip_if_no_network;
use core_test_support::test_ody::TestOdy;
use core_test_support::test_ody::test_ody;
use core_test_support::wait_for_event;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

/// Chat Completions SSE body whose terminal content chunk carries
/// `finish_reason`, followed by a usage chunk (production-realistic: ody-rs
/// always requests `stream_options.include_usage`).
fn chat_sse_with_finish_reason(finish_reason: &str) -> String {
    format!(
        "data: {{\"id\":\"resp_1\",\"choices\":[{{\"delta\":{{\"role\":\"assistant\",\"content\":\"partial answer\"}}}}]}}\n\n\
         data: {{\"id\":\"resp_1\",\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"{finish_reason}\"}}]}}\n\n\
         data: {{\"id\":\"resp_1\",\"choices\":[],\"usage\":{{\"prompt_tokens\":10,\"completion_tokens\":5,\"total_tokens\":15}}}}\n\n\
         data: [DONE]\n\n"
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn max_tokens_truncation_emits_warning_and_ends_turn() {
    skip_if_no_network!();

    let server = MockServer::start().await;

    let sse =
        ResponseTemplate::new(200)
            .set_body_raw(chat_sse_with_finish_reason("length"), "text/event-stream");
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse)
        .mount(&server)
        .await;

    // Explicit Chat-wire provider pointed at the mock server. `PATH` satisfies
    // the auth plumbing without a real secret (same trick as
    // stream_error_allows_next_turn.rs).
    let provider = ModelProviderInfo {
        name: "mock-chat".into(),
        base_url: Some(format!("{}/v1", server.uri())),
        env_key: Some("PATH".into()),
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        wire_api: WireApi::Chat,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: Some(1),
        stream_max_retries: Some(1),
        stream_idle_timeout_ms: Some(2_000),
        websocket_connect_timeout_ms: None,
        supports_websockets: false,
        capabilities: ProviderCapabilities::default(),
    };

    let TestOdy { ody, .. } = test_ody()
        .with_config(move |config| {
            config.model_provider = provider;
        })
        .build(&server)
        .await
        .unwrap();

    ody.submit(Op::UserInput {
        items: vec![UserInput::Text {
            text: "hello".into(),
            text_elements: Vec::new(),
        }],
        final_output_json_schema: None,
        responsesapi_client_metadata: None,
        additional_context: Default::default(),
        thread_settings: Default::default(),
    })
    .await
    .unwrap();

    // 1) A Warning event surfaces the truncation with the raw reason.
    let warning = wait_for_event(&ody, |ev| matches!(ev, EventMsg::Warning(_))).await;
    let EventMsg::Warning(warning) = warning else {
        panic!("expected warning event");
    };
    assert!(
        warning.message.contains("length"),
        "warning should name the raw finish_reason, got: {}",
        warning.message
    );

    // 2) The turn still completes normally (no auto-follow-up re-prompt).
    wait_for_event(&ody, |ev| matches!(ev, EventMsg::TurnComplete(_))).await;

    // 3) Exactly one model request was made — proves needs_follow_up stayed
    //    false (a follow-up would have issued a second request).
    let requests = server
        .received_requests()
        .await
        .expect("requests recorded");
    let model_requests = requests
        .iter()
        .filter(|req| req.url.path() == "/v1/chat/completions")
        .count();
    assert_eq!(
        model_requests, 1,
        "truncated turn must not auto-follow-up with another model request"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn normal_stop_finish_reason_emits_no_warning() {
    skip_if_no_network!();

    let server = MockServer::start().await;

    let sse =
        ResponseTemplate::new(200)
            .set_body_raw(chat_sse_with_finish_reason("stop"), "text/event-stream");
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse)
        .mount(&server)
        .await;

    let provider = ModelProviderInfo {
        name: "mock-chat".into(),
        base_url: Some(format!("{}/v1", server.uri())),
        env_key: Some("PATH".into()),
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        wire_api: WireApi::Chat,
        query_params: None,
        http_headers: None,
        env_http_headers: None,
        request_max_retries: Some(1),
        stream_max_retries: Some(1),
        stream_idle_timeout_ms: Some(2_000),
        websocket_connect_timeout_ms: None,
        supports_websockets: false,
        capabilities: ProviderCapabilities::default(),
    };

    let TestOdy { ody, .. } = test_ody()
        .with_config(move |config| {
            config.model_provider = provider;
        })
        .build(&server)
        .await
        .unwrap();

    ody.submit(Op::UserInput {
        items: vec![UserInput::Text {
            text: "hello".into(),
            text_elements: Vec::new(),
        }],
        final_output_json_schema: None,
        responsesapi_client_metadata: None,
        additional_context: Default::default(),
        thread_settings: Default::default(),
    })
    .await
    .unwrap();

    // A normal stop completes the turn without any truncation warning. The
    // predicate asserts absence WHILE draining: wait_for_event consumes and
    // discards non-matching events, so a separate post-hoc wait could never
    // observe a warning emitted before TurnComplete.
    wait_for_event(&ody, |ev| {
        assert!(
            !matches!(ev, EventMsg::Warning(w) if w.message.contains("finish_reason")),
            "normal stop must not emit a truncation warning"
        );
        matches!(ev, EventMsg::TurnComplete(_))
    })
    .await;
}
