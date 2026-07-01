#![cfg(not(target_os = "windows"))]

use ody_protocol::models::PermissionProfile;
use ody_protocol::protocol::AskForApproval;
use ody_protocol::protocol::EventMsg;
use ody_protocol::protocol::Op;
use ody_protocol::user_input::UserInput;
use core_test_support::responses;
use core_test_support::skip_if_no_network;
use core_test_support::test_ody::TestOdy;
use core_test_support::test_ody::local_selections;
use core_test_support::test_ody::test_ody;
use core_test_support::test_ody::turn_permission_fields;
use core_test_support::wait_for_event;
use pretty_assertions::assert_eq;
use responses::ev_assistant_message;
use responses::ev_completed;
use responses::sse;
use responses::start_mock_server;

const SCHEMA: &str = r#"
{
    "type": "object",
    "properties": {
        "explanation": { "type": "string" },
        "final_answer": { "type": "string" }
    },
    "required": ["explanation", "final_answer"],
    "additionalProperties": false
}
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ody_returns_json_result_for_gpt5() -> anyhow::Result<()> {
    ody_returns_json_result("gpt-5.4".to_string()).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ody_returns_json_result_for_gpt5_ody() -> anyhow::Result<()> {
    ody_returns_json_result("gpt-5.4".to_string()).await
}

async fn ody_returns_json_result(model: String) -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;

    let sse1 = sse(vec![
        ev_assistant_message(
            "m2",
            r#"{"explanation": "explanation", "final_answer": "final_answer"}"#,
        ),
        ev_completed("r1"),
    ]);

    let expected_schema: serde_json::Value = serde_json::from_str(SCHEMA)?;
    let match_json_text_param = move |req: &wiremock::Request| {
        let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap_or_default();
        let Some(text) = body.get("text") else {
            return false;
        };
        let Some(format) = text.get("format") else {
            return false;
        };

        format.get("name") == Some(&serde_json::Value::String("ody_output_schema".into()))
            && format.get("type") == Some(&serde_json::Value::String("json_schema".into()))
            && format.get("strict") == Some(&serde_json::Value::Bool(true))
            && format.get("schema") == Some(&expected_schema)
    };
    responses::mount_sse_once_match(&server, match_json_text_param, sse1).await;

    let TestOdy { ody, config, .. } = test_ody().build(&server).await?;
    let cwd = config.cwd.clone();
    let (sandbox_policy, permission_profile) =
        turn_permission_fields(PermissionProfile::Disabled, cwd.as_path());

    // 1) Normal user input – should hit server once.
    ody
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "hello world".into(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: Some(serde_json::from_str(SCHEMA)?),
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: ody_protocol::protocol::ThreadSettingsOverrides {
                environments: Some(local_selections(cwd)),
                approval_policy: Some(AskForApproval::Never),
                sandbox_policy: Some(sandbox_policy),
                permission_profile,
                collaboration_mode: Some(ody_protocol::config_types::CollaborationMode {
                    mode: ody_protocol::config_types::ModeKind::Default,
                    settings: ody_protocol::config_types::Settings {
                        model,
                        reasoning_effort: None,
                        developer_instructions: None,
                    },
                }),
                ..Default::default()
            },
        })
        .await?;

    let message = wait_for_event(&ody, |ev| matches!(ev, EventMsg::AgentMessage(_))).await;
    if let EventMsg::AgentMessage(message) = message {
        let json: serde_json::Value = serde_json::from_str(&message.message)?;
        assert_eq!(
            json.get("explanation"),
            Some(&serde_json::Value::String("explanation".into()))
        );
        assert_eq!(
            json.get("final_answer"),
            Some(&serde_json::Value::String("final_answer".into()))
        );
    } else {
        anyhow::bail!("expected agent message event");
    }

    Ok(())
}
