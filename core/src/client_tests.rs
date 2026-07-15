use super::ModelClient;
use super::X_ODY_INSTALLATION_ID_HEADER;
use super::X_ODY_PARENT_THREAD_ID_HEADER;
use super::X_ODY_TURN_METADATA_HEADER;
use super::X_ODY_WINDOW_ID_HEADER;
use super::X_OPENAI_SUBAGENT_HEADER;
use crate::AttestationContext;
use crate::AttestationProvider;
use crate::GenerateAttestationFuture;
use crate::responses_metadata::OdyResponsesMetadata;
use crate::test_support::TestOdyResponsesRequestKind;
use crate::test_support::responses_metadata as test_responses_metadata;
use ody_api::ApiError;
use ody_api::ResponseEvent;
use ody_model_provider::SharedModelProvider;
use ody_model_provider_info::ModelProviderInfo;
use ody_model_provider_info::WireApi;
use ody_model_provider_info::create_kimi_provider;
use ody_otel::SessionTelemetry;
use ody_protocol::ThreadId;
use ody_protocol::models::ContentItem;
use ody_protocol::models::ResponseItem;
use ody_protocol::model_metadata::ModelInfo;
use ody_protocol::model_metadata::ReasoningEffort;
use ody_protocol::model_metadata::ModelCapabilities;
use ody_protocol::protocol::InternalSessionSource;
use ody_protocol::protocol::SessionSource;
use ody_protocol::protocol::SubAgentSource;
use ody_rollout_trace::ExecutionStatus;
use ody_rollout_trace::InferenceTraceAttempt;
use ody_rollout_trace::InferenceTraceContext;
use ody_rollout_trace::RawTraceEventPayload;
use ody_rollout_trace::RolloutTrace;
use ody_rollout_trace::TraceWriter;
use ody_rollout_trace::replay_bundle;
use futures::StreamExt;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::task::Context;
use std::task::Poll;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::Notify;
use tracing::Event;
use tracing::Subscriber;
use tracing::field::Visit;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context as LayerContext;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;

use ody_protocol::config_types::ReasoningSummary;
const TEST_INSTALLATION_ID: &str = "11111111-1111-4111-8111-111111111111";

fn test_model_client(session_source: SessionSource) -> ModelClient {
    let provider = create_kimi_provider();
    let thread_id = ThreadId::new();
    ModelClient::new(
        thread_id,
        provider,
        session_source,
        /*model_verbosity*/ None,
        /*enable_request_compression*/ false,
        /*include_timing_metrics*/ false,
        /*beta_features_header*/ None,
        /*item_ids_enabled*/ false,
        /*attestation_provider*/ None,
    )
}

fn test_model_provider() -> SharedModelProvider {
    test_model_client(SessionSource::Cli).state.provider.clone()
}

fn test_responses_metadata_for_client(
    client: &ModelClient,
    turn_id: Option<&str>,
    window_id: String,
    parent_thread_id: Option<ThreadId>,
    request_kind: TestOdyResponsesRequestKind,
) -> OdyResponsesMetadata {
    let thread_id = client.state.thread_id.to_string();
    test_responses_metadata(
        TEST_INSTALLATION_ID,
        &thread_id,
        &thread_id,
        turn_id,
        window_id,
        &client.state.session_source,
        parent_thread_id,
        request_kind,
    )
}

fn test_model_info() -> ModelInfo {
    serde_json::from_value(json!({
        "slug": "gpt-test",
        "display_name": "gpt-test",
        "description": "desc",
        "default_reasoning_level": "medium",
        "supported_reasoning_levels": [
            {"effort": "medium", "description": "medium"}
        ],
        "shell_type": "shell_command",
        "visibility": "list",
        "supported_in_api": true,
        "priority": 1,
        "upgrade": null,
        "base_instructions": "base instructions",
        "model_messages": null,
        "supports_reasoning_summaries": false,
        "support_verbosity": false,
        "default_verbosity": null,
        "truncation_policy": {"mode": "bytes", "limit": 10000},
        "supports_parallel_tool_calls": false,
        "supports_image_detail_original": false,
        "context_window": 272000,
        "auto_compact_token_limit": null,
        "experimental_supported_tools": []
    }))
    .expect("deserialize test model info")
}

fn test_session_telemetry() -> SessionTelemetry {
    SessionTelemetry::new(
        ThreadId::new(),
        "gpt-test",
        "gpt-test",
        /*auth_mode*/ None,
        "test-originator".to_string(),
        /*log_user_prompts*/ false,
        "test-terminal".to_string(),
        SessionSource::Cli,
    )
}

#[test]
fn ultra_reasoning_uses_max_for_requests() {
    assert_eq!(
        (
            super::reasoning_effort_for_request(ReasoningEffort::Ultra),
            super::reasoning_effort_for_request(ReasoningEffort::High),
        ),
        (
            ReasoningEffort::Custom("max".to_string()),
            ReasoningEffort::High,
        )
    );
}

#[derive(Default)]
struct TagCollectorVisitor {
    tags: BTreeMap<String, String>,
}

impl Visit for TagCollectorVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.tags
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.tags
            .insert(field.name().to_string(), format!("{value:?}"));
    }
}

#[derive(Clone)]
struct TagCollectorLayer {
    tags: Arc<Mutex<BTreeMap<String, String>>>,
}

impl<S> Layer<S> for TagCollectorLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &Event<'_>, _ctx: LayerContext<'_, S>) {
        if event.metadata().target() != "feedback_tags" {
            return;
        }
        let mut visitor = TagCollectorVisitor::default();
        event.record(&mut visitor);
        self.tags.lock().unwrap().extend(visitor.tags);
    }
}

fn started_inference_attempt(temp: &TempDir) -> anyhow::Result<InferenceTraceAttempt> {
    let writer = Arc::new(TraceWriter::create(
        temp.path(),
        "trace-1".to_string(),
        "rollout-1".to_string(),
        "thread-root".to_string(),
    )?);
    writer.append(RawTraceEventPayload::ThreadStarted {
        thread_id: "thread-root".to_string(),
        agent_path: "/root".to_string(),
        metadata_payload: None,
    })?;
    writer.append(RawTraceEventPayload::OdyTurnStarted {
        ody_turn_id: "turn-1".to_string(),
        thread_id: "thread-root".to_string(),
    })?;

    let inference_trace = InferenceTraceContext::enabled(
        writer,
        "thread-root".to_string(),
        "turn-1".to_string(),
        "gpt-test".to_string(),
        "test-provider".to_string(),
    );
    let attempt = inference_trace.start_attempt();
    attempt.record_started(&json!({
        "model": "gpt-test",
        "input": [{
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": "hello"}]
        }],
    }));
    Ok(attempt)
}

fn output_message(id: &str, text: &str) -> ResponseItem {
    ResponseItem::Message {
        id: Some(id.to_string()),
        role: "assistant".to_string(),
        content: vec![ContentItem::OutputText {
            text: text.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

async fn replay_until_cancelled(temp: &TempDir) -> anyhow::Result<RolloutTrace> {
    let mut rollout = replay_bundle(temp.path())?;
    for _ in 0..50 {
        let inference = rollout
            .inference_calls
            .values()
            .next()
            .expect("inference should be reduced");
        if inference.execution.status == ExecutionStatus::Cancelled {
            return Ok(rollout);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
        rollout = replay_bundle(temp.path())?;
    }
    Ok(rollout)
}

struct NotifyAfterEventStream {
    events: VecDeque<ResponseEvent>,
    yielded: usize,
    notify_after: usize,
    notify: Arc<Notify>,
}

impl futures::Stream for NotifyAfterEventStream {
    type Item = std::result::Result<ResponseEvent, ApiError>;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let Some(event) = self.events.pop_front() else {
            return Poll::Pending;
        };
        self.yielded += 1;
        if self.yielded == self.notify_after {
            self.notify.notify_one();
        }
        Poll::Ready(Some(Ok(event)))
    }
}

#[test]
fn build_subagent_headers_sets_other_subagent_label() {
    let client = test_model_client(SessionSource::SubAgent(SubAgentSource::Other(
        "memory_consolidation".to_string(),
    )));
    let headers = client.build_subagent_headers();
    let value = headers
        .get(X_OPENAI_SUBAGENT_HEADER)
        .and_then(|value| value.to_str().ok());
    assert_eq!(value, Some("memory_consolidation"));
}

#[test]
fn build_subagent_headers_sets_internal_memory_consolidation_label() {
    let client = test_model_client(SessionSource::Internal(
        InternalSessionSource::MemoryConsolidation,
    ));
    let headers = client.build_subagent_headers();
    let value = headers
        .get(X_OPENAI_SUBAGENT_HEADER)
        .and_then(|value| value.to_str().ok());
    assert_eq!(value, Some("memory_consolidation"));
}

#[test]
fn build_ws_client_metadata_includes_window_lineage_and_turn_metadata() {
    let parent_thread_id = ThreadId::new();
    let client = test_model_client(SessionSource::SubAgent(SubAgentSource::ThreadSpawn {
        parent_thread_id,
        depth: 2,
        agent_path: None,
        agent_nickname: None,
        agent_role: None,
    }));

    let thread_id = client.state.thread_id.to_string();
    let expected_window_id = format!("{thread_id}:1");
    let responses_metadata = test_responses_metadata_for_client(
        &client,
        Some("turn-123"),
        expected_window_id.clone(),
        Some(parent_thread_id),
        TestOdyResponsesRequestKind::Turn,
    );
    let client_metadata =
        client.build_ws_client_metadata(&responses_metadata, /*use_responses_lite*/ false);
    let parent_thread_id = parent_thread_id.to_string();
    let turn_metadata: serde_json::Value = serde_json::from_str(
        client_metadata
            .get(X_ODY_TURN_METADATA_HEADER)
            .expect("turn metadata"),
    )
    .expect("valid turn metadata");
    for (client_key, metadata_key, expected) in [
        (
            X_ODY_INSTALLATION_ID_HEADER,
            "installation_id",
            "11111111-1111-4111-8111-111111111111",
        ),
        ("session_id", "session_id", thread_id.as_str()),
        ("thread_id", "thread_id", thread_id.as_str()),
        ("turn_id", "turn_id", "turn-123"),
        (
            X_ODY_WINDOW_ID_HEADER,
            "window_id",
            expected_window_id.as_str(),
        ),
        (
            X_ODY_PARENT_THREAD_ID_HEADER,
            "parent_thread_id",
            parent_thread_id.as_str(),
        ),
    ] {
        assert_eq!(
            client_metadata.get(client_key).map(String::as_str),
            Some(expected)
        );
        assert_eq!(turn_metadata[metadata_key].as_str(), Some(expected));
    }
    assert_eq!(
        client_metadata
            .get(X_OPENAI_SUBAGENT_HEADER)
            .map(String::as_str),
        Some("collab_spawn")
    );
}

#[tokio::test]
async fn summarize_memories_returns_empty_for_empty_input() {
    let client = test_model_client(SessionSource::Cli);
    let model_info = test_model_info();
    let session_telemetry = test_session_telemetry();

    let output = client
        .summarize_memories(
            Vec::new(),
            &model_info,
            /*effort*/ None,
            &session_telemetry,
        )
        .await
        .expect("empty summarize request should succeed");
    assert_eq!(output.len(), 0);
}

#[tokio::test]
async fn dropped_response_stream_traces_cancelled_partial_output() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let attempt = started_inference_attempt(&temp)?;

    // The provider has produced one complete output item, but no terminal
    // response.completed event. The harness has enough information to keep this
    // item in history, so the trace should preserve it when the stream is
    // abandoned.
    let item = output_message("msg-1", "partial answer");
    let api_stream = futures::stream::iter([Ok(ResponseEvent::OutputItemDone(item))])
        .chain(futures::stream::pending());
    let (mut stream, _) = super::map_response_events(
        /*upstream_request_id*/ None,
        api_stream,
        test_session_telemetry(),
        attempt,
        test_model_provider(),
    );

    let observed = stream
        .next()
        .await
        .expect("mapped stream should yield output item")?;
    assert!(matches!(observed, ResponseEvent::OutputItemDone(_)));

    // Dropping the consumer is how turn interruption/preemption stops polling
    // the provider stream. The mapper task observes that drop asynchronously
    // and records cancellation using the output items it has already seen.
    drop(stream);

    // Cancellation is recorded by the mapper task after Drop wakes it, so the
    // replay may need a short wait before the terminal event appears on disk.
    let rollout = replay_until_cancelled(&temp).await?;
    let inference = rollout
        .inference_calls
        .values()
        .next()
        .expect("inference should be reduced");

    assert_eq!(inference.execution.status, ExecutionStatus::Cancelled);
    assert_eq!(inference.response_item_ids.len(), 1);
    assert_eq!(rollout.raw_payloads.len(), 2);

    Ok(())
}

#[tokio::test]
async fn response_stream_records_last_model_feedback_ids() {
    let tags = Arc::new(Mutex::new(BTreeMap::new()));
    let _guard = tracing_subscriber::registry()
        .with(TagCollectorLayer { tags: tags.clone() })
        .set_default();

    let api_stream = futures::stream::iter([
        Ok(ResponseEvent::Created),
        Ok(ResponseEvent::Completed {
            response_id: "resp-123".to_string(),
            token_usage: None,
            end_turn: Some(true),
            finish_reason: None,
        }),
    ]);
    let (mut stream, _) = super::map_response_events(
        Some("req-123".to_string()),
        api_stream,
        test_session_telemetry(),
        InferenceTraceAttempt::disabled(),
        test_model_provider(),
    );

    while stream.next().await.is_some() {}

    let tags = tags.lock().unwrap().clone();
    assert_eq!(
        tags.get("last_model_request_id").map(String::as_str),
        Some("\"req-123\"")
    );
    assert_eq!(
        tags.get("last_model_response_id").map(String::as_str),
        Some("\"resp-123\"")
    );
}

#[tokio::test]
async fn dropped_backpressured_response_stream_traces_cancelled_partial_output()
-> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let attempt = started_inference_attempt(&temp)?;
    let backpressured_item_yielded = Arc::new(Notify::new());
    let mut events = VecDeque::new();
    for _ in 0..super::RESPONSE_STREAM_CHANNEL_CAPACITY {
        events.push_back(ResponseEvent::Created);
    }
    events.push_back(ResponseEvent::OutputItemDone(output_message(
        "msg-1",
        "partial answer",
    )));
    let api_stream = NotifyAfterEventStream {
        events,
        yielded: 0,
        notify_after: super::RESPONSE_STREAM_CHANNEL_CAPACITY + 1,
        notify: Arc::clone(&backpressured_item_yielded),
    };

    let (stream, _) = super::map_response_events(
        /*upstream_request_id*/ None,
        api_stream,
        test_session_telemetry(),
        attempt,
        test_model_provider(),
    );

    // Fill the mapper channel with non-terminal events, then yield one output
    // item. The mapper has observed that item and is blocked trying to send it
    // downstream, so dropping the consumer covers the send-failure path rather
    // than the `consumer_dropped` select branch.
    backpressured_item_yielded.notified().await;
    drop(stream);

    let rollout = replay_until_cancelled(&temp).await?;
    let inference = rollout
        .inference_calls
        .values()
        .next()
        .expect("inference should be reduced");

    assert_eq!(inference.execution.status, ExecutionStatus::Cancelled);
    assert_eq!(inference.response_item_ids.len(), 1);
    assert_eq!(rollout.raw_payloads.len(), 2);

    Ok(())
}

fn model_client_with_counting_attestation(
    odysseythink_provider: bool,
) -> (ModelClient, Arc<AtomicUsize>) {
    #[derive(Debug)]
    struct CountingAttestationProvider {
        calls: Arc<AtomicUsize>,
    }

    impl AttestationProvider for CountingAttestationProvider {
        fn header_for_request(
            &self,
            _context: AttestationContext,
        ) -> GenerateAttestationFuture<'_> {
            let calls = self.calls.clone();
            Box::pin(async move {
                let call = calls.fetch_add(1, Ordering::Relaxed) + 1;
                Some(http::HeaderValue::from_bytes(format!("v1.header-{call}").as_bytes()).unwrap())
            })
        }
    }

    let attestation_calls = Arc::new(AtomicUsize::new(0));
    let provider = if odysseythink_provider {
        ModelProviderInfo {
            name: "OpenAI".into(),
            base_url: Some("https://api.odysseythink.com/v1".to_string()),
            env_key: None,
            env_key_instructions: None,
            experimental_bearer_token: None,
            auth: None,
            wire_api: ody_model_provider_info::WireApi::Responses,
            query_params: None,
            http_headers: Some(
                [("version".to_string(), env!("CARGO_PKG_VERSION").to_string())]
                    .into_iter()
                    .collect(),
            ),
            env_http_headers: None,
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            websocket_connect_timeout_ms: None,
            supports_websockets: true,
            capabilities: ody_model_provider_info::ProviderCapabilities {
                supports_websockets: true,
                supports_remote_compaction: true,
                namespace_tools: true,
                image_generation: true,
                web_search: true,
                command_auth: false,
                attestation: false,
            },
        }
    } else {
        create_kimi_provider()
    };
    let model_client = ModelClient::new(
        ThreadId::new(),
        provider,
        SessionSource::Exec,
        /*model_verbosity*/ None,
        /*enable_request_compression*/ false,
        /*include_timing_metrics*/ false,
        /*beta_features_header*/ None,
        /*item_ids_enabled*/ false,
        Some(Arc::new(CountingAttestationProvider {
            calls: attestation_calls.clone(),
        })),
    );
    (model_client, attestation_calls)
}

#[tokio::test]
async fn websocket_handshake_omits_attestation_for_odysseythink_responses() {
    let (model_client, attestation_calls) =
        model_client_with_counting_attestation(/*odysseythink_provider*/ true);
    let responses_metadata = test_responses_metadata_for_client(
        &model_client,
        /*turn_id*/ None,
        format!("{}:0", model_client.state.thread_id),
        /*parent_thread_id*/ None,
        TestOdyResponsesRequestKind::WebsocketConnection,
    );

    let headers = model_client
        .build_websocket_headers(&responses_metadata)
        .await;

    assert_eq!(
        headers
            .get(crate::attestation::X_OAI_ATTESTATION_HEADER)
            .and_then(|value| value.to_str().ok()),
        None,
    );
    assert_eq!(attestation_calls.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn non_odysseythink_endpoints_omit_attestation_generation() {
    let (model_client, attestation_calls) =
        model_client_with_counting_attestation(/*odysseythink_provider*/ false);
    let mut response_headers = http::HeaderMap::new();

    if let Some(header_value) = model_client.generate_attestation_header_for().await {
        response_headers.insert(crate::attestation::X_OAI_ATTESTATION_HEADER, header_value);
    }
    let mut compaction_headers = http::HeaderMap::new();
    if let Some(header_value) = model_client.generate_attestation_header_for().await {
        compaction_headers.insert(crate::attestation::X_OAI_ATTESTATION_HEADER, header_value);
    }
    let mut realtime_headers = http::HeaderMap::new();
    if let Some(header_value) = model_client.generate_attestation_header_for().await {
        realtime_headers.insert(crate::attestation::X_OAI_ATTESTATION_HEADER, header_value);
    }

    assert_eq!(
        response_headers.get(crate::attestation::X_OAI_ATTESTATION_HEADER),
        None,
    );
    assert_eq!(
        compaction_headers.get(crate::attestation::X_OAI_ATTESTATION_HEADER),
        None,
    );
    assert_eq!(
        realtime_headers.get(crate::attestation::X_OAI_ATTESTATION_HEADER),
        None,
    );
    assert_eq!(attestation_calls.load(Ordering::Relaxed), 0);
}
#[tokio::test]
async fn chat_stream_maps_events_to_response_stream() {
    use futures::stream;
    use ody_model_provider::{ChatEvent, ContentPart, FinishReason};

    let chat_stream = stream::iter(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::ContentPart(ContentPart::Text("hello".into()))),
        Ok(ChatEvent::Finish {
            reason: FinishReason::Stop,
            raw_reason: None,
        }),
    ]);
    let stream = crate::client::map_chat_stream(
        Box::pin(chat_stream),
        create_test_session_telemetry(),
        InferenceTraceContext::disabled().start_attempt(),
    );

    let mut saw_created = false;
    let mut saw_output_delta = false;
    let mut saw_completed = false;
    futures::pin_mut!(stream);
    while let Some(event) = stream.next().await {
        match event {
            Ok(ResponseEvent::Created) => saw_created = true,
            Ok(ResponseEvent::OutputTextDelta(_)) => saw_output_delta = true,
            Ok(ResponseEvent::Completed { .. }) => saw_completed = true,
            _ => {}
        }
    }
    assert!(saw_created, "Expected Created event");
    assert!(saw_output_delta, "Expected OutputTextDelta event");
    assert!(saw_completed, "Expected Completed event");
}

#[tokio::test]
async fn chat_stream_maps_max_tokens_finish_to_completed_end_turn_true() {
    use futures::stream;
    use ody_model_provider::{ChatEvent, ContentPart, FinishReason};

    let chat_stream = stream::iter(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::ContentPart(ContentPart::Text("partial".into()))),
        Ok(ChatEvent::Finish {
            reason: FinishReason::MaxTokens,
            raw_reason: Some("length".into()),
        }),
    ]);
    let stream = crate::client::map_chat_stream(
        Box::pin(chat_stream),
        create_test_session_telemetry(),
        InferenceTraceContext::disabled().start_attempt(),
    );

    futures::pin_mut!(stream);
    let mut completed = None;
    while let Some(event) = stream.next().await {
        if matches!(event, Ok(ResponseEvent::Completed { .. })) {
            completed = Some(event.expect("completed event is Ok"));
        }
    }
    match completed {
        Some(ResponseEvent::Completed {
            end_turn,
            finish_reason,
            ..
        }) => {
            assert_eq!(
                end_turn,
                Some(true),
                "MaxTokens must still end the turn (Some(false) would trigger auto-follow-up)"
            );
            assert_eq!(
                finish_reason.as_deref(),
                Some("length"),
                "raw finish_reason must be passed through"
            );
        }
        other => panic!("expected Completed, got {other:?}"),
    }
}

#[tokio::test]
async fn chat_stream_maps_non_turn_ending_finishes_to_completed_end_turn_false() {
    use futures::stream;
    use ody_model_provider::{ChatEvent, ContentPart, FinishReason};

    // Every `FinishReason` variant outside {Stop, MaxTokens} must keep the
    // paused-turn semantics (`end_turn == Some(false)`).
    let cases: Vec<(FinishReason, Option<String>)> = vec![
        (FinishReason::ToolCalls, Some("tool_calls".into())),
        (FinishReason::ContentFilter, Some("content_filter".into())),
        (FinishReason::PauseTurn, Some("pause_turn".into())),
        (
            FinishReason::Other("incomplete".into()),
            Some("incomplete".into()),
        ),
    ];
    for (reason, raw_reason) in cases {
        let chat_stream = stream::iter(vec![
            Ok(ChatEvent::Start),
            Ok(ChatEvent::ContentPart(ContentPart::Text("paused".into()))),
            Ok(ChatEvent::Finish {
                reason: reason.clone(),
                raw_reason,
            }),
        ]);
        let stream = crate::client::map_chat_stream(
            Box::pin(chat_stream),
            create_test_session_telemetry(),
            InferenceTraceContext::disabled().start_attempt(),
        );

        futures::pin_mut!(stream);
        let mut completed = None;
        while let Some(event) = stream.next().await {
            if matches!(event, Ok(ResponseEvent::Completed { .. })) {
                completed = Some(event.expect("completed event is Ok"));
            }
        }
        match completed {
            Some(ResponseEvent::Completed { end_turn, .. }) => {
                assert_eq!(
                    end_turn,
                    Some(false),
                    "{reason:?} must keep the paused-turn semantics"
                );
            }
            other => panic!("expected Completed for {reason:?}, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn chat_stream_folds_usage_into_finish_completed() {
    use futures::stream;
    use ody_model_provider::{ChatEvent, ContentPart, FinishReason, Usage};

    let chat_stream = stream::iter(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::ContentPart(ContentPart::Text("partial".into()))),
        Ok(ChatEvent::Usage(Usage {
            input_tokens: 10,
            output_tokens: 5,
            reasoning_tokens: None,
        })),
        Ok(ChatEvent::Finish {
            reason: FinishReason::MaxTokens,
            raw_reason: Some("length".into()),
        }),
    ]);
    let stream = crate::client::map_chat_stream(
        Box::pin(chat_stream),
        create_test_session_telemetry(),
        InferenceTraceContext::disabled().start_attempt(),
    );

    futures::pin_mut!(stream);
    let mut completed_events = Vec::new();
    while let Some(event) = stream.next().await {
        if let Ok(ResponseEvent::Completed { .. }) = event {
            completed_events.push(event.expect("completed event is Ok"));
        }
    }
    assert_eq!(
        completed_events.len(),
        1,
        "usage must be folded into the Finish completed, not short-circuit it"
    );
    match &completed_events[0] {
        ResponseEvent::Completed {
            token_usage,
            end_turn,
            finish_reason,
            ..
        } => {
            let usage = token_usage.as_ref().expect("usage folded into completed");
            assert_eq!(usage.input_tokens, 10);
            assert_eq!(usage.output_tokens, 5);
            assert_eq!(*end_turn, Some(true));
            assert_eq!(finish_reason.as_deref(), Some("length"));
        }
        other => panic!("expected Completed, got {other:?}"),
    }
}

#[tokio::test]
async fn chat_stream_flushes_buffered_usage_when_stream_ends_without_finish() {
    use futures::stream;
    use ody_model_provider::{ChatEvent, Usage};

    let chat_stream = stream::iter(vec![
        Ok(ChatEvent::Start),
        Ok(ChatEvent::Usage(Usage {
            input_tokens: 10,
            output_tokens: 5,
            reasoning_tokens: None,
        })),
    ]);
    let stream = crate::client::map_chat_stream(
        Box::pin(chat_stream),
        create_test_session_telemetry(),
        InferenceTraceContext::disabled().start_attempt(),
    );

    futures::pin_mut!(stream);
    let mut completed_events = Vec::new();
    while let Some(event) = stream.next().await {
        if let Ok(ResponseEvent::Completed { .. }) = event {
            completed_events.push(event.expect("completed event is Ok"));
        }
    }
    assert_eq!(
        completed_events.len(),
        1,
        "buffered usage must still close the turn when the stream ends without Finish"
    );
    match &completed_events[0] {
        ResponseEvent::Completed {
            token_usage,
            end_turn,
            finish_reason,
            ..
        } => {
            let usage = token_usage.as_ref().expect("usage folded into completed");
            assert_eq!(usage.input_tokens, 10);
            assert_eq!(usage.output_tokens, 5);
            assert_eq!(*end_turn, Some(true));
            assert_eq!(finish_reason.as_deref(), None);
        }
        other => panic!("expected Completed, got {other:?}"),
    }
}

fn test_model_client_with_provider(provider: ModelProviderInfo) -> ModelClient {
    let thread_id = ThreadId::new();
    ModelClient::new(
        thread_id,
        provider,
        SessionSource::Cli,
        /*model_verbosity*/ None,
        /*enable_request_compression*/ false,
        /*include_timing_metrics*/ false,
        /*beta_features_header*/ None,
        /*item_ids_enabled*/ false,
        /*attestation_provider*/ None,
    )
}

fn create_test_session_telemetry() -> SessionTelemetry {
    SessionTelemetry::new(
      ThreadId::new(),
      "test-model",
      "test-model",
      None,
      "test-originator".to_string(),
      false,
      "test-terminal".to_string(),
      SessionSource::Cli,
  )
}

fn test_prompt_for_chat_provider() -> crate::client_common::Prompt {
    crate::client_common::Prompt {
        input: vec![ResponseItem::Message {
            id: None,
            role: "user".into(),
            content: vec![ContentItem::InputText { text: "hello".into() }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        }],
        tools: Vec::new(),
        parallel_tool_calls: false,
        base_instructions: Default::default(),
        output_schema: None,
        output_schema_strict: true,
    }
}

fn test_model_info_for_chat_provider() -> ModelInfo {
    ModelInfo {
        slug: "test-model".into(),
        display_name: "Test Model".into(),
        description: None,
        default_reasoning_level: None,
        supported_reasoning_levels: Vec::new(),
        shell_type: ody_protocol::model_metadata::ConfigShellToolType::Default,
        visibility: ody_protocol::model_metadata::ModelVisibility::List,
        supported_in_api: true,
        priority: 0,
        additional_speed_tiers: Vec::new(),
        service_tiers: Vec::new(),
        default_service_tier: None,
        availability_nux: None,
        upgrade: None,
        base_instructions: "You are a helpful assistant.".into(),
        model_messages: None,
        supports_reasoning_summaries: false,
        default_reasoning_summary: ody_protocol::config_types::ReasoningSummary::None,
        support_verbosity: false,
        default_verbosity: None,
        web_search_tool_type: ody_protocol::model_metadata::WebSearchToolType::Text,
        truncation_policy: ody_protocol::model_metadata::TruncationPolicyConfig::bytes(1_000_000),
        supports_parallel_tool_calls: false,
        supports_image_detail_original: false,
        context_window: None,
        max_context_window: None,
        auto_compact_token_limit: None,
        comp_hash: None,
        effective_context_window_percent: 90,
        experimental_supported_tools: Vec::new(),
        input_modalities: Vec::new(),
        used_fallback_model_metadata: false,
        supports_search_tool: false,
        use_responses_lite: false,
        auto_review_model_override: None,
        tool_mode: None,
        multi_agent_version: None,
        capabilities: ModelCapabilities::default(),
    }
}
