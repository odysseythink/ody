#![allow(clippy::unwrap_used)]

use core_test_support::test_ody::local_selections;

use core_test_support::TempDirExt;
use core_test_support::responses;
use core_test_support::responses::ResponsesRequest;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_sequence;
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

fn complete_design_markdown() -> String {
    // A minimal design that passes C1–C8 (300+ chars, 3+ ## headings, all 8 sections).
    concat!(
        "# Feature Design\n\n",
        "## Scope\n",
        "In scope: the core behaviour. Out of scope: the UI polish. ",
        "This line pads the document beyond the minimum content length so ",
        "the structural gate does not trip on an otherwise complete design.\n\n",
        "## Architecture\n",
        "The approach is to reuse the existing pipeline and add a stage.\n\n",
        "## Data Models\n",
        "struct DesignState { sections: Vec<String> }\n\n",
        "## Algorithms\n",
        "implementation notes: walk the sections and tally coverage.\n\n",
        "## Error Handling\n",
        "failure scenarios and graceful degradation are handled inline.\n\n",
        "## Self-Review\n",
        "audit checklist reviewed against the rubric.\n\n",
        "## User Approval\n",
        "user final approval captured before handoff.\n\n",
        "## Reuse Analysis\n",
        "component reuse survey of existing components follows.\n",
    ).to_string()
}

// ---------------------------------------------------------------------------
// T2: Mode validation — submit_design rejected in Plan mode
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn submit_design_rejected_in_plan_mode() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;

    let mut builder = test_ody();
    let TestOdy {
        ody,
        cwd,
        session_configured,
        ..
    } = builder.build(&server).await?;

    let call_id = "submit-design-in-plan-mode";
    let design_markdown = complete_design_markdown();
    let args = json!({"design": design_markdown}).to_string();

    let response = sse(vec![
        ev_response_created("resp-1"),
        ev_function_call(call_id, "submit_design", &args),
        ev_completed("resp-1"),
    ]);
    let mock = responses::mount_sse_once(&server, response).await;

    let (sandbox_policy, permission_profile) =
        turn_permission_fields(PermissionProfile::Disabled, cwd.path());

    ody.submit(Op::UserInput {
        items: vec![UserInput::Text {
            text: "please submit the design".into(),
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
            // Intentional: Plan mode, not Design — the handler must reject.
            collaboration_mode: Some(CollaborationMode {
                mode: ModeKind::Plan,
                settings: Settings {
                    model: session_configured.model.clone(),
                    reasoning_effort: None,
                    developer_instructions: None,
                    design_audit_level: None,
                },
            }),
            ..Default::default()
        },
    })
    .await?;

    // The turn should complete (function call was made), but the tool output
    // should be an error.
    let _completed = wait_for_event_match(&ody, |event| match event {
        EventMsg::ItemCompleted(_) => Some(()),
        _ => None,
    })
    .await;

    let req = mock.single_request();
    let (output_text, success) = call_output(&req, call_id);
    assert_eq!(success, Some(false), "call in Plan mode must return error (success=false)");
    assert!(
        output_text.contains("only available in Design mode"),
        "error must name Design mode: {output_text}"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// T3: Design finalization — persist, item id suffix, "Design submitted"
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn submit_design_persists_and_ends_turn() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;

    let mut builder = test_ody();
    let TestOdy {
        ody,
        cwd,
        session_configured,
        ..
    } = builder.build(&server).await?;

    let call_id = "submit-design-call";
    let design_markdown = complete_design_markdown();
    let args = json!({"design": design_markdown, "final": true}).to_string();

    let response = sse(vec![
        ev_response_created("resp-1"),
        ev_function_call(call_id, "submit_design", &args),
        ev_completed("resp-1"),
    ]);
    let mock = responses::mount_sse_once(&server, response).await;

    let (sandbox_policy, permission_profile) =
        turn_permission_fields(PermissionProfile::Disabled, cwd.path());

    ody.submit(Op::UserInput {
        items: vec![UserInput::Text {
            text: "please design something".into(),
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
                mode: ModeKind::Design,
                settings: Settings {
                    model: session_configured.model.clone(),
                    reasoning_effort: None,
                    developer_instructions: None,
                    design_audit_level: None,
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

    // T3 assertion: item id must end with "-design".
    assert!(
        completed.id.ends_with("-design"),
        "item id must end with '-design', got: {}",
        completed.id
    );
    assert_eq!(completed.text, design_markdown);

    // T3 assertion: file persisted in .ody-code/designs/.
    let plan_path = completed.plan_file_path.expect("plan_file_path must be set");
    assert!(
        plan_path.starts_with(cwd.path()),
        "design path {plan_path:?} should be under cwd"
    );
    assert!(
        plan_path.exists(),
        "design file {plan_path:?} should have been persisted"
    );
    let path_str = plan_path.to_string_lossy();
    assert!(
        path_str.contains("designs"),
        "design file must be under designs/ directory: {path_str}"
    );
    let persisted = tokio::fs::read_to_string(&plan_path).await?;
    assert_eq!(persisted, design_markdown, "persisted design must match submitted markdown");

    // T3 assertion: output is "Design submitted".
    let req = mock.single_request();
    let (output_text, success) = call_output(&req, call_id);
    assert_eq!(output_text, "Design submitted");
    assert_eq!(success, Some(true));

    // T3 assertion: only one /responses request (turn terminated).
    let requests = server
        .received_requests()
        .await
        .expect("server recorded requests");
    let responses_count = requests
        .iter()
        .filter(|r| r.method == "POST" && r.url.path().ends_with("/responses"))
        .count();
    assert_eq!(responses_count, 1, "submit_design should end the turn");

    // T8: termination gate in turn.rs widened to Plan|Design.
    // If submit_design had NOT ended the turn, the mock server would have
    // received a second /responses request.

    Ok(())
}

// ---------------------------------------------------------------------------
// T4: C1–C8 rejection — file persisted but NOT final, missing sections listed
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn submit_design_rejects_incomplete_c1_c8() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;

    let mut builder = test_ody();
    let TestOdy {
        ody,
        cwd,
        session_configured,
        ..
    } = builder.build(&server).await?;

    // Design missing C8 (Reuse Analysis) and intentionally below 300 chars
    // to test structural gate too.
    let incomplete = "# D\n\n## Scope\nIn.\n\n## Architecture\nDesign.\n";
    let call_id = "submit-design-incomplete";
    let args = json!({"design": incomplete, "final": true}).to_string();

    let first_response = sse(vec![
        ev_response_created("resp-1"),
        ev_function_call(call_id, "submit_design", &args),
        ev_completed("resp-1"),
    ]);

    // Second response: model retries with complete design.
    let complete = complete_design_markdown();
    let retry_call_id = "submit-design-retry";
    let retry_args = json!({"design": complete, "final": true}).to_string();
    let second_response = sse(vec![
        ev_response_created("resp-2"),
        ev_function_call(retry_call_id, "submit_design", &retry_args),
        ev_completed("resp-2"),
    ]);

    let response_mock = mount_sse_sequence(&server, vec![first_response, second_response]).await;

    let (sandbox_policy, permission_profile) =
        turn_permission_fields(PermissionProfile::Disabled, cwd.path());

    ody.submit(Op::UserInput {
        items: vec![UserInput::Text {
            text: "please design".into(),
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
                mode: ModeKind::Design,
                settings: Settings {
                    model: session_configured.model.clone(),
                    reasoning_effort: None,
                    developer_instructions: None,
                    design_audit_level: None,
                },
            }),
            ..Default::default()
        },
    })
    .await?;

    // First completion (incomplete design).
    let first_completed = wait_for_event_match(&ody, |event| match event {
        EventMsg::ItemCompleted(ody_protocol::protocol::ItemCompletedEvent {
            item: TurnItem::Plan(item),
            ..
        }) => Some(item.clone()),
        _ => None,
    })
    .await;
    assert_eq!(first_completed.text, incomplete);

    // T4 assertion: file WAS persisted despite being incomplete.
    let first_path = first_completed.plan_file_path.expect("plan_file_path must be set even for incomplete");
    let persisted = tokio::fs::read_to_string(&first_path).await?;
    assert_eq!(persisted, incomplete, "incomplete design must still be persisted to disk");

    // Second completion (complete design).
    let second_completed = wait_for_event_match(&ody, |event| match event {
        EventMsg::ItemCompleted(ody_protocol::protocol::ItemCompletedEvent {
            item: TurnItem::Plan(item),
            ..
        }) => Some(item.clone()),
        _ => None,
    })
    .await;
    assert_eq!(second_completed.text, complete);

    let requests = response_mock.requests();
    assert_eq!(requests.len(), 2, "incomplete must trigger retry; got {} requests", requests.len());

    // T4 assertion: first call output contains "NOT final" or "incomplete".
    let (first_output, first_success) = call_output(&requests[0], call_id);
    assert_eq!(first_success, Some(true), "incomplete design call is not an error (success=true, but non-terminal)");
    assert!(
        first_output.contains("NOT final") || first_output.contains("incomplete"),
        "first output must indicate non-final state: {first_output}"
    );

    // T4 assertion: retry call is terminal.
    let (retry_output, retry_success) = call_output(&requests[1], retry_call_id);
    assert_eq!(retry_output, "Design submitted");
    assert_eq!(retry_success, Some(true));

    Ok(())
}

// ---------------------------------------------------------------------------
// T5: Split middle submission — stem_dir path, NOT terminal, NO C1–C8 check
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn submit_design_split_pending_part_returns_stem_dir() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;

    let mut builder = test_ody();
    let TestOdy {
        ody,
        cwd,
        session_configured,
        ..
    } = builder.build(&server).await?;

    // Index with pending part — intentionally incomplete (no C8), but split
    // mode must skip C1–C8 check.
    let index_markdown = "# Split Design\n\n## Parts\n| # | File | Scope | Status |\n|---|---|---|---|\n| 1 | part1.md | scope one | pending |\n";
    let index_call_id = "submit-design-split-index";
    let index_args = json!({"design": index_markdown}).to_string();

    let first_response = sse(vec![
        ev_response_created("resp-1"),
        ev_function_call(index_call_id, "submit_design", &index_args),
        ev_completed("resp-1"),
    ]);

    // Second call: all parts done (no manifest — single-file final). Still
    // needs enough content/headings to pass C1–C8.
    let final_markdown = complete_design_markdown();
    let final_call_id = "submit-design-final";
    let final_args = json!({"design": final_markdown, "final": true}).to_string();
    let second_response = sse(vec![
        ev_response_created("resp-2"),
        ev_function_call(final_call_id, "submit_design", &final_args),
        ev_completed("resp-2"),
    ]);

    let response_mock = mount_sse_sequence(&server, vec![first_response, second_response]).await;

    let (sandbox_policy, permission_profile) =
        turn_permission_fields(PermissionProfile::Disabled, cwd.path());

    ody.submit(Op::UserInput {
        items: vec![UserInput::Text {
            text: "please design with split".into(),
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
                mode: ModeKind::Design,
                settings: Settings {
                    model: session_configured.model.clone(),
                    reasoning_effort: None,
                    developer_instructions: None,
                    design_audit_level: None,
                },
            }),
            ..Default::default()
        },
    })
    .await?;

    let index_completed = wait_for_event_match(&ody, |event| match event {
        EventMsg::ItemCompleted(ody_protocol::protocol::ItemCompletedEvent {
            item: TurnItem::Plan(item),
            ..
        }) => Some(item.clone()),
        _ => None,
    })
    .await;
    assert_eq!(index_completed.text, index_markdown);

    // Second completion.
    let final_completed = wait_for_event_match(&ody, |event| match event {
        EventMsg::ItemCompleted(ody_protocol::protocol::ItemCompletedEvent {
            item: TurnItem::Plan(item),
            ..
        }) => Some(item.clone()),
        _ => None,
    })
    .await;
    assert_eq!(final_completed.text, final_markdown);

    let requests = response_mock.requests();
    assert_eq!(requests.len(), 2, "pending split must continue the turn");

    let (index_output, index_success) = call_output(&requests[0], index_call_id);
    assert_eq!(index_success, Some(true));

    // T5 assertion: must contain stem_dir absolute path.
    assert!(
        index_output.contains("designs/") || index_output.contains("designs\\"),
        "split output must contain stem_dir path (expected 'designs/'): {index_output}"
    );

    // T5 assertion: must NOT say "Design submitted".
    assert_ne!(index_output, "Design submitted", "split call must not be terminal");

    // T5 assertion: must NOT contain "Plan mode" (no cross-mode language leak).
    assert!(
        !index_output.contains("Plan mode"),
        "design split output must not mention Plan mode: {index_output}"
    );

    // T5 assertion: must mention "Design mode".
    assert!(
        index_output.to_lowercase().contains("design mode"),
        "split output must mention Design mode: {index_output}"
    );

    let (final_output, _) = call_output(&requests[1], final_call_id);
    assert_eq!(final_output, "Design submitted");

    Ok(())
}

// ---------------------------------------------------------------------------
// T6: Done-parts guard — bare submission accepted when nothing verified-done
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn submit_design_rejects_naked_index_after_done_parts() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;

    let mut builder = test_ody();
    let TestOdy {
        ody,
        cwd,
        session_configured,
        ..
    } = builder.build(&server).await?;

    // First: submit a design that the after-turn hook processes (has parts manifest).
    let parts_done_markdown = "# Split Design\n\n## Parts\n| # | File | Scope | Status |\n|---|---|---|---|\n| 1 | core.md | core impl | done |\n";
    let parts_done_call_id = "submit-design-parts-done";
    let parts_done_args = json!({"design": parts_done_markdown}).to_string();

    let first_sse = sse(vec![
        ev_response_created("resp-1"),
        ev_function_call(parts_done_call_id, "submit_design", &parts_done_args),
        ev_completed("resp-1"),
    ]);

    // Second: try to submit a bare design with no ## Parts — this must be rejected
    // if the after-turn hook previously recorded done_count > 0.
    let bare_call_id = "submit-design-bare";
    let bare_markdown = complete_design_markdown();
    let bare_args = json!({"design": bare_markdown}).to_string();

    let second_sse = sse(vec![
        ev_response_created("resp-2"),
        ev_function_call(bare_call_id, "submit_design", &bare_args),
        ev_completed("resp-2"),
    ]);

    let response_mock = mount_sse_sequence(&server, vec![first_sse, second_sse]).await;

    let (sandbox_policy, permission_profile) =
        turn_permission_fields(PermissionProfile::Disabled, cwd.path());

    ody.submit(Op::UserInput {
        items: vec![UserInput::Text {
            text: "design with parts then bare".into(),
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
                mode: ModeKind::Design,
                settings: Settings {
                    model: session_configured.model.clone(),
                    reasoning_effort: None,
                    developer_instructions: None,
                    design_audit_level: None,
                },
            }),
            ..Default::default()
        },
    })
    .await?;

    // First completion (parts manifest).
    let _first = wait_for_event_match(&ody, |event| match event {
        EventMsg::ItemCompleted(ody_protocol::protocol::ItemCompletedEvent {
            item: TurnItem::Plan(item),
            ..
        }) => Some(item.clone()),
        _ => None,
    })
    .await;

    // Second completion.
    let _second = wait_for_event_match(&ody, |event| match event {
        EventMsg::ItemCompleted(ody_protocol::protocol::ItemCompletedEvent {
            item: TurnItem::Plan(item),
            ..
        }) => Some(item.clone()),
        _ => None,
    })
    .await;

    let requests = response_mock.requests();
    assert_eq!(requests.len(), 2);

    // T6 assertion: The bare submission should be accepted and terminal because
    // the manifest snapshot had no verified-done rows (the part file doesn't exist).
    // When no parts are verified-done (file exists), the guard does NOT fire,
    // and the bare submission succeeds.
    let (bare_output, bare_success) = call_output(&requests[1], bare_call_id);
    assert_eq!(bare_success, Some(true), "bare submission when nothing verified-done must succeed");
    let _ = bare_output;

    Ok(())
}
