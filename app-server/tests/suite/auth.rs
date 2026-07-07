use anyhow::Result;
use app_test_support::TestAppServer;
use app_test_support::to_response;
use ody_app_server_protocol::AuthState;
use ody_app_server_protocol::AuthMode;
use ody_app_server_protocol::GetAuthStateParams;
use ody_app_server_protocol::GetAuthStateResponse;
use ody_app_server_protocol::GetAuthStatusParams;
use ody_app_server_protocol::GetAuthStatusResponse;
use ody_app_server_protocol::JSONRPCResponse;
use ody_app_server_protocol::LoginResponse;
use ody_app_server_protocol::RequestId;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::time::timeout;

// Bazel CI can spend tens of seconds starting app-server subprocesses or
// processing auth RPCs under load.
const DEFAULT_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

fn create_config_toml_custom_provider(
    ody_home: &std::path::Path,
) -> std::io::Result<()> {
    let config_toml = ody_home.join("config.toml");
    let requires_line = "";
    let contents = format!(
        r#"
model = "mock-model"
approval_policy = "never"
sandbox_mode = "danger-full-access"

model_provider = "mock_provider"

[features]
shell_snapshot = false

[model_providers.mock_provider]
name = "Mock provider for test"
base_url = "http://127.0.0.1:0/v1"
wire_api = "responses"
request_max_retries = 0
stream_max_retries = 0
{requires_line}
"#
    );
    std::fs::write(config_toml, contents)
}

fn create_config_toml(ody_home: &std::path::Path) -> std::io::Result<()> {
    let config_toml = ody_home.join("config.toml");
    std::fs::write(
        config_toml,
        r#"
model = "mock-model"
approval_policy = "never"
sandbox_mode = "danger-full-access"

[features]
shell_snapshot = false
"#,
    )
}

fn create_config_toml_forced_login(
    ody_home: &std::path::Path,
    forced_method: &str,
) -> std::io::Result<()> {
    let config_toml = ody_home.join("config.toml");
    let contents = format!(
        r#"
model = "mock-model"
approval_policy = "never"
sandbox_mode = "danger-full-access"
forced_login_method = "{forced_method}"

[features]
shell_snapshot = false
"#
    );
    std::fs::write(config_toml, contents)
}

async fn login_with_api_key_via_request(mcp: &mut TestAppServer, api_key: &str) -> Result<()> {
    let request_id = mcp.send_login_account_api_key_request(api_key).await?;

    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let response: LoginResponse = to_response(resp)?;
    assert_eq!(response, LoginResponse::ApiKey {});
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_auth_status_no_auth() -> Result<()> {
    let ody_home = TempDir::new()?;
    create_config_toml(ody_home.path())?;

    let mut mcp =
        TestAppServer::new_with_env(ody_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_get_auth_status_request(GetAuthStatusParams {
            include_token: Some(true),
            refresh_token: Some(false),
        })
        .await?;

    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let status: GetAuthStatusResponse = to_response(resp)?;
    assert_eq!(status.auth_method, None, "expected no auth method");
    assert_eq!(status.auth_token, None, "expected no token");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_auth_status_with_api_key() -> Result<()> {
    let ody_home = TempDir::new()?;
    create_config_toml(ody_home.path())?;

    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    login_with_api_key_via_request(&mut mcp, "sk-test-key").await?;

    let request_id = mcp
        .send_get_auth_status_request(GetAuthStatusParams {
            include_token: Some(true),
            refresh_token: Some(false),
        })
        .await?;

    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let status: GetAuthStatusResponse = to_response(resp)?;
    assert_eq!(status.auth_method, Some(AuthMode::ApiKey));
    assert_eq!(status.auth_token, Some("sk-test-key".to_string()));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_key_supports_auth_status_and_account_read() -> Result<()> {
    let ody_home = TempDir::new()?;
    create_config_toml(ody_home.path())?;

    let mut mcp = TestAppServer::new_with_env(ody_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    login_with_api_key_via_request(&mut mcp, "sk-test-key").await?;

    let request_id = mcp
        .send_get_auth_status_request(GetAuthStatusParams {
            include_token: Some(true),
            refresh_token: Some(false),
        })
        .await?;

    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let status: GetAuthStatusResponse = to_response(resp)?;
    assert_eq!(
        status,
        GetAuthStatusResponse {
            auth_method: Some(AuthMode::ApiKey),
            auth_token: Some("sk-test-key".to_string()),
        }
    );

    let request_id = mcp
        .send_get_account_request(GetAuthStateParams {
            refresh_token: false,
        })
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    assert_eq!(
        to_response::<GetAuthStateResponse>(response)?,
        GetAuthStateResponse {
            account: Some(AuthState::ApiKey {}),
        }
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_auth_status_with_api_key_when_auth_not_required() -> Result<()> {
    let ody_home = TempDir::new()?;
    create_config_toml_custom_provider(ody_home.path())?;

    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    login_with_api_key_via_request(&mut mcp, "sk-test-key").await?;

    let request_id = mcp
        .send_get_auth_status_request(GetAuthStatusParams {
            include_token: Some(true),
            refresh_token: Some(false),
        })
        .await?;

    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let status: GetAuthStatusResponse = to_response(resp)?;
    assert_eq!(status.auth_method, None, "expected no auth method");
    assert_eq!(status.auth_token, None, "expected no token");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_auth_status_with_api_key_no_include_token() -> Result<()> {
    let ody_home = TempDir::new()?;
    create_config_toml(ody_home.path())?;

    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    login_with_api_key_via_request(&mut mcp, "sk-test-key").await?;

    // Build params via struct so None field is omitted in wire JSON.
    let params = GetAuthStatusParams {
        include_token: None,
        refresh_token: Some(false),
    };
    let request_id = mcp.send_get_auth_status_request(params).await?;

    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let status: GetAuthStatusResponse = to_response(resp)?;
    assert_eq!(status.auth_method, Some(AuthMode::ApiKey));
    assert!(status.auth_token.is_none(), "token must be omitted");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_auth_status_with_api_key_refresh_requested() -> Result<()> {
    let ody_home = TempDir::new()?;
    create_config_toml(ody_home.path())?;

    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    login_with_api_key_via_request(&mut mcp, "sk-test-key").await?;

    let request_id = mcp
        .send_get_auth_status_request(GetAuthStatusParams {
            include_token: Some(true),
            refresh_token: Some(true),
        })
        .await?;

    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let status: GetAuthStatusResponse = to_response(resp)?;
    assert_eq!(
        status,
        GetAuthStatusResponse {
            auth_method: Some(AuthMode::ApiKey),
            auth_token: Some("sk-test-key".to_string()),
        }
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_api_key_succeeds_when_forced_api() -> Result<()> {
    let ody_home = TempDir::new()?;
    create_config_toml_forced_login(ody_home.path(), "api")?;

    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_login_account_api_key_request("sk-test-key")
        .await?;

    let resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let response: LoginResponse = to_response(resp)?;
    assert_eq!(response, LoginResponse::ApiKey {});
    Ok(())
}
