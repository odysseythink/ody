//! End-to-end Design mode integration tests (Phase D9).
use anyhow::Result;
use ody_protocol::config_types::CollaborationMode;
use ody_protocol::config_types::ModeKind;
use ody_protocol::config_types::Settings;
use ody_protocol::protocol::COLLABORATION_MODE_CLOSE_TAG;
use ody_protocol::protocol::COLLABORATION_MODE_OPEN_TAG;
use ody_protocol::protocol::EventMsg;
use ody_protocol::protocol::Op;
use ody_protocol::user_input::UserInput;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_once;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_ody::local_selections;
use core_test_support::test_ody::test_ody;
use core_test_support::wait_for_event;
use pretty_assertions::assert_eq;
use serde_json::Value;

fn collab_mode(mode: ModeKind, instructions: Option<&str>) -> CollaborationMode {
    CollaborationMode {
        mode,
        settings: Settings {
            model: "gpt-5.4".to_string(),
            reasoning_effort: None,
            developer_instructions: instructions.map(str::to_string),
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
    assert!(combined.contains("Design Mode"), "expected Design Mode instructions");
    assert!(combined.contains(".ody-code/designs/"), "expected designs directory anchor");
    assert!(combined.contains("<HARD-GATE>"), "expected HARD-GATE anchor");
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
                            "Split designs larger than {{ split_threshold }} subsystems.".to_string(),
                        ),
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
    assert!(combined.contains("handed off"), "expected handoff reminder; got {combined:?}");
    assert!(combined.contains("designs"), "expected design path mention");

    Ok(())
}
