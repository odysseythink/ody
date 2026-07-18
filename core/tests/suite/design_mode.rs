//! End-to-end Design mode integration tests (Phase D9).
use anyhow::Result;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_once;
use core_test_support::responses::mount_sse_sequence;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_ody::local_selections;
use core_test_support::test_ody::test_ody;
use core_test_support::test_ody::turn_permission_fields;
use core_test_support::wait_for_event;
use core_test_support::wait_for_event_match;
use ody_protocol::config_types::CollaborationMode;
use ody_protocol::config_types::DesignAuditLevel;
use ody_protocol::config_types::ModeKind;
use ody_protocol::config_types::Settings;
use ody_protocol::models::PermissionProfile;
use ody_protocol::protocol::AskForApproval;
use ody_protocol::protocol::COLLABORATION_MODE_CLOSE_TAG;
use ody_protocol::protocol::COLLABORATION_MODE_OPEN_TAG;
use ody_protocol::protocol::EventMsg;
use ody_protocol::protocol::Op;
use ody_protocol::user_input::UserInput;
use pretty_assertions::assert_eq;
use serde_json::Value;

fn collab_mode(mode: ModeKind, instructions: Option<&str>) -> CollaborationMode {
    CollaborationMode {
        mode,
        settings: Settings {
            model: "gpt-5.4".to_string(),
            reasoning_effort: None,
            developer_instructions: instructions.map(str::to_string),
            design_audit_level: None,
        },
    }
}

fn design_preset() -> CollaborationMode {
    collab_mode(ModeKind::Design, None)
}

fn plan_preset() -> CollaborationMode {
    collab_mode(ModeKind::Plan, None)
}

fn developer_texts(input: &[Value]) -> Vec<String> {
    input
        .iter()
        .filter(|item| item.get("role").and_then(Value::as_str) == Some("developer"))
        .filter_map(|item| item.get("content")?.as_array().cloned())
        .flatten()
        .filter_map(|content| {
            let text = content.get("text")?.as_str()?;
            Some(text.to_string())
        })
        .collect()
}

fn collab_xml(text: &str) -> String {
    format!("{COLLABORATION_MODE_OPEN_TAG}{text}{COLLABORATION_MODE_CLOSE_TAG}")
}

fn count_messages_containing(texts: &[String], target: &str) -> usize {
    texts.iter().filter(|text| text.contains(target)).count()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn design_mode_includes_design_instructions() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let req = mount_sse_once(
        &server,
        sse(vec![ev_response_created("resp-1"), ev_completed("resp-1")]),
    )
    .await;

    let test = test_ody().build(&server).await?;

    test.ody
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "hello".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: ody_protocol::protocol::ThreadSettingsOverrides {
                environments: Some(local_selections(test.config.cwd.clone())),
                approval_policy: Some(test.config.permissions.approval_policy.value()),
                sandbox_policy: Some(test.config.legacy_sandbox_policy()),
                summary: Some(
                    test.config
                        .model_reasoning_summary
                        .unwrap_or(ody_protocol::config_types::ReasoningSummary::Auto),
                ),
                collaboration_mode: Some(design_preset()),
                ..Default::default()
            },
        })
        .await?;
    wait_for_event(&test.ody, |ev| matches!(ev, EventMsg::TurnComplete(_))).await;

    let input = req.single_request().input();
    let dev_texts = developer_texts(&input);
    let collab_xml = collab_xml("");
    assert_eq!(count_messages_containing(&dev_texts, &collab_xml), 1);
    let combined = dev_texts.join("\n");
    assert!(
        combined.contains("Design Mode"),
        "expected Design Mode instructions"
    );
    assert!(
        combined.contains(".ody-code/designs/"),
        "expected designs directory anchor"
    );
    assert!(
        combined.contains("<HARD-GATE>"),
        "expected HARD-GATE anchor"
    );
    assert!(combined.contains("Step 0"), "expected Step 0 audit gate");

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn design_mode_renders_split_threshold_from_plan_config() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let req = mount_sse_once(
        &server,
        sse(vec![ev_response_created("resp-1"), ev_completed("resp-1")]),
    )
    .await;

    let mut builder = test_ody().with_config(|config| {
        config.plan_mode = Some(ody_config::config_toml::PlanModeConfigToml {
            split_threshold: Some(12),
            ..Default::default()
        });
    });
    let test = builder.build(&server).await?;

    test.ody
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "hello".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: ody_protocol::protocol::ThreadSettingsOverrides {
                environments: Some(local_selections(test.config.cwd.clone())),
                approval_policy: Some(test.config.permissions.approval_policy.value()),
                sandbox_policy: Some(test.config.legacy_sandbox_policy()),
                summary: Some(
                    test.config
                        .model_reasoning_summary
                        .unwrap_or(ody_protocol::config_types::ReasoningSummary::Auto),
                ),
                collaboration_mode: Some(CollaborationMode {
                    mode: ModeKind::Design,
                    settings: Settings {
                        model: "gpt-5.4".to_string(),
                        reasoning_effort: None,
                        developer_instructions: Some(
                            "Split designs larger than {{ split_threshold }} subsystems."
                                .to_string(),
                        ),
                        design_audit_level: None,
                    },
                }),
                ..Default::default()
            },
        })
        .await?;
    wait_for_event(&test.ody, |ev| matches!(ev, EventMsg::TurnComplete(_))).await;

    let input = req.single_request().input();
    let dev_texts = developer_texts(&input);
    let combined = dev_texts.join("\n");
    assert!(
        combined.contains("Split designs larger than 12 subsystems."),
        "split_threshold should be rendered from plan_mode config; got {combined:?}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn design_mode_allows_writing_design_file_and_blocks_other_files() -> Result<()> {
    skip_if_no_network!(Ok(()));

    // This test uses the real file-system/exec path, so it needs a workspace.
    let server = start_mock_server().await;
    let _req = mount_sse_once(
        &server,
        sse(vec![
            ev_response_created("resp-1"),
            // Model asks to write a design file and then a code file.
            ev_response_created("resp-1"),
            ev_completed("resp-1"),
        ]),
    )
    .await;

    let test = test_ody().build(&server).await?;
    let designs_dir = test.config.cwd.join(".ody-code").join("designs");
    std::fs::create_dir_all(&designs_dir)?;

    test.ody
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "write the design to .ody-code/designs/2026-07-11-test.md".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: ody_protocol::protocol::ThreadSettingsOverrides {
                environments: Some(local_selections(test.config.cwd.clone())),
                approval_policy: Some(test.config.permissions.approval_policy.value()),
                sandbox_policy: Some(test.config.legacy_sandbox_policy()),
                summary: Some(
                    test.config
                        .model_reasoning_summary
                        .unwrap_or(ody_protocol::config_types::ReasoningSummary::Auto),
                ),
                collaboration_mode: Some(design_preset()),
                ..Default::default()
            },
        })
        .await?;
    wait_for_event(&test.ody, |ev| matches!(ev, EventMsg::TurnComplete(_))).await;

    // We do not drive the model to actually produce a patch here; the gate is
    // already unit-tested in core. This test documents the integration shape.
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn design_to_plan_handoff_injects_reminder() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let _req1 = mount_sse_once(
        &server,
        sse(vec![ev_response_created("resp-1"), ev_completed("resp-1")]),
    )
    .await;
    let req2 = mount_sse_once(
        &server,
        sse(vec![ev_response_created("resp-2"), ev_completed("resp-2")]),
    )
    .await;

    let test = test_ody().build(&server).await?;

    // Enter Design mode.
    core_test_support::submit_thread_settings(
        &test.ody,
        ody_protocol::protocol::ThreadSettingsOverrides {
            collaboration_mode: Some(design_preset()),
            ..Default::default()
        },
    )
    .await?;

    test.ody
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "hello in design".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: Default::default(),
        })
        .await?;
    wait_for_event(&test.ody, |ev| matches!(ev, EventMsg::TurnComplete(_))).await;

    // Switch to Plan mode.  Because no design artifact has been written, the
    // strict enforcement gate will veto and keep the mode Design.  We then
    // pre-seed a design file on disk and re-attempt the switch.
    let designs_dir = test.config.cwd.join(".ody-code").join("designs");
    std::fs::create_dir_all(&designs_dir)?;
    let design_path = designs_dir.join("2026-07-11-test.md");
    let complete_design = concat!(
        "# Feature Design\n\n",
        "## Scope\n",
        "In scope: core behaviour. Out of scope: UI polish.\n\n",
        "## Architecture\n",
        "Reuse the existing pipeline and add a stage.\n\n",
        "## Data Models\n",
        "struct DesignState { sections: Vec<String> }\n\n",
        "## Algorithms\n",
        "implementation notes: walk the sections and tally coverage.\n\n",
        "## Error Handling\n",
        "failure scenarios and graceful degradation handled inline.\n\n",
        "## Self-Review\n",
        "audit checklist reviewed against the rubric.\n\n",
        "## User Approval\n",
        "user final approval captured before handoff.\n\n",
        "## Reuse Analysis\n",
        "component reuse survey of existing components follows.\n",
    );
    std::fs::write(&design_path, complete_design)?;

    core_test_support::submit_thread_settings(
        &test.ody,
        ody_protocol::protocol::ThreadSettingsOverrides {
            collaboration_mode: Some(plan_preset()),
            ..Default::default()
        },
    )
    .await?;

    test.ody
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "hello in plan after handoff".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: Default::default(),
        })
        .await?;
    wait_for_event(&test.ody, |ev| matches!(ev, EventMsg::TurnComplete(_))).await;

    let input = req2.single_request().input();
    let dev_texts = developer_texts(&input);
    let combined = dev_texts.join("\n");
    assert!(
        combined.contains("handed off"),
        "expected handoff reminder; got {combined:?}"
    );
    assert!(combined.contains("designs"), "expected design path mention");

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn design_mode_injects_selected_audit_level() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let req = mount_sse_once(
        &server,
        sse(vec![ev_response_created("resp-1"), ev_completed("resp-1")]),
    )
    .await;

    let mut builder = test_ody().with_config(|config| {
        config.plan_mode = Some(ody_config::config_toml::PlanModeConfigToml {
            design_audit_level: Some(ody_protocol::config_types::DesignAuditLevel::Standard),
            ..Default::default()
        });
    });
    let test = builder.build(&server).await?;

    test.ody
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "hello".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: ody_protocol::protocol::ThreadSettingsOverrides {
                environments: Some(local_selections(test.config.cwd.clone())),
                approval_policy: Some(test.config.permissions.approval_policy.value()),
                sandbox_policy: Some(test.config.legacy_sandbox_policy()),
                summary: Some(
                    test.config
                        .model_reasoning_summary
                        .unwrap_or(ody_protocol::config_types::ReasoningSummary::Auto),
                ),
                collaboration_mode: Some(CollaborationMode {
                    mode: ModeKind::Design,
                    settings: Settings {
                        model: "gpt-5.4".to_string(),
                        reasoning_effort: None,
                        developer_instructions: None,
                        design_audit_level: Some(DesignAuditLevel::Standard),
                    },
                }),
                ..Default::default()
            },
        })
        .await?;
    wait_for_event(&test.ody, |ev| matches!(ev, EventMsg::TurnComplete(_))).await;

    let input = req.single_request().input();
    let dev_texts = developer_texts(&input);
    let combined = dev_texts.join("\n");
    assert!(
        combined.contains("Audit level: Standard"),
        "expected audit level injection; got {combined:?}"
    );
    assert!(
        combined.contains("Do NOT ask the user to choose the audit level again"),
        "expected host-managed instruction; got {combined:?}"
    );

    Ok(())
}

fn call_output(
    req: &core_test_support::responses::ResponsesRequest,
    call_id: &str,
) -> (String, Option<bool>) {
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
        "component reuse survey of existing components follows.",
    )
    .to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn submit_design_final_triggers_adversarial_review_and_appends_findings() -> anyhow::Result<()>
{
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;

    // First response: assistant calls submit_design(final: true).
    let call_id = "submit-design-final";
    let design_markdown = complete_design_markdown();
    let args = serde_json::json!({"design": design_markdown, "final": true}).to_string();
    let first_response = sse(vec![
        ev_response_created("resp-1"),
        ev_function_call(call_id, "submit_design", &args),
        ev_completed("resp-1"),
    ]);

    // Second response: review model returns structured JSON findings.
    let review_json = serde_json::json!({
        "overall_assessment": "One high-confidence gap.",
        "findings": [
            {
                "severity": "high",
                "confidence": "high",
                "title": "Missing operational runbook",
                "detail": "The design does not describe on-call procedures.",
                "location": "## Error Handling",
                "suggested_fix": "Add a runbook section."
            }
        ]
    })
    .to_string();
    let second_response = sse(vec![
        ev_response_created("resp-2"),
        ev_assistant_message("msg-review", &review_json),
        ev_completed("resp-2"),
    ]);

    let response_mock = mount_sse_sequence(&server, vec![first_response, second_response]).await;

    let mut builder = test_ody().with_config(|config| {
        config.review_model = Some("review-model".to_string());
    });
    let test = builder.build(&server).await?;

    let (sandbox_policy, permission_profile) =
        turn_permission_fields(PermissionProfile::Disabled, test.cwd.path());

    test.ody
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "please finalize the design".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: ody_protocol::protocol::ThreadSettingsOverrides {
                environments: Some(local_selections(test.config.cwd.clone())),
                approval_policy: Some(AskForApproval::Never),
                sandbox_policy: Some(sandbox_policy),
                permission_profile,
                collaboration_mode: Some(CollaborationMode {
                    mode: ModeKind::Design,
                    settings: Settings {
                        model: test.session_configured.model.clone(),
                        reasoning_effort: None,
                        developer_instructions: None,
                        design_audit_level: None,
                    },
                }),
                ..Default::default()
            },
        })
        .await?;

    let _completed = wait_for_event_match(&test.ody, |event| match event {
        EventMsg::ItemCompleted(_) => Some(()),
        _ => None,
    })
    .await;

    let requests = response_mock.requests();
    assert_eq!(
        requests.len(),
        2,
        "final design submission with review_model configured must trigger a review sub-session request"
    );

    // First request is the main turn; second request is the review sub-session.
    let review_request = &requests[1];
    let review_body = review_request.body_json();
    let review_input = review_body["input"].as_array().expect("review input array");
    let review_texts: Vec<String> = review_input
        .iter()
        .filter_map(|item| item.get("content").and_then(|c| c.as_array()).cloned())
        .flatten()
        .filter_map(|entry| entry.get("text").and_then(Value::as_str).map(str::to_owned))
        .collect();
    let combined_review_text = review_texts.join("\n");
    assert!(
        combined_review_text.contains("BREAK the design"),
        "review prompt must ask the model to break the design"
    );
    assert!(
        combined_review_text.contains(&design_markdown),
        "review prompt must include the design markdown"
    );
    assert!(
        combined_review_text.contains("Output strictly as JSON"),
        "review prompt must demand JSON output"
    );

    // Tool output contains the findings appendix.
    let first_req = &requests[0];
    let (output_text, success) = call_output(first_req, call_id);
    assert_eq!(success, Some(true));
    assert!(
        output_text.contains("## Adversarial design review findings"),
        "tool output must include the review findings appendix: {output_text}"
    );
    assert!(
        output_text.contains("[High] Missing operational runbook"),
        "tool output must include the finding title: {output_text}"
    );
    assert!(
        output_text.contains("advisory and do not block"),
        "tool output must include the advisory disclaimer: {output_text}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn submit_design_final_uses_design_review_model_over_review_model() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;

    let call_id = "submit-design-final";
    let design_markdown = complete_design_markdown();
    let args = serde_json::json!({"design": design_markdown, "final": true}).to_string();
    let first_response = sse(vec![
        ev_response_created("resp-1"),
        ev_function_call(call_id, "submit_design", &args),
        ev_completed("resp-1"),
    ]);

    let review_json = serde_json::json!({
        "overall_assessment": "No issues.",
        "findings": []
    })
    .to_string();
    let second_response = sse(vec![
        ev_response_created("resp-2"),
        ev_assistant_message("msg-review", &review_json),
        ev_completed("resp-2"),
    ]);

    let response_mock = mount_sse_sequence(&server, vec![first_response, second_response]).await;

    let mut builder = test_ody().with_config(|config| {
        config.review_model = Some("review-model".to_string());
        config.design_review_model = Some("design-review-model".to_string());
    });
    let test = builder.build(&server).await?;

    let (sandbox_policy, permission_profile) =
        turn_permission_fields(PermissionProfile::Disabled, test.cwd.path());

    test.ody
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "please finalize the design".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: ody_protocol::protocol::ThreadSettingsOverrides {
                environments: Some(local_selections(test.config.cwd.clone())),
                approval_policy: Some(AskForApproval::Never),
                sandbox_policy: Some(sandbox_policy),
                permission_profile,
                collaboration_mode: Some(CollaborationMode {
                    mode: ModeKind::Design,
                    settings: Settings {
                        model: test.session_configured.model.clone(),
                        reasoning_effort: None,
                        developer_instructions: None,
                        design_audit_level: None,
                    },
                }),
                ..Default::default()
            },
        })
        .await?;

    let _completed = wait_for_event_match(&test.ody, |event| match event {
        EventMsg::ItemCompleted(_) => Some(()),
        _ => None,
    })
    .await;

    let requests = response_mock.requests();
    assert_eq!(
        requests.len(),
        2,
        "final design submission with design_review_model configured must trigger a review sub-session request"
    );

    let review_request = &requests[1];
    let review_body = review_request.body_json();
    assert_eq!(
        review_body["model"].as_str().unwrap(),
        "design-review-model",
        "design review must use design_review_model, not review_model"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn submit_design_final_skips_review_when_review_model_unset() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let call_id = "submit-design-no-review";
    let design_markdown = complete_design_markdown();
    let args = serde_json::json!({"design": design_markdown, "final": true}).to_string();
    let response = sse(vec![
        ev_response_created("resp-1"),
        ev_function_call(call_id, "submit_design", &args),
        ev_completed("resp-1"),
    ]);
    let mock = mount_sse_once(&server, response).await;

    let test = test_ody().build(&server).await?;

    let (sandbox_policy, permission_profile) =
        turn_permission_fields(PermissionProfile::Disabled, test.cwd.path());

    test.ody
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "please finalize the design".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: ody_protocol::protocol::ThreadSettingsOverrides {
                environments: Some(local_selections(test.config.cwd.clone())),
                approval_policy: Some(AskForApproval::Never),
                sandbox_policy: Some(sandbox_policy),
                permission_profile,
                collaboration_mode: Some(design_preset()),
                ..Default::default()
            },
        })
        .await?;

    let _completed = wait_for_event_match(&test.ody, |event| match event {
        EventMsg::ItemCompleted(_) => Some(()),
        _ => None,
    })
    .await;

    let req = mock.single_request();
    let (output_text, success) = call_output(&req, call_id);
    assert_eq!(success, Some(true));
    assert_eq!(output_text, "Design submitted");
    assert!(
        !output_text.contains("Adversarial design review findings"),
        "no review should be triggered when review_model is unset"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn submit_design_final_review_fail_open_on_unparseable_output() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;

    let call_id = "submit-design-review-fallback";
    let design_markdown = complete_design_markdown();
    let args = serde_json::json!({"design": design_markdown, "final": true}).to_string();
    let first_response = sse(vec![
        ev_response_created("resp-1"),
        ev_function_call(call_id, "submit_design", &args),
        ev_completed("resp-1"),
    ]);

    // Review model returns prose instead of JSON.
    let second_response = sse(vec![
        ev_response_created("resp-2"),
        ev_assistant_message("msg-review", "This looks fine to me."),
        ev_completed("resp-2"),
    ]);

    let response_mock = mount_sse_sequence(&server, vec![first_response, second_response]).await;

    let mut builder = test_ody().with_config(|config| {
        config.review_model = Some("review-model".to_string());
    });
    let test = builder.build(&server).await?;

    let (sandbox_policy, permission_profile) =
        turn_permission_fields(PermissionProfile::Disabled, test.cwd.path());

    test.ody
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "please finalize the design".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: ody_protocol::protocol::ThreadSettingsOverrides {
                environments: Some(local_selections(test.config.cwd.clone())),
                approval_policy: Some(AskForApproval::Never),
                sandbox_policy: Some(sandbox_policy),
                permission_profile,
                collaboration_mode: Some(CollaborationMode {
                    mode: ModeKind::Design,
                    settings: Settings {
                        model: test.session_configured.model.clone(),
                        reasoning_effort: None,
                        developer_instructions: None,
                        design_audit_level: None,
                    },
                }),
                ..Default::default()
            },
        })
        .await?;

    let _completed = wait_for_event_match(&test.ody, |event| match event {
        EventMsg::ItemCompleted(_) => Some(()),
        _ => None,
    })
    .await;

    let requests = response_mock.requests();
    assert_eq!(requests.len(), 2);

    let first_req = &requests[0];
    let (output_text, success) = call_output(first_req, call_id);
    assert_eq!(success, Some(true));
    assert!(
        output_text.contains("Design submitted"),
        "submit_design must still succeed when review output is unparseable: {output_text}"
    );
    assert!(
        output_text.contains("Review output could not be structured"),
        "tool output must include the fallback finding: {output_text}"
    );
    assert!(
        output_text.contains("This looks fine to me."),
        "fallback finding must quote the raw review output: {output_text}"
    );

    Ok(())
}
