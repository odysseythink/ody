#![allow(clippy::unwrap_used)]

use core_test_support::test_ody::local_selections;

use core_test_support::TempDirExt;
use core_test_support::responses;
use core_test_support::responses::ResponsesRequest;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_ody::TestOdy;
use core_test_support::test_ody::test_ody;
use core_test_support::test_ody::turn_permission_fields;
use core_test_support::wait_for_event_match;
use ody_protocol::config_types::CollaborationMode;
use ody_protocol::config_types::ModeKind;
use ody_protocol::config_types::Settings;
use ody_protocol::items::TurnItem;
use ody_protocol::models::PermissionProfile;
use ody_protocol::protocol::AskForApproval;
use ody_protocol::protocol::EventMsg;
use ody_protocol::protocol::Op;
use ody_protocol::user_input::UserInput;
use serde_json::json;

fn call_output(req: &ResponsesRequest, call_id: &str) -> (String, Option<bool>) {
    let raw = req.function_call_output(call_id);
    assert_eq!(
        raw.get("call_id").and_then(serde_json::Value::as_str),
        Some(call_id),
        "mismatched call_id in function_call_output"
    );
    let (content_opt, success) = req
        .function_call_output_content_and_success(call_id)
        .expect("function_call_output present");
    let content = content_opt.expect("function_call_output content present");
    (content, success)
}

async fn submit_plan_round_trip() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;

    let mut builder = test_ody();
    let TestOdy {
        ody,
        cwd,
        session_configured,
        ..
    } = builder.build(&server).await?;

    let call_id = "submit-plan-call";
    let plan_markdown = "# Test Plan\n- Step 1\n- Step 2\n";
    let args = json!({"plan": plan_markdown}).to_string();

    // First response: the model calls submit_plan. There should be no second
    // response because submit_plan marks the turn as submitted.
    let first_response = sse(vec![
        ev_response_created("resp-1"),
        ev_function_call(call_id, "submit_plan", &args),
        ev_completed("resp-1"),
    ]);
    let first_mock = responses::mount_sse_once(&server, first_response).await;

    let (sandbox_policy, permission_profile) =
        turn_permission_fields(PermissionProfile::Disabled, cwd.path());

    ody.submit(Op::UserInput {
        items: vec![UserInput::Text {
            text: "please make a plan".into(),
            text_elements: Vec::new(),
        }],
        final_output_json_schema: None,
        responsesapi_client_metadata: None,
        additional_context: Default::default(),
        thread_settings: ody_protocol::protocol::ThreadSettingsOverrides {
            environments: Some(local_selections(cwd.abs())),
            approval_policy: Some(AskForApproval::Never),
            sandbox_policy: Some(sandbox_policy),
            permission_profile,
            collaboration_mode: Some(CollaborationMode {
                mode: ModeKind::Plan,
                settings: Settings {
                    model: session_configured.model.clone(),
                    reasoning_effort: None,
                    developer_instructions: None,
                },
            }),
            ..Default::default()
        },
    })
    .await?;

    let completed = wait_for_event_match(&ody, |event| match event {
        EventMsg::ItemCompleted(ody_protocol::protocol::ItemCompletedEvent {
            item: TurnItem::Plan(item),
            ..
        }) => Some(item.clone()),
        _ => None,
    })
    .await;

    assert_eq!(completed.text, plan_markdown);
    assert!(
        completed.plan_file_path.is_some(),
        "plan_file_path should be set"
    );
    let plan_path = completed.plan_file_path.unwrap();
    assert!(
        plan_path.starts_with(cwd.path()),
        "plan path {plan_path:?} should be under cwd"
    );
    assert!(
        plan_path.exists(),
        "plan file {plan_path:?} should have been persisted"
    );
    let persisted = tokio::fs::read_to_string(&plan_path).await?;
    assert_eq!(persisted, plan_markdown, "persisted plan should match");

    let req = first_mock.single_request();
    let (output_text, success) = call_output(&req, call_id);
    assert_eq!(output_text, "Plan submitted");
    assert_eq!(success, Some(true));

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn submit_plan_persists_plan_and_ends_turn() -> anyhow::Result<()> {
    submit_plan_round_trip().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn submit_plan_terminal_does_not_trigger_second_sampling() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;

    let mut builder = test_ody();
    let TestOdy {
        ody,
        cwd,
        session_configured,
        ..
    } = builder.build(&server).await?;

    let call_id = "submit-plan-terminal";
    let plan_markdown = "# Terminal Test\n- only one response\n";
    let args = json!({"plan": plan_markdown}).to_string();

    let first_response = sse(vec![
        ev_response_created("resp-1"),
        ev_function_call(call_id, "submit_plan", &args),
        ev_completed("resp-1"),
    ]);
    let first_mock = responses::mount_sse_once(&server, first_response).await;

    let (sandbox_policy, permission_profile) =
        turn_permission_fields(PermissionProfile::Disabled, cwd.path());

    ody.submit(Op::UserInput {
        items: vec![UserInput::Text {
            text: "please make a plan".into(),
            text_elements: Vec::new(),
        }],
        final_output_json_schema: None,
        responsesapi_client_metadata: None,
        additional_context: Default::default(),
        thread_settings: ody_protocol::protocol::ThreadSettingsOverrides {
            environments: Some(local_selections(cwd.abs())),
            approval_policy: Some(AskForApproval::Never),
            sandbox_policy: Some(sandbox_policy),
            permission_profile,
            collaboration_mode: Some(CollaborationMode {
                mode: ModeKind::Plan,
                settings: Settings {
                    model: session_configured.model.clone(),
                    reasoning_effort: None,
                    developer_instructions: None,
                },
            }),
            ..Default::default()
        },
    })
    .await?;

    let _completed = wait_for_event_match(&ody, |event| match event {
        EventMsg::ItemCompleted(ody_protocol::protocol::ItemCompletedEvent {
            item: TurnItem::Plan(_),
            ..
        }) => Some(()),
        _ => None,
    })
    .await;

    let req = first_mock.single_request();
    let (output_text, _) = call_output(&req, call_id);
    assert_eq!(output_text, "Plan submitted");

    // The server must have received exactly one /responses request.
    let requests = server
        .received_requests()
        .await
        .expect("server recorded requests");
    let responses_count = requests
        .iter()
        .filter(|r| r.method == "POST" && r.url.path().ends_with("/responses"))
        .count();
    assert_eq!(
        responses_count, 1,
        "submit_plan should end the turn after a single /responses request"
    );

    Ok(())
}
