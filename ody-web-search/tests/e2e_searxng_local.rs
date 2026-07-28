//! End-to-end web search test against a local SearXNG instance.
//!
//! Requires a SearXNG server at `http://localhost:9999/search`.
//! Run with:
//!   cargo nextest run -p ody-web-search --test e2e_searxng_local -- --ignored
//! Or to run only the ignored tests:
//!   cargo nextest run -p ody-web-search --test e2e_searxng_local --run-ignored all

use std::collections::HashMap;
use std::sync::Arc;

use ody_protocol::protocol::TruncationPolicy;
use ody_protocol::ToolName;
use ody_tools::{NoopTurnItemEmitter, ToolCall, ToolPayload, ToolExecutor};
use ody_web_search::{
    config::{WebSearchConfig, WebSearchProviderConfig, WebSearchProviderName},
    fallback::FallbackWebSearchProvider,
    http_client::default_http_client,
    provider::{WebSearchOptions, WebSearchProvider},
    providers::create_default_registry,
    tool::WebSearchTool,
};

fn local_searxng_config() -> WebSearchConfig {
    let mut options = HashMap::new();
    options.insert(
        "base_url".to_string(),
        serde_json::Value::String("http://localhost:9999/search".to_string()),
    );
    WebSearchConfig {
        primary: WebSearchProviderConfig {
            provider: WebSearchProviderName::Searxng,
            api_key: None,
            timeout_ms: Some(30000),
            options,
        },
        secondary: None,
    }
}

fn tool_call(arguments: &str) -> ToolCall {
    ToolCall {
        turn_id: "turn-1".to_string(),
        call_id: "call-1".to_string(),
        tool_name: ToolName::plain("WebSearch"),
        model: "test-model".to_string(),
        truncation_policy: TruncationPolicy::Bytes(0),
        conversation_history: ody_tools::ConversationHistory::default(),
        turn_item_emitter: Arc::new(NoopTurnItemEmitter),
        environments: Vec::new(),
        payload: ToolPayload::Function {
            arguments: arguments.to_string(),
        },
    }
}

#[tokio::test]
#[ignore = "requires local SearXNG on localhost:9999"]
async fn searxng_provider_returns_results() {
    let config = local_searxng_config();
    let registry = create_default_registry();
    let client = default_http_client();
    let provider = registry
        .create(&config.primary, client)
        .expect("should create searxng provider");

    let results = provider
        .search("rust programming language", &WebSearchOptions::default())
        .await
        .expect("search should succeed");

    assert!(!results.is_empty(), "expected at least one result");
    let first = &results[0];
    assert!(!first.title.is_empty(), "result title should not be empty");
    assert!(!first.url.is_empty(), "result url should not be empty");
    println!("first result: {} -> {}", first.title, first.url);
}

#[tokio::test]
#[ignore = "requires local SearXNG on localhost:9999"]
async fn web_search_tool_formats_local_searxng_output() {
    let config = local_searxng_config();
    let registry = create_default_registry();
    let client = default_http_client();
    let primary = registry
        .create(&config.primary, client)
        .expect("should create primary provider");
    let provider: Arc<dyn WebSearchProvider> =
        Arc::new(FallbackWebSearchProvider::new(primary, None));
    let tool = WebSearchTool::new("e2e-session".to_string(), provider);

    let output = tool
        .handle(tool_call(
            r#"{"query":"rust programming language","limit":3,"include_content":false}"#,
        ))
        .await
        .expect("tool execution should succeed");

    let value = output.code_mode_result(&ToolPayload::Function {
        arguments: String::new(),
    });
    let text = value["text"].as_str().expect("text field should be a string");
    assert!(
        text.to_lowercase().contains("rust"),
        "output should mention the query: {text}"
    );
    assert!(
        text.contains("http"),
        "output should contain at least one URL: {text}"
    );
    let result_count = value["result_count"].as_u64().expect("result_count should be a number");
    assert!(result_count > 0, "result_count should be greater than 0");
    println!("tool output ({result_count} results):\n{text}");
}

#[tokio::test]
#[ignore = "requires local SearXNG on localhost:9999"]
async fn services_web_search_toml_config_round_trips_to_local_searxng() {
    let toml = r#"
[webSearch]
primary = { provider = "searxng", timeout_ms = 30000, options = { base_url = "http://localhost:9999/search" } }
"#;
    let services: ody_web_search::config::ServicesConfig = toml::from_str(toml)
        .expect("services config should deserialize from TOML");
    let web_search_config = services
        .web_search
        .expect("web_search config should be present");
    assert_eq!(
        web_search_config.primary.provider,
        WebSearchProviderName::Searxng
    );

    let registry = create_default_registry();
    let client = default_http_client();
    let provider = registry
        .create(&web_search_config.primary, client)
        .expect("registry should create searxng provider from config");

    let results = provider
        .search("searxng", &WebSearchOptions::default())
        .await
        .expect("search should succeed");
    assert!(!results.is_empty(), "expected at least one result");
    println!(
        "config round-trip first result: {} -> {}",
        results[0].title, results[0].url
    );
}
