use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::Result;
use app_test_support::TestAppServer;
use app_test_support::to_response;
use axum::Router;
use core_test_support::responses;
use ody_app_server::in_process;
use ody_app_server::in_process::InProcessStartArgs;
use ody_app_server_protocol::ClientInfo;
use ody_app_server_protocol::ClientRequest;
use ody_app_server_protocol::InitializeParams;
use ody_app_server_protocol::JSONRPCResponse;
use ody_app_server_protocol::McpResourceReadParams;
use ody_app_server_protocol::RequestId;
use ody_app_server_protocol::ThreadStartParams;
use ody_app_server_protocol::ThreadStartResponse;
use ody_app_server_protocol::TurnStartParams;
use ody_app_server_protocol::UserInput;
use ody_arg0::Arg0DispatchPaths;
use ody_config::CloudConfigBundleLoader;
use ody_config::LoaderOverrides;
use ody_core::config::ConfigBuilder;
use ody_exec_server::EnvironmentManager;
use ody_feedback::OdyFeedback;
use ody_protocol::protocol::SessionSource;
use pretty_assertions::assert_eq;
use rmcp::handler::server::ServerHandler;
use rmcp::model::ListResourcesResult;
use rmcp::model::Meta;
use rmcp::model::PaginatedRequestParams;
use rmcp::model::ProtocolVersion;
use rmcp::model::RawResource;
use rmcp::model::ReadResourceRequestParams;
use rmcp::model::ReadResourceResult;
use rmcp::model::Resource;
use rmcp::model::ResourceContents;
use rmcp::model::ServerCapabilities;
use rmcp::model::ServerInfo;
use rmcp::service::RequestContext;
use rmcp::service::RoleServer;
use rmcp::transport::StreamableHttpServerConfig;
use rmcp::transport::StreamableHttpService;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use serde_json::json;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio::time::timeout;

const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(60);
const SKILL_NAME: &str = "demo-plugin:deploy";
const RAW_SKILL_DESCRIPTION: &str = "Deploy\nthrough the <hosted> orchestrator.";
const SKILL_RESOURCE_URI: &str = "skill://plugin_demo/deploy";
const SKILL_MAIN_PROMPT_URI: &str = "skill://plugin_demo/deploy/SKILL.md";
const SKILL_REFERENCE_URI: &str = "skill://plugin_demo/deploy/references/deploy.md";
const SKILL_MARKER: &str = "ORCHESTRATOR_SKILL_BODY_MARKER";
const SKILL_CONTENTS: &str = concat!(
    "---\n",
    "name: deploy\n",
    "description: Deploy through the orchestrator.\n",
    "---\n\n",
    "# Deploy\n\n",
    "ORCHESTRATOR_SKILL_BODY_MARKER\n\n",
    "Read the [deployment reference](skill://plugin_demo/deploy/references/deploy.md).\n",
);
const SKILL_REFERENCE_CONTENTS: &str =
    "# Deploy reference\n\nUse the orchestrator deployment API.\n";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_executor_does_not_expose_orchestrator_skills() -> Result<()> {
    let responses_server = responses::start_mock_server().await;
    let (apps_server_url, _apps_server_calls, apps_server_handle) =
        start_resource_apps_mcp_server().await?;
    let responses_server_uri = responses_server.uri();
    let (_ody_home, mut mcp) =
        start_resource_test_app_server(&apps_server_url, &responses_server_uri).await?;

    let thread_start_id = mcp
        .send_thread_start_request(ThreadStartParams {
            model: Some("mock-model".to_string()),
            ..Default::default()
        })
        .await?;
    let thread_start_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(thread_start_id)),
    )
    .await??;
    let ThreadStartResponse { thread, .. } = to_response(thread_start_resp)?;

    let response_mock = responses::mount_sse_once(
        &responses_server,
        responses::sse(vec![
            responses::ev_response_created("resp-no-orchestrator-skill"),
            responses::ev_assistant_message("msg-no-orchestrator-skill", "Done"),
            responses::ev_completed("resp-no-orchestrator-skill"),
        ]),
    )
    .await;
    let turn_start_id = mcp
        .send_turn_start_request(TurnStartParams {
            thread_id: thread.id,
            input: vec![UserInput::Text {
                text: format!("Use ${SKILL_NAME}"),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(turn_start_id)),
    )
    .await??;
    timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("turn/completed"),
    )
    .await??;

    let request = response_mock.single_request();
    assert!(request.tool_by_name("skills", "list").is_none());
    assert!(request.tool_by_name("skills", "read").is_none());
    assert!(
        request
            .message_input_texts("developer")
            .iter()
            .all(|text| !text.contains(SKILL_NAME))
    );
    assert!(
        request
            .message_input_texts("user")
            .iter()
            .all(|text| !text.contains(SKILL_MARKER))
    );

    apps_server_handle.abort();
    let _ = apps_server_handle.await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabled_orchestrator_skills_do_not_expose_skills_namespace() -> Result<()> {
    let responses_server = responses::start_mock_server().await;
    let (apps_server_url, apps_server_calls, apps_server_handle) =
        start_resource_apps_mcp_server().await?;
    let responses_server_uri = responses_server.uri();
    let (_ody_home, mut mcp) = start_resource_test_app_server_with_extra_config(
        &apps_server_url,
        &responses_server_uri,
        r#"
[orchestrator.skills]
enabled = false
"#,
    )
    .await?;

    let thread_start_id = mcp
        .send_thread_start_request(ThreadStartParams {
            model: Some("mock-model".to_string()),
            environments: Some(Vec::new()),
            ..Default::default()
        })
        .await?;
    let thread_start_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(thread_start_id)),
    )
    .await??;
    let ThreadStartResponse { thread, .. } = to_response(thread_start_resp)?;

    let response_mock = responses::mount_sse_once(
        &responses_server,
        responses::sse(vec![
            responses::ev_response_created("resp-disabled-orchestrator-skills"),
            responses::ev_assistant_message("msg-disabled-orchestrator-skills", "Done"),
            responses::ev_completed("resp-disabled-orchestrator-skills"),
        ]),
    )
    .await;
    let turn_start_id = mcp
        .send_turn_start_request(TurnStartParams {
            thread_id: thread.id,
            input: vec![UserInput::Text {
                text: format!("Use ${SKILL_NAME}"),
                text_elements: Vec::new(),
            }],
            ..Default::default()
        })
        .await?;
    timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(turn_start_id)),
    )
    .await??;
    timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_notification_message("turn/completed"),
    )
    .await??;

    let request = response_mock.single_request();
    assert!(request.tool_by_name("skills", "list").is_none());
    assert!(request.tool_by_name("skills", "read").is_none());
    assert!(
        request
            .message_input_texts("developer")
            .iter()
            .all(|text| !text.contains(SKILL_NAME))
    );
    assert!(
        request
            .message_input_texts("user")
            .iter()
            .all(|text| !text.contains(SKILL_MARKER))
    );
    assert_eq!(
        ResourceAppsMcpCallCounts {
            list_resources: 0,
            main_prompt_reads: 0,
            reference_reads: 0,
        },
        apps_server_calls.snapshot()
    );

    apps_server_handle.abort();
    let _ = apps_server_handle.await;
    Ok(())
}

#[tokio::test]
async fn mcp_resource_read_returns_error_for_unknown_thread() -> Result<()> {
    let ody_home = TempDir::new()?;
    let loader_overrides = LoaderOverrides::without_managed_config_for_tests();
    let config = ConfigBuilder::default()
        .ody_home(ody_home.path().to_path_buf())
        .fallback_cwd(Some(ody_home.path().to_path_buf()))
        .loader_overrides(loader_overrides.clone())
        .build()
        .await?;
    // This negative-path test does not need the stdio subprocess; keeping it
    // in-process avoids child-process teardown timing in nextest leak detection.
    let client = in_process::start(InProcessStartArgs {
        arg0_paths: Arg0DispatchPaths::default(),
        config: Arc::new(config),
        cli_overrides: Vec::new(),
        loader_overrides,
        strict_config: false,
        cloud_config_bundle: CloudConfigBundleLoader::default(),
        thread_config_loader: Arc::new(ody_config::NoopThreadConfigLoader),
        feedback: OdyFeedback::new(),
        log_db: None,
        state_db: None,
        environment_manager: Arc::new(EnvironmentManager::default_for_tests()),
        config_warnings: Vec::new(),
        session_source: SessionSource::Cli,
        enable_ody_api_key_env: false,
        initialize: InitializeParams {
            client_info: ClientInfo {
                name: "ody-app-server-tests".to_string(),
                title: None,
                version: "0.1.0".to_string(),
            },
            capabilities: None,
        },
        channel_capacity: in_process::DEFAULT_IN_PROCESS_CHANNEL_CAPACITY,
    })
    .await?;

    let response = client
        .request(ClientRequest::McpResourceRead {
            request_id: RequestId::Integer(1),
            params: McpResourceReadParams {
                thread_id: Some("00000000-0000-4000-8000-000000000000".to_string()),
                server: "ody_apps".to_string(),
                uri: "test://ody/resource".to_string(),
            },
        })
        .await;
    client.shutdown().await?;

    let error = match response? {
        Ok(result) => anyhow::bail!("expected thread-not-found error, got response: {result:?}"),
        Err(error) => error,
    };
    assert!(
        error.message.contains("thread not found"),
        "expected thread-not-found error, got: {error:?}"
    );

    Ok(())
}

async fn start_resource_test_app_server(
    apps_server_url: &str,
    responses_server_uri: &str,
) -> Result<(TempDir, TestAppServer)> {
    start_resource_test_app_server_with_extra_config(apps_server_url, responses_server_uri, "")
        .await
}

async fn start_resource_test_app_server_with_extra_config(
    apps_server_url: &str,
    responses_server_uri: &str,
    extra_config: &str,
) -> Result<(TempDir, TestAppServer)> {
    let ody_home = TempDir::new()?;
    std::fs::write(
        ody_home.path().join("config.toml"),
        format!(
            r#"
model = "mock-model"
approval_policy = "untrusted"
sandbox_mode = "read-only"

model_provider = "mock_provider"
legacy_base_url = "{apps_server_url}"
mcp_oauth_credentials_store = "file"

[features]
apps = true

[skills]
include_instructions = true
{extra_config}

[model_providers.mock_provider]
name = "Mock provider for test"
base_url = "{responses_server_uri}/v1"
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0
"#
        ),
    )?;

    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;
    Ok((ody_home, mcp))
}

async fn start_resource_apps_mcp_server()
-> Result<(String, Arc<ResourceAppsMcpCalls>, JoinHandle<()>)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let apps_server_url = format!("http://{addr}");
    let calls = Arc::new(ResourceAppsMcpCalls::default());
    let server_calls = Arc::clone(&calls);

    let mcp_service = StreamableHttpService::new(
        move || {
            Ok(ResourceAppsMcpServer {
                calls: Arc::clone(&server_calls),
            })
        },
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );
    let router = Router::new().nest_service("/api/ody/ps/mcp", mcp_service);
    let apps_server_handle = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    Ok((apps_server_url, calls, apps_server_handle))
}

#[derive(Debug, Default)]
struct ResourceAppsMcpCalls {
    list_resources: AtomicUsize,
    main_prompt_reads: AtomicUsize,
    reference_reads: AtomicUsize,
}

impl ResourceAppsMcpCalls {
    fn snapshot(&self) -> ResourceAppsMcpCallCounts {
        ResourceAppsMcpCallCounts {
            list_resources: self.list_resources.load(Ordering::Relaxed),
            main_prompt_reads: self.main_prompt_reads.load(Ordering::Relaxed),
            reference_reads: self.reference_reads.load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct ResourceAppsMcpCallCounts {
    list_resources: usize,
    main_prompt_reads: usize,
    reference_reads: usize,
}

#[derive(Clone)]
struct ResourceAppsMcpServer {
    calls: Arc<ResourceAppsMcpCalls>,
}

impl ServerHandler for ResourceAppsMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_resources().build())
            .with_protocol_version(ProtocolVersion::V_2025_06_18)
    }

    async fn list_resources(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, rmcp::ErrorData> {
        self.calls.list_resources.fetch_add(1, Ordering::Relaxed);
        let cursor = request.and_then(|request| request.cursor);
        if cursor.is_none() {
            return Ok(ListResourcesResult {
                resources: vec![skill_resource(
                    "skill://plugin_ignored/ignored",
                    "plugin_ignored/ignored",
                    "Not an MCP skill resource.",
                    "text/plain",
                    "ignored-plugin",
                    "ignored",
                )],
                next_cursor: Some("skills-page".to_string()),
                meta: None,
            });
        }
        if cursor.as_deref() == Some("failing-page") {
            return Err(rmcp::ErrorData::internal_error(
                "simulated later-page failure",
                /*data*/ None,
            ));
        }
        if cursor.as_deref() != Some("skills-page") {
            return Err(rmcp::ErrorData::invalid_params(
                "unexpected resources/list cursor",
                /*data*/ None,
            ));
        }

        Ok(ListResourcesResult {
            resources: vec![skill_resource(
                SKILL_RESOURCE_URI,
                "plugin_demo/deploy",
                RAW_SKILL_DESCRIPTION,
                "mcp/skill",
                "demo-plugin",
                "deploy",
            )],
            next_cursor: Some("failing-page".to_string()),
            meta: None,
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, rmcp::ErrorData> {
        let uri = request.uri;
        if uri == SKILL_MAIN_PROMPT_URI {
            self.calls.main_prompt_reads.fetch_add(1, Ordering::Relaxed);
            return Ok(ReadResourceResult::new(vec![
                ResourceContents::TextResourceContents {
                    uri: SKILL_MAIN_PROMPT_URI.to_string(),
                    mime_type: Some("text/markdown".to_string()),
                    text: SKILL_CONTENTS.to_string(),
                    meta: None,
                },
            ]));
        }
        if uri == SKILL_REFERENCE_URI {
            self.calls.reference_reads.fetch_add(1, Ordering::Relaxed);
            return Ok(ReadResourceResult::new(vec![
                ResourceContents::TextResourceContents {
                    uri: SKILL_REFERENCE_URI.to_string(),
                    mime_type: Some("text/markdown".to_string()),
                    text: SKILL_REFERENCE_CONTENTS.to_string(),
                    meta: None,
                },
            ]));
        }

        Err(rmcp::ErrorData::resource_not_found(
            format!("resource not found: {uri}"),
            None,
        ))
    }
}

fn skill_resource(
    uri: &str,
    name: &str,
    description: &str,
    mime_type: &str,
    plugin_name: &str,
    skill_name: &str,
) -> Resource {
    Resource::new(
        RawResource::new(uri, name)
            .with_description(description)
            .with_mime_type(mime_type)
            .with_meta(skill_resource_meta(plugin_name, skill_name)),
        /*annotations*/ None,
    )
}

fn skill_resource_meta(plugin_name: &str, skill_name: &str) -> Meta {
    Meta(serde_json::Map::from_iter([
        ("plugin_name".to_string(), json!(plugin_name)),
        ("skill_name".to_string(), json!(skill_name)),
    ]))
}
