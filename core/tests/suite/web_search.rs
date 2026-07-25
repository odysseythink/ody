#![allow(clippy::unwrap_used)]

use core_test_support::responses;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_ody::test_ody;
use ody_features::Feature;
use ody_protocol::config_types::WebSearchMode;
use ody_protocol::models::PermissionProfile;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use std::sync::Arc;

fn find_web_search_tool(body: &Value) -> &Value {
    body["tools"]
        .as_array()
        .expect("request body should include tools array")
        .iter()
        .find(|tool| tool.get("type").and_then(Value::as_str) == Some("web_search"))
        .expect("tools should include a web_search tool")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_search_mode_cached_sets_external_web_access_false() {
    skip_if_no_network!();

    let server = start_mock_server().await;
    let sse = responses::sse(vec![
        responses::ev_response_created("resp-1"),
        responses::ev_completed("resp-1"),
    ]);
    let resp_mock = responses::mount_sse_once(&server, sse).await;

    let mut builder = test_ody().with_model("k3").with_config(|config| {
        config
            .web_search_mode
            .set(WebSearchMode::Cached)
            .expect("test web_search_mode should satisfy constraints");
    });
    let test = builder
        .build(&server)
        .await
        .expect("create test Ody conversation");

    test.submit_turn_with_permission_profile(
        "hello cached web search",
        PermissionProfile::read_only(),
    )
    .await
    .expect("submit turn");

    let body = resp_mock.single_request().body_json();
    let tool = find_web_search_tool(&body);
    assert_eq!(
        tool.get("external_web_access").and_then(Value::as_bool),
        Some(false),
        "web_search cached mode should force external_web_access=false"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_search_tool_config_from_config_toml_is_forwarded_to_request() {
    skip_if_no_network!();

    let server = start_mock_server().await;
    let sse = responses::sse(vec![
        responses::ev_response_created("resp-1"),
        responses::ev_completed("resp-1"),
    ]);
    let resp_mock = responses::mount_sse_once(&server, sse).await;

    let home = Arc::new(tempfile::TempDir::new().expect("create ody home"));
    std::fs::write(
        home.path().join("config.toml"),
        r#"web_search = "live"

[tools.web_search]
context_size = "high"
allowed_domains = ["example.com"]
location = { country = "US", city = "New York", timezone = "America/New_York" }
"#,
    )
    .expect("write config.toml");

    let mut builder = test_ody().with_model("kimi-for-coding").with_home(home);
    let test = builder
        .build(&server)
        .await
        .expect("create test Ody conversation");

    test.submit_turn_with_permission_profile(
        "hello configured web search",
        PermissionProfile::Disabled,
    )
    .await
    .expect("submit turn");

    let body = resp_mock.single_request().body_json();
    let tool = find_web_search_tool(&body);
    assert_eq!(
        tool,
        &json!({
            "type": "web_search",
            "external_web_access": true,
            "search_context_size": "high",
            "filters": {
                "allowed_domains": ["example.com"],
            },
            "user_location": {
                "type": "approximate",
                "country": "US",
                "city": "New York",
                "timezone": "America/New_York",
            },
        })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn indexed_web_search_mode_sets_index_gate() {
    skip_if_no_network!();

    let server = start_mock_server().await;
    let sse = responses::sse(vec![
        responses::ev_response_created("resp-1"),
        responses::ev_completed("resp-1"),
    ]);
    let resp_mock = responses::mount_sse_once(&server, sse).await;

    let home = Arc::new(tempfile::TempDir::new().expect("create ody home"));
    std::fs::write(home.path().join("config.toml"), r#"web_search = "indexed""#)
        .expect("write config.toml");

    let mut builder = test_ody().with_model("kimi-for-coding").with_home(home);
    let test = builder
        .build(&server)
        .await
        .expect("create test Ody conversation");

    test.submit_turn_with_permission_profile(
        "hello indexed web search",
        PermissionProfile::Disabled,
    )
    .await
    .expect("submit turn");

    let body = resp_mock.single_request().body_json();
    let tool = find_web_search_tool(&body);
    assert_eq!(
        (
            tool.get("external_web_access").and_then(Value::as_bool),
            tool.get("index_gated_web_access").and_then(Value::as_bool),
        ),
        (Some(true), Some(true))
    );
}
