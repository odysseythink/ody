use super::AnalyticsEventsClient;
use super::AnalyticsEventsDestination;
use super::AnalyticsEventsQueue;
#[cfg(debug_assertions)]
use super::capture_track_events_request;
#[cfg(debug_assertions)]
use super::send_track_events_request;
use super::track_event_request_batches;
use crate::events::OdyAcceptedLineFingerprintsEventParams;
use crate::events::OdyAcceptedLineFingerprintsEventRequest;
use crate::events::SkillInvocationEventParams;
use crate::events::SkillInvocationEventRequest;
use crate::events::TrackEventRequest;
use crate::facts::AnalyticsFact;
use crate::facts::CustomAnalyticsFact;
use crate::facts::DesignReviewCompletedInput;
use crate::facts::DesignReviewFailedInput;
use crate::facts::DesignReviewFailureReason;
use crate::facts::DesignReviewStartedInput;
use crate::facts::InvocationType;
use ody_app_server_protocol::ApprovalsReviewer as AppServerApprovalsReviewer;
use ody_app_server_protocol::AskForApproval as AppServerAskForApproval;
use ody_app_server_protocol::ClientRequest;
use ody_app_server_protocol::ClientResponsePayload;
use ody_app_server_protocol::RequestId;
use ody_app_server_protocol::SandboxPolicy as AppServerSandboxPolicy;
use ody_app_server_protocol::SessionSource as AppServerSessionSource;
use ody_app_server_protocol::Thread;
use ody_app_server_protocol::ThreadArchiveParams;
use ody_app_server_protocol::ThreadArchiveResponse;
use ody_app_server_protocol::ThreadForkResponse;
use ody_app_server_protocol::ThreadResumeResponse;
use ody_app_server_protocol::ThreadStartResponse;
use ody_app_server_protocol::ThreadStatus as AppServerThreadStatus;
use ody_app_server_protocol::Turn;
use ody_app_server_protocol::TurnStartParams;
use ody_app_server_protocol::TurnStartResponse;
use ody_app_server_protocol::TurnStatus as AppServerTurnStatus;
use ody_app_server_protocol::TurnSteerParams;
use ody_app_server_protocol::TurnSteerResponse;
use ody_utils_absolute_path::test_support::PathBufExt;
use ody_utils_absolute_path::test_support::test_path_buf;
use std::collections::HashSet;
#[cfg(debug_assertions)]
use std::fs;
#[cfg(debug_assertions)]
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
#[cfg(debug_assertions)]
use std::time::SystemTime;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;

fn sample_accepted_line_fingerprint_event(thread_id: &str) -> TrackEventRequest {
    TrackEventRequest::AcceptedLineFingerprints(Box::new(OdyAcceptedLineFingerprintsEventRequest {
        event_type: "ody_accepted_line_fingerprints",
        event_params: OdyAcceptedLineFingerprintsEventParams {
            event_type: "ody.accepted_line_fingerprints",
            turn_id: "turn-1".to_string(),
            thread_id: thread_id.to_string(),
            product_surface: Some("ody".to_string()),
            model_slug: Some("kimi-for-coding".to_string()),
            completed_at: 1,
            repo_hash: None,
            accepted_added_lines: 1,
            accepted_deleted_lines: 0,
            line_fingerprints: Vec::new(),
        },
    }))
}

fn sample_regular_track_event(thread_id: &str) -> TrackEventRequest {
    TrackEventRequest::SkillInvocation(SkillInvocationEventRequest {
        event_type: "skill_invocation",
        skill_id: format!("skill-{thread_id}"),
        skill_name: "doc".to_string(),
        event_params: SkillInvocationEventParams {
            product_client_id: None,
            skill_scope: None,
            plugin_id: None,
            repo_url: None,
            thread_id: Some(thread_id.to_string()),
            turn_id: Some("turn-1".to_string()),
            invoke_type: Some(InvocationType::Explicit),
            model_slug: Some("kimi-for-coding".to_string()),
        },
    })
}

#[cfg(debug_assertions)]
fn unique_capture_path(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("system clock should be after Unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "ody-analytics-{name}-{}-{nonce}.jsonl",
        std::process::id()
    ))
}

fn client_with_receiver() -> (AnalyticsEventsClient, mpsc::Receiver<AnalyticsFact>) {
    let (sender, receiver) = mpsc::channel(8);
    let queue = AnalyticsEventsQueue {
        sender,
        app_used_emitted_keys: Arc::new(Mutex::new(HashSet::new())),
        plugin_used_emitted_keys: Arc::new(Mutex::new(HashSet::new())),
    };
    (AnalyticsEventsClient { queue: Some(queue) }, receiver)
}

#[test]
#[cfg(debug_assertions)]
fn analytics_destination_uses_explicit_capture_file() {
    let capture_path = unique_capture_path("destination");
    let destination = AnalyticsEventsDestination::from_base_url_and_capture_file(
        "https://api.odysseythink.com/v1/".to_string(),
        Some(capture_path.clone()),
    );

    assert_eq!(
        destination,
        AnalyticsEventsDestination::CaptureFile {
            path: capture_path.clone()
        }
    );
    assert_eq!(
        fs::read_to_string(&capture_path).expect("read capture file"),
        ""
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mode = fs::metadata(&capture_path)
            .expect("read capture file metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    fs::remove_file(capture_path).expect("remove capture file");
}

#[test]
fn analytics_destination_uses_http_without_capture_file() {
    let destination = AnalyticsEventsDestination::from_base_url_and_capture_file(
        "https://api.odysseythink.com/v1/".to_string(),
        /*capture_file*/ None,
    );

    assert_eq!(
        destination,
        AnalyticsEventsDestination::Http {
            url: "https://api.odysseythink.com/v1/ody/analytics-events/events".to_string()
        }
    );
}

#[test]
#[cfg(not(debug_assertions))]
fn analytics_destination_ignores_capture_file_in_release() {
    let destination = AnalyticsEventsDestination::from_base_url_and_capture_file(
        "https://api.odysseythink.com/v1/".to_string(),
        Some(std::path::PathBuf::from("ignored.jsonl")),
    );

    assert_eq!(
        destination,
        AnalyticsEventsDestination::Http {
            url: "https://api.odysseythink.com/v1/ody/analytics-events/events".to_string()
        }
    );
}

#[tokio::test]
#[cfg(debug_assertions)]
async fn capture_file_writes_exact_serialized_request() {
    let capture_path = unique_capture_path("single");
    let destination = AnalyticsEventsDestination::CaptureFile {
        path: capture_path.clone(),
    };
    let event = sample_regular_track_event("thread-1");
    let expected_event = serde_json::to_value(&event).expect("serialize expected event");
    send_track_events_request(&destination, vec![event]).await;

    let contents = fs::read_to_string(&capture_path).expect("read capture file");
    let lines = contents.lines().collect::<Vec<_>>();
    assert_eq!(lines.len(), 1);
    let payload: serde_json::Value =
        serde_json::from_str(lines[0]).expect("parse captured payload");
    assert_eq!(payload, serde_json::json!({"events": [expected_event]}));

    fs::remove_file(capture_path).expect("remove capture file");
}

#[tokio::test]
#[cfg(debug_assertions)]
async fn capture_file_writes_final_batches_as_separate_lines() {
    let capture_path = unique_capture_path("batches");
    let destination = AnalyticsEventsDestination::CaptureFile {
        path: capture_path.clone(),
    };
    let events = vec![
        sample_regular_track_event("thread-1"),
        sample_accepted_line_fingerprint_event("thread-2"),
        sample_regular_track_event("thread-3"),
    ];

    for batch in track_event_request_batches(events) {
        send_track_events_request(&destination, batch).await;
    }

    let contents = fs::read_to_string(&capture_path).expect("read capture file");
    let payloads = contents
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("parse capture line"))
        .collect::<Vec<_>>();
    assert_eq!(payloads.len(), 3);
    assert_eq!(payloads[0]["events"][0]["skill_id"], "skill-thread-1");
    assert_eq!(
        payloads[1]["events"][0]["event_type"],
        "ody_accepted_line_fingerprints"
    );
    assert_eq!(payloads[2]["events"][0]["skill_id"], "skill-thread-3");

    fs::remove_file(capture_path).expect("remove capture file");
}

#[test]
#[cfg(debug_assertions)]
fn capture_write_failure_still_consumes_delivery() {
    let capture_path = unique_capture_path("missing-parent").join("events.jsonl");
    let destination = AnalyticsEventsDestination::CaptureFile { path: capture_path };
    let payload = crate::events::TrackEventsRequest {
        events: vec![sample_regular_track_event("thread-1")],
    };

    assert!(capture_track_events_request(&destination, &payload));
}

fn sample_turn_start_request() -> ClientRequest {
    ClientRequest::TurnStart {
        request_id: RequestId::Integer(1),
        params: TurnStartParams {
            thread_id: "thread-1".to_string(),
            client_user_message_id: None,
            input: Vec::new(),
            ..Default::default()
        },
    }
}

fn sample_turn_steer_request() -> ClientRequest {
    ClientRequest::TurnSteer {
        request_id: RequestId::Integer(2),
        params: TurnSteerParams {
            thread_id: "thread-1".to_string(),
            expected_turn_id: "turn-1".to_string(),
            client_user_message_id: None,
            input: Vec::new(),
            responsesapi_client_metadata: None,
            additional_context: None,
        },
    }
}

fn sample_thread_archive_request() -> ClientRequest {
    ClientRequest::ThreadArchive {
        request_id: RequestId::Integer(3),
        params: ThreadArchiveParams {
            thread_id: "thread-1".to_string(),
        },
    }
}

fn sample_thread(thread_id: &str) -> Thread {
    Thread {
        id: thread_id.to_string(),
        session_id: format!("session-{thread_id}"),
        forked_from_id: None,
        parent_thread_id: None,
        preview: "first prompt".to_string(),
        ephemeral: false,
        model_provider: "kimi".to_string(),
        created_at: 1,
        updated_at: 2,
        recency_at: Some(2),
        status: AppServerThreadStatus::Idle,
        path: None,
        cwd: test_path_buf("/tmp").abs(),
        cli_version: "0.0.0".to_string(),
        source: AppServerSessionSource::Exec,
        thread_source: None,
        agent_nickname: None,
        agent_role: None,
        git_info: None,
        name: None,
        turns: Vec::new(),
    }
}

fn sample_thread_start_response() -> ClientResponsePayload {
    ClientResponsePayload::ThreadStart(ThreadStartResponse {
        thread: sample_thread("thread-1"),
        model: "kimi-k2.5".to_string(),
        model_provider: "kimi".to_string(),
        service_tier: None,
        cwd: test_path_buf("/tmp").abs(),
        runtime_workspace_roots: Vec::new(),
        instruction_sources: Vec::new(),
        approval_policy: AppServerAskForApproval::OnFailure,
        approvals_reviewer: AppServerApprovalsReviewer::User,
        sandbox: AppServerSandboxPolicy::DangerFullAccess,
        active_permission_profile: None,
        reasoning_effort: None,
        multi_agent_mode: Default::default(),
    })
}

fn sample_thread_resume_response() -> ClientResponsePayload {
    ClientResponsePayload::ThreadResume(ThreadResumeResponse {
        thread: sample_thread("thread-2"),
        model: "kimi-k2.5".to_string(),
        model_provider: "kimi".to_string(),
        service_tier: None,
        cwd: test_path_buf("/tmp").abs(),
        runtime_workspace_roots: Vec::new(),
        instruction_sources: Vec::new(),
        approval_policy: AppServerAskForApproval::OnFailure,
        approvals_reviewer: AppServerApprovalsReviewer::User,
        sandbox: AppServerSandboxPolicy::DangerFullAccess,
        active_permission_profile: None,
        reasoning_effort: None,
        multi_agent_mode: Default::default(),
        initial_turns_page: None,
    })
}

fn sample_thread_fork_response() -> ClientResponsePayload {
    ClientResponsePayload::ThreadFork(ThreadForkResponse {
        thread: sample_thread("thread-3"),
        model: "kimi-k2.5".to_string(),
        model_provider: "kimi".to_string(),
        service_tier: None,
        cwd: test_path_buf("/tmp").abs(),
        runtime_workspace_roots: Vec::new(),
        instruction_sources: Vec::new(),
        approval_policy: AppServerAskForApproval::OnFailure,
        approvals_reviewer: AppServerApprovalsReviewer::User,
        sandbox: AppServerSandboxPolicy::DangerFullAccess,
        active_permission_profile: None,
        reasoning_effort: None,
        multi_agent_mode: Default::default(),
    })
}

fn sample_turn_start_response() -> ClientResponsePayload {
    ClientResponsePayload::TurnStart(TurnStartResponse {
        turn: Turn {
            id: "turn-1".to_string(),
            items_view: ody_app_server_protocol::TurnItemsView::Full,
            items: Vec::new(),
            status: AppServerTurnStatus::InProgress,
            error: None,
            started_at: None,
            completed_at: None,
            duration_ms: None,
        },
    })
}

fn sample_turn_steer_response() -> ClientResponsePayload {
    ClientResponsePayload::TurnSteer(TurnSteerResponse {
        turn_id: "turn-2".to_string(),
    })
}

#[test]
fn track_request_only_enqueues_analytics_relevant_requests() {
    let (client, mut receiver) = client_with_receiver();

    for (request_id, request) in [
        (RequestId::Integer(1), sample_turn_start_request()),
        (RequestId::Integer(2), sample_turn_steer_request()),
    ] {
        client.track_request(/*connection_id*/ 7, request_id, &request);
        assert!(matches!(
            receiver.try_recv(),
            Ok(AnalyticsFact::ClientRequest { .. })
        ));
    }

    let ignored_request = sample_thread_archive_request();
    client.track_request(
        /*connection_id*/ 7,
        RequestId::Integer(3),
        &ignored_request,
    );
    assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));
}

#[test]
fn track_response_only_enqueues_analytics_relevant_responses() {
    let (client, mut receiver) = client_with_receiver();

    for (request_id, response) in [
        (RequestId::Integer(1), sample_thread_start_response()),
        (RequestId::Integer(2), sample_thread_resume_response()),
        (RequestId::Integer(3), sample_thread_fork_response()),
        (RequestId::Integer(4), sample_turn_start_response()),
        (RequestId::Integer(5), sample_turn_steer_response()),
    ] {
        client.track_response(/*connection_id*/ 7, request_id, response);
        assert!(matches!(
            receiver.try_recv(),
            Ok(AnalyticsFact::ClientResponse { .. })
        ));
    }

    client.track_response(
        /*connection_id*/ 7,
        RequestId::Integer(6),
        ClientResponsePayload::ThreadArchive(ThreadArchiveResponse {}),
    );
    assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));
}

#[test]
fn track_event_request_batches_only_isolates_accepted_line_fingerprint_events() {
    let batches = track_event_request_batches(vec![
        sample_regular_track_event("thread-1"),
        sample_regular_track_event("thread-2"),
        sample_accepted_line_fingerprint_event("thread-3"),
        sample_accepted_line_fingerprint_event("thread-4"),
        sample_regular_track_event("thread-5"),
        sample_regular_track_event("thread-6"),
    ]);

    assert_eq!(batches.len(), 4);
    assert_eq!(batches[0].len(), 2);
    assert_eq!(batches[1].len(), 1);
    assert_eq!(batches[2].len(), 1);
    assert_eq!(batches[3].len(), 2);
    assert!(batches[1][0].should_send_in_isolated_request());
    assert!(batches[2][0].should_send_in_isolated_request());
}

#[tokio::test]
async fn track_design_review_started_emits_custom_fact() {
    let (client, mut receiver) = client_with_receiver();
    let input = DesignReviewStartedInput {
        thread_id: "thread-1".to_string(),
        turn_id: "turn-1".to_string(),
        review_model: "gpt-review".to_string(),
        started_at_ms: 1_000,
    };
    client.track_design_review_started(input.clone());

    let fact = receiver.try_recv().expect("fact should be queued");
    match fact {
        AnalyticsFact::Custom(CustomAnalyticsFact::DesignReviewStarted(received)) => {
            assert_eq!(received.thread_id, "thread-1");
            assert_eq!(received.turn_id, "turn-1");
            assert_eq!(received.review_model, "gpt-review");
            assert_eq!(received.started_at_ms, 1_000);
        }
        _other => panic!("expected DesignReviewStarted fact, got other variant"),
    }
}

#[tokio::test]
async fn track_design_review_completed_emits_custom_fact() {
    let (client, mut receiver) = client_with_receiver();
    let input = DesignReviewCompletedInput {
        thread_id: "thread-1".to_string(),
        turn_id: "turn-1".to_string(),
        review_model: "gpt-review".to_string(),
        finding_count: 3,
        critical_count: 1,
        high_count: 1,
        medium_count: 1,
        low_count: 0,
        started_at_ms: 1_000,
        completed_at_ms: 2_000,
    };
    client.track_design_review_completed(input.clone());

    let fact = receiver.try_recv().expect("fact should be queued");
    match fact {
        AnalyticsFact::Custom(CustomAnalyticsFact::DesignReviewCompleted(received)) => {
            assert_eq!(received.finding_count, 3);
            assert_eq!(received.critical_count, 1);
            assert_eq!(received.low_count, 0);
        }
        _other => panic!("expected DesignReviewCompleted fact, got other variant"),
    }
}

#[tokio::test]
async fn track_design_review_failed_emits_custom_fact() {
    let (client, mut receiver) = client_with_receiver();
    let input = DesignReviewFailedInput {
        thread_id: "thread-1".to_string(),
        turn_id: "turn-1".to_string(),
        review_model: "gpt-review".to_string(),
        reason: DesignReviewFailureReason::Timeout,
        started_at_ms: 1_000,
        completed_at_ms: 2_000,
    };
    client.track_design_review_failed(input.clone());

    let fact = receiver.try_recv().expect("fact should be queued");
    match fact {
        AnalyticsFact::Custom(CustomAnalyticsFact::DesignReviewFailed(received)) => {
            assert_eq!(received.reason, DesignReviewFailureReason::Timeout);
        }
        _other => panic!("expected DesignReviewFailed fact, got other variant"),
    }
}
