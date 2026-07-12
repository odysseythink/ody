use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;
use app_test_support::TestAppServer;
use app_test_support::create_mock_responses_server_sequence_unchecked;
use app_test_support::to_response;
use core_test_support::responses;
use core_test_support::skip_if_no_network;
use ody_app_server_protocol::ItemCompletedNotification;
use ody_app_server_protocol::ItemStartedNotification;
use ody_app_server_protocol::JSONRPCMessage;
use ody_app_server_protocol::JSONRPCResponse;
use ody_app_server_protocol::PlanDeltaNotification;
use ody_app_server_protocol::RequestId;
use ody_app_server_protocol::ThreadItem;
use ody_app_server_protocol::ThreadStartParams;
use ody_app_server_protocol::ThreadStartResponse;
use ody_app_server_protocol::TurnCompletedNotification;
use ody_app_server_protocol::TurnStartParams;
use ody_app_server_protocol::TurnStartResponse;
use ody_app_server_protocol::TurnStatus;
use ody_app_server_protocol::UserInput as V2UserInput;
use ody_features::FEATURES;
use ody_features::Feature;
use ody_protocol::config_types::CollaborationMode;
use ody_protocol::config_types::ModeKind;
use ody_protocol::config_types::Settings;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;
use std::path::Path;
use tempfile::TempDir;
use tokio::time::sleep;
use tokio::time::timeout;
use wiremock::MockServer;

const DEFAULT_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn plan_mode_submit_plan_emits_plan_item() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let plan_markdown = "# Final plan\n- first\n- second\n";
    let args = serde_json::json!({"plan": plan_markdown}).to_string();
    let responses = vec![responses::sse(vec![
        responses::ev_response_created("resp-1"),
        responses::ev_function_call("call-submit-plan", "submit_plan", &args),
        responses::ev_completed("resp-1"),
    ])];
    let server = create_mock_responses_server_sequence_unchecked(responses).await;

    let ody_home = TempDir::new()?;
    create_config_toml(ody_home.path(), &server.uri())?;

    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let turn = start_plan_mode_turn(&mut mcp).await?;
    let (_, completed_items, plan_deltas, turn_completed) =
        collect_turn_notifications(&mut mcp).await?;
    wait_for_responses_request_count(&server, /*expected_count*/ 1).await?;

    assert_eq!(turn_completed.turn.id, turn.id);
    assert_eq!(turn_completed.turn.status, TurnStatus::Completed);

    // The single PlanDelta from submit_plan should carry the full plan text.
    let expected_plan_id = format!("{}-plan", turn.id);
    assert!(
        plan_deltas
            .iter()
            .all(|delta| delta.item_id == expected_plan_id),
        "all plan deltas must belong to the same plan item"
    );
    let streamed_plan = plan_deltas
        .iter()
        .map(|delta| delta.delta.as_str())
        .collect::<String>();
    assert_eq!(streamed_plan, plan_markdown);

    let plan_items: Vec<_> = completed_items
        .iter()
        .filter_map(|item| match item {
            ThreadItem::Plan { .. } => Some(item.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(plan_items.len(), 1, "expected exactly one Plan item");

    let plan_item = &plan_items[0];
    let ThreadItem::Plan {
        id,
        text,
        plan_file_path,
    } = plan_item
    else {
        unreachable!();
    };
    assert_eq!(id, &expected_plan_id);
    assert_eq!(text, &plan_markdown);
    assert!(
        plan_file_path.is_some(),
        "submit_plan must set plan_file_path"
    );
    let plan_path = plan_file_path.as_ref().unwrap();
    assert!(
        plan_path.starts_with(ody_home.path()),
        "plan path {plan_path:?} should be under ody_home"
    );
    assert!(plan_path.exists(), "plan file {plan_path:?} should exist");
    let persisted = tokio::fs::read_to_string(plan_path).await?;
    assert_eq!(persisted, plan_markdown, "persisted plan should match");

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn plan_mode_without_proposed_plan_does_not_emit_plan_item() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let responses = vec![responses::sse(vec![
        responses::ev_response_created("resp-1"),
        responses::ev_assistant_message("msg-1", "Done"),
        responses::ev_completed("resp-1"),
    ])];
    let server = create_mock_responses_server_sequence_unchecked(responses).await;

    let ody_home = TempDir::new()?;
    create_config_toml(ody_home.path(), &server.uri())?;

    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let _turn = start_plan_mode_turn(&mut mcp).await?;
    let (_, completed_items, plan_deltas, _) = collect_turn_notifications(&mut mcp).await?;
    wait_for_responses_request_count(&server, /*expected_count*/ 1).await?;

    let has_plan_item = completed_items
        .iter()
        .any(|item| matches!(item, ThreadItem::Plan { .. }));
    assert!(!has_plan_item);
    assert!(plan_deltas.is_empty());

    Ok(())
}

async fn start_plan_mode_turn(mcp: &mut TestAppServer) -> Result<ody_app_server_protocol::Turn> {
    let thread_req = mcp
        .send_thread_start_request(ThreadStartParams {
            model: Some("mock-model".to_string()),
            ..Default::default()
        })
        .await?;
    let thread_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(thread_req)),
    )
    .await??;
    let thread = to_response::<ThreadStartResponse>(thread_resp)?.thread;

    let collaboration_mode = CollaborationMode {
        mode: ModeKind::Plan,
        settings: Settings {
            model: "mock-model".to_string(),
            reasoning_effort: None,
            developer_instructions: None,
            design_audit_level: None,
        },
    };
    let turn_req = mcp
        .send_turn_start_request(TurnStartParams {
            thread_id: thread.id,
            client_user_message_id: None,
            input: vec![V2UserInput::Text {
                text: "Plan this".to_string(),
                text_elements: Vec::new(),
            }],
            collaboration_mode: Some(collaboration_mode),
            ..Default::default()
        })
        .await?;
    let turn_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(turn_req)),
    )
    .await??;
    Ok(to_response::<TurnStartResponse>(turn_resp)?.turn)
}

async fn collect_turn_notifications(
    mcp: &mut TestAppServer,
) -> Result<(
    Vec<ThreadItem>,
    Vec<ThreadItem>,
    Vec<PlanDeltaNotification>,
    TurnCompletedNotification,
)> {
    let mut started_items = Vec::new();
    let mut completed_items = Vec::new();
    let mut plan_deltas = Vec::new();

    loop {
        let message = timeout(DEFAULT_READ_TIMEOUT, mcp.read_next_message()).await??;
        let JSONRPCMessage::Notification(notification) = message else {
            continue;
        };
        match notification.method.as_str() {
            "item/started" => {
                let params = notification
                    .params
                    .ok_or_else(|| anyhow!("item/started notifications must include params"))?;
                let payload: ItemStartedNotification = serde_json::from_value(params)?;
                started_items.push(payload.item);
            }
            "item/completed" => {
                let params = notification
                    .params
                    .ok_or_else(|| anyhow!("item/completed notifications must include params"))?;
                let payload: ItemCompletedNotification = serde_json::from_value(params)?;
                completed_items.push(payload.item);
            }
            "item/plan/delta" => {
                let params = notification
                    .params
                    .ok_or_else(|| anyhow!("item/plan/delta notifications must include params"))?;
                let payload: PlanDeltaNotification = serde_json::from_value(params)?;
                plan_deltas.push(payload);
            }
            "turn/completed" => {
                let params = notification
                    .params
                    .ok_or_else(|| anyhow!("turn/completed notifications must include params"))?;
                let payload: TurnCompletedNotification = serde_json::from_value(params)?;
                return Ok((started_items, completed_items, plan_deltas, payload));
            }
            _ => {}
        }
    }
}

async fn wait_for_responses_request_count(
    server: &MockServer,
    expected_count: usize,
) -> Result<()> {
    timeout(DEFAULT_READ_TIMEOUT, async {
        loop {
            let Some(requests) = server.received_requests().await else {
                bail!("wiremock did not record requests");
            };
            let responses_request_count = requests
                .iter()
                .filter(|request| {
                    request.method == "POST" && request.url.path().ends_with("/responses")
                })
                .count();
            if responses_request_count == expected_count {
                return Ok::<(), anyhow::Error>(());
            }
            if responses_request_count > expected_count {
                bail!(
                    "expected exactly {expected_count} /responses requests, got {responses_request_count}"
                );
            }
            sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await??;
    Ok(())
}

fn create_config_toml(ody_home: &Path, server_uri: &str) -> std::io::Result<()> {
    let features = BTreeMap::from([(Feature::CollaborationModes, true)]);
    let feature_entries = features
        .into_iter()
        .map(|(feature, enabled)| {
            let key = FEATURES
                .iter()
                .find(|spec| spec.id == feature)
                .map(|spec| spec.key)
                .expect("feature should have a config key");
            format!("{key} = {enabled}")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let config_toml = ody_home.join("config.toml");
    std::fs::write(
        config_toml,
        format!(
            r#"
model = "mock-model"
approval_policy = "never"
sandbox_mode = "read-only"

model_provider = "mock_provider"

[features]
{feature_entries}

[model_providers.mock_provider]
name = "Mock provider for test"
base_url = "{server_uri}/v1"
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0
"#
        ),
    )
}
