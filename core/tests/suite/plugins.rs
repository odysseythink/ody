#![cfg(not(target_os = "windows"))]
#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use anyhow::Result;
use ody_features::Feature;
use ody_protocol::protocol::EventMsg;
use ody_protocol::protocol::Op;
use core_test_support::apps_test_server::SEARCH_CALENDAR_CREATE_TOOL;
use core_test_support::responses::ResponseMock;
use core_test_support::responses::ResponsesRequest;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::ev_tool_search_call;
use core_test_support::responses::mount_sse_once;
use core_test_support::responses::mount_sse_sequence;
use core_test_support::responses::namespace_child_tool;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::stdio_server_bin;
use core_test_support::test_ody::TestOdy;
use core_test_support::test_ody::test_ody;
use core_test_support::wait_for_event;
use core_test_support::wait_for_mcp_server;
use tempfile::TempDir;
use wiremock::MockServer;

const SAMPLE_PLUGIN_CONFIG_NAME: &str = "sample@test";
const SAMPLE_PLUGIN_DISPLAY_NAME: &str = "sample";
const SAMPLE_PLUGIN_DESCRIPTION: &str = "inspect sample data";
const SAMPLE_PLUGIN_APP_NAMESPACE: &str = "mcp__ody_apps__google_calendar";
const SAMPLE_PLUGIN_MCP_NAMESPACE: &str = "mcp__sample";
const PLUGIN_APP_SEARCH_CALL_ID: &str = "plugin-app-search";
const PLUGIN_MCP_SEARCH_CALL_ID: &str = "plugin-mcp-search";

fn sample_plugin_root(home: &TempDir) -> std::path::PathBuf {
    home.path().join("plugins/cache/test/sample/local")
}

fn write_sample_plugin_manifest_and_config(home: &TempDir) -> std::path::PathBuf {
    let plugin_root = sample_plugin_root(home);
    std::fs::create_dir_all(plugin_root.join(".ody-plugin")).expect("create plugin manifest dir");
    std::fs::write(
        plugin_root.join(".ody-plugin/plugin.json"),
        format!(
            r#"{{"name":"{SAMPLE_PLUGIN_DISPLAY_NAME}","description":"{SAMPLE_PLUGIN_DESCRIPTION}"}}"#
        ),
    )
    .expect("write plugin manifest");
    std::fs::write(
        home.path().join("config.toml"),
        format!(
            "[features]\nplugins = true\n\n[plugins.\"{SAMPLE_PLUGIN_CONFIG_NAME}\"]\nenabled = true\n"
        ),
    )
    .expect("write config");
    plugin_root
}

fn write_plugin_skill_plugin(home: &TempDir) -> std::path::PathBuf {
    let plugin_root = write_sample_plugin_manifest_and_config(home);
    let skill_dir = plugin_root.join("skills/sample-search");
    std::fs::create_dir_all(skill_dir.as_path()).expect("create plugin skill dir");
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\ndescription: inspect sample data\n---\n\n# body\n",
    )
    .expect("write plugin skill");
    skill_dir.join("SKILL.md")
}

fn write_plugin_mcp_plugin(home: &TempDir, command: &str) {
    let plugin_root = write_sample_plugin_manifest_and_config(home);
    std::fs::write(
        plugin_root.join(".mcp.json"),
        format!(
            r#"{{
  "mcpServers": {{
    "sample": {{
      "command": "{command}",
      "cwd": ".",
      "startup_timeout_sec": 60.0
    }}
  }}
}}"#
        ),
    )
    .expect("write plugin mcp config");
}

fn write_plugin_app_plugin(home: &TempDir) {
    write_plugin_app_plugin_with_name(home, "sample");
}

fn write_plugin_app_plugin_with_name(home: &TempDir, app_name: &str) {
    let plugin_root = write_sample_plugin_manifest_and_config(home);
    std::fs::write(
        plugin_root.join(".app.json"),
        format!(
            r#"{{
  "apps": {{
    "{app_name}": {{
      "id": "calendar"
    }}
  }}
}}"#
        ),
    )
    .expect("write plugin app config");
}

async fn build_analytics_plugin_test_ody(
    server: &MockServer,
    ody_home: Arc<TempDir>,
) -> Result<TestOdy> {
    let mut builder = test_ody()
        .with_home(ody_home)
        .with_model("gpt-5.2")
        .with_config(move |config| {
        });
    Ok(builder
        .build(server)
        .await
        .expect("create new conversation"))
}

async fn mount_plugin_tool_search_turn(server: &MockServer) -> ResponseMock {
    mount_sse_sequence(
        server,
        vec![
            sse(vec![
                ev_response_created("resp-1"),
                ev_tool_search_call(
                    PLUGIN_APP_SEARCH_CALL_ID,
                    &serde_json::json!({"query": "create calendar event"}),
                ),
                ev_tool_search_call(
                    PLUGIN_MCP_SEARCH_CALL_ID,
                    &serde_json::json!({"query": "echo"}),
                ),
                ev_completed("resp-1"),
            ]),
            sse(vec![ev_response_created("resp-2"), ev_completed("resp-2")]),
        ],
    )
    .await
}

fn assert_plugin_provenance(tool: &serde_json::Value) {
    let description = tool
        .get("description")
        .and_then(serde_json::Value::as_str)
        .expect("plugin tool description should be present");
    assert!(
        description.contains("This tool is part of plugin `sample`."),
        "expected plugin provenance in tool description: {description:?}"
    );
}

fn searched_plugin_tools(
    request: &ResponsesRequest,
) -> (Option<serde_json::Value>, Option<serde_json::Value>) {
    let app_output = request.tool_search_output(PLUGIN_APP_SEARCH_CALL_ID);
    let mcp_output = request.tool_search_output(PLUGIN_MCP_SEARCH_CALL_ID);
    (
        namespace_child_tool(
            &app_output,
            SAMPLE_PLUGIN_APP_NAMESPACE,
            SEARCH_CALENDAR_CREATE_TOOL,
        )
        .cloned(),
        namespace_child_tool(&mcp_output, SAMPLE_PLUGIN_MCP_NAMESPACE, "echo").cloned(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_plugin_mentions_use_mcp_for_api_key_dual_surface_plugins() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = start_mock_server().await;
    let mock = mount_plugin_tool_search_turn(&server).await;

    let ody_home = Arc::new(TempDir::new()?);
    let rmcp_test_server_bin = match stdio_server_bin() {
        Ok(bin) => bin,
        Err(err) => {
            eprintln!("test_stdio_server binary not available, skipping test: {err}");
            return Ok(());
        }
    };
    write_plugin_skill_plugin(ody_home.as_ref());
    write_plugin_mcp_plugin(ody_home.as_ref(), &rmcp_test_server_bin);
    write_plugin_app_plugin(ody_home.as_ref());

    let mut builder = test_ody()
        .with_home(ody_home)
        .with_config(move |config| {
            config
                .features
                .enable(Feature::Apps)
                .expect("test config should allow feature update");
        });
    let test_ody = builder
        .build(&server)
        .await
        .expect("create new conversation");
    let ody = Arc::clone(&test_ody.ody);
    wait_for_mcp_server(&ody, "sample").await?;

    ody
        .submit(Op::UserInput {
            items: vec![ody_protocol::user_input::UserInput::Mention {
                name: "sample".into(),
                path: format!("plugin://{SAMPLE_PLUGIN_CONFIG_NAME}"),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: Default::default(),
        })
        .await?;
    wait_for_event(&ody, |ev| matches!(ev, EventMsg::TurnComplete(_))).await;

    let requests = mock.requests();
    let request = &requests[0];
    let developer_messages = request.message_input_texts("developer");
    assert!(
        developer_messages
            .iter()
            .any(|text| text.contains("Skills from this plugin")),
        "expected plugin skills guidance: {developer_messages:?}"
    );
    assert!(
        developer_messages
            .iter()
            .any(|text| text.contains("MCP servers from this plugin")),
        "expected visible plugin MCP guidance: {developer_messages:?}"
    );
    assert!(
        !developer_messages
            .iter()
            .any(|text| text.contains("Apps from this plugin")),
        "expected plugin app guidance to be suppressed for API-key auth: {developer_messages:?}"
    );
    assert!(
        request
            .tool_by_name(SAMPLE_PLUGIN_APP_NAMESPACE, SEARCH_CALENDAR_CREATE_TOOL)
            .is_none(),
        "plugin app tool should not leak into the request for API-key auth"
    );
    let (calendar_tool, echo_tool) = searched_plugin_tools(&requests[1]);
    assert!(
        calendar_tool.is_none(),
        "plugin app tool should be hidden for API-key auth"
    );
    let echo_tool = echo_tool.expect("plugin MCP tool should be searchable");
    assert_plugin_provenance(&echo_tool);

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_plugin_mentions_track_plugin_used_analytics() -> Result<()> {
    skip_if_no_network!(Ok(()));
    let server = start_mock_server().await;
    let _resp_mock = mount_sse_once(
        &server,
        sse(vec![ev_response_created("resp-1"), ev_completed("resp-1")]),
    )
    .await;

    let ody_home = Arc::new(TempDir::new()?);
    write_plugin_skill_plugin(ody_home.as_ref());
    let test_ody = build_analytics_plugin_test_ody(&server, ody_home).await?;
    let ody = Arc::clone(&test_ody.ody);

    ody
        .submit(Op::UserInput {
            items: vec![ody_protocol::user_input::UserInput::Mention {
                name: "sample".into(),
                path: format!("plugin://{SAMPLE_PLUGIN_CONFIG_NAME}"),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: Default::default(),
        })
        .await?;
    wait_for_event(&ody, |ev| matches!(ev, EventMsg::TurnComplete(_))).await;

    let deadline = Instant::now() + Duration::from_secs(10);
    let plugin_event = loop {
        let requests = server.received_requests().await.unwrap_or_default();
        if let Some(event) = requests
            .into_iter()
            .filter(|request| request.url.path() == "/ody/analytics-events/events")
            .find_map(|request| {
                let payload: serde_json::Value = serde_json::from_slice(&request.body).ok()?;
                payload["events"].as_array().and_then(|events| {
                    events
                        .iter()
                        .find(|event| event["event_type"] == "ody_plugin_used")
                        .cloned()
                })
            })
        {
            break event;
        }
        if Instant::now() >= deadline {
            panic!("timed out waiting for plugin analytics request");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    let event = plugin_event;
    assert_eq!(event["event_params"]["plugin_id"], "sample@test");
    assert_eq!(event["event_params"]["plugin_name"], "sample");
    assert_eq!(event["event_params"]["marketplace_name"], "test");
    assert_eq!(event["event_params"]["has_skills"], true);
    assert_eq!(event["event_params"]["mcp_server_count"], 0);
    assert_eq!(
        event["event_params"]["mcp_server_names"],
        serde_json::json!([])
    );
    assert_eq!(
        event["event_params"]["connector_ids"],
        serde_json::json!([])
    );
    assert_eq!(
        event["event_params"]["product_client_id"],
        serde_json::json!(ody_client::default_client::originator().value)
    );
    assert_eq!(event["event_params"]["model_slug"], "gpt-5.2");
    assert!(event["event_params"]["thread_id"].as_str().is_some());
    assert!(event["event_params"]["turn_id"].as_str().is_some());

    Ok(())
}
