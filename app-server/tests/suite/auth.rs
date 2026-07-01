use anyhow::Result;
use app_test_support::ApiKeyAuthFixture;
use app_test_support::TestAppServer;
use app_test_support::to_response;
use app_test_support::write_api_key_auth;
use chrono::Duration;
use chrono::Utc;
use ody_app_server_protocol::Account;
use ody_app_server_protocol::AuthMode;
use ody_app_server_protocol::GetAccountParams;
use ody_app_server_protocol::GetAccountResponse;
use ody_app_server_protocol::GetAuthStatusParams;
use ody_app_server_protocol::GetAuthStatusResponse;
use ody_app_server_protocol::JSONRPCError;
use ody_app_server_protocol::JSONRPCResponse;
use ody_app_server_protocol::LoginAccountResponse;
use ody_app_server_protocol::RequestId;
use ody_config::types::AuthCredentialsStoreMode;

use pretty_assertions::assert_eq;
use std::path::Path;
use tempfile::TempDir;
use tokio::time::timeout;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::header;
use wiremock::matchers::method;
use wiremock::matchers::path;

// Bazel CI can spend tens of seconds starting app-server subprocesses or
// processing auth RPCs under load.
const DEFAULT_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

fn create_config_toml_custom_provider(
    ody_home: &Path,
    requires_odysseythink_auth: bool,
) -> std::io::Result<()> {
    let config_toml = ody_home.join("config.toml");
    let requires_line = if requires_odysseythink_auth {
        "requires_odysseythink_auth = true\n"
    } else {
        ""
    };
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

fn create_config_toml(ody_home: &Path) -> std::io::Result<()> {
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

fn create_config_toml_forced_login(ody_home: &Path, forced_method: &str) -> std::io::Result<()> {
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
    let response: LoginAccountResponse = to_response(resp)?;
    assert_eq!(response, LoginAccountResponse::ApiKey {});
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
async fn personal_access_token_without_email_supports_auth_status_and_account_read() -> Result<()> {
    let ody_home = TempDir::new()?;
    create_config_toml(ody_home.path())?;

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/user-auth-credential/whoami"))
        .and(header("Authorization", "Bearer at-test-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "email": null,
            "user_id": "user-123",
            "account_id": "account-123",
            "plan_type": "pro",
            "account_is_compliant": false,
        })))
        .expect(1..)
        .mount(&server)
        .await;

    let authapi_base_url = server.uri();
    let mut mcp = TestAppServer::new_with_env(
        ody_home.path(),
        &[
            ("OPENAI_API_KEY", None),
            ("ODY_ACCESS_TOKEN", Some("at-test-token")),
            ("ODY_AUTHAPI_BASE_URL", Some(authapi_base_url.as_str())),
        ],
    )
    .await?;
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
    assert_eq!(
        status,
        GetAuthStatusResponse {
            auth_method: Some(AuthMode::ApiKey),
            auth_token: None,
            requires_odysseythink_auth: Some(true),
        }
    );

    let request_id = mcp
        .send_get_account_request(GetAccountParams {
            refresh_token: false,
        })
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    assert_eq!(
        response
            .result
            .get("account")
            .and_then(|account| account.get("email")),
        Some(&serde_json::Value::Null),
    );
    assert_eq!(
        to_response::<GetAccountResponse>(response)?,
        GetAccountResponse {
            account: Some(Account::ApiKey {}),
            requires_odysseythink_auth: true,
        }
    );

    server.verify().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_auth_status_with_api_key_when_auth_not_required() -> Result<()> {
    let ody_home = TempDir::new()?;
    create_config_toml_custom_provider(ody_home.path(), /*requires_odysseythink_auth*/ false)?;

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
    assert_eq!(
        status.requires_odysseythink_auth,
        Some(false),
        "requires_odysseythink_auth should be false",
    );
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
            requires_odysseythink_auth: Some(true),
        }
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_auth_status_omits_token_after_permanent_refresh_failure() -> Result<()> {
    let ody_home = TempDir::new()?;
    create_config_toml(ody_home.path())?;
    write_api_key_auth(
        ody_home.path(),
        ApiKeyAuthFixture::new("stale-access-token")
            .refresh_token("stale-refresh-token")
            .account_id("acct_123")
            .email("user@example.com")
            .plan_type("pro"),
        AuthCredentialsStoreMode::File,
    )?;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "error": {
                "code": "refresh_token_reused"
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let refresh_url = format!("{}/oauth/token", server.uri());
    let mut mcp = TestAppServer::new_with_env(
        ody_home.path(),
        &[
            ("OPENAI_API_KEY", None),

        ],
    )
    .await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

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
            auth_token: None,
            requires_odysseythink_auth: Some(true),
        }
    );

    let second_request_id = mcp
        .send_get_auth_status_request(GetAuthStatusParams {
            include_token: Some(true),
            refresh_token: Some(true),
        })
        .await?;

    let second_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(second_request_id)),
    )
    .await??;
    let second_status: GetAuthStatusResponse = to_response(second_resp)?;
    assert_eq!(second_status, status);

    server.verify().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_auth_status_omits_token_after_proactive_refresh_failure() -> Result<()> {
    let ody_home = TempDir::new()?;
    create_config_toml(ody_home.path())?;
    write_api_key_auth(
        ody_home.path(),
        ApiKeyAuthFixture::new("stale-access-token")
            .refresh_token("stale-refresh-token")
            .account_id("acct_123")
            .email("user@example.com")
            .plan_type("pro")
            ,
        AuthCredentialsStoreMode::File,
    )?;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "error": {
                "code": "refresh_token_reused"
            }
        })))
        .expect(2)
        .mount(&server)
        .await;

    let refresh_url = format!("{}/oauth/token", server.uri());
    let mut mcp = TestAppServer::new_with_env(
        ody_home.path(),
        &[
            ("OPENAI_API_KEY", None),

        ],
    )
    .await?;
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
    assert_eq!(
        status,
        GetAuthStatusResponse {
            auth_method: Some(AuthMode::ApiKey),
            auth_token: None,
            requires_odysseythink_auth: Some(true),
        }
    );

    server.verify().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_auth_status_returns_token_after_proactive_refresh_recovery() -> Result<()> {
    let ody_home = TempDir::new()?;
    create_config_toml(ody_home.path())?;
    write_api_key_auth(
        ody_home.path(),
        ApiKeyAuthFixture::new("stale-access-token")
            .refresh_token("stale-refresh-token")
            .account_id("acct_123")
            .email("user@example.com")
            .plan_type("pro")
            ,
        AuthCredentialsStoreMode::File,
    )?;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "error": {
                "code": "refresh_token_reused"
            }
        })))
        .expect(2)
        .mount(&server)
        .await;

    let refresh_url = format!("{}/oauth/token", server.uri());
    let mut mcp = TestAppServer::new_with_env(
        ody_home.path(),
        &[
            ("OPENAI_API_KEY", None),

        ],
    )
    .await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let failed_request_id = mcp
        .send_get_auth_status_request(GetAuthStatusParams {
            include_token: Some(true),
            refresh_token: Some(true),
        })
        .await?;

    let failed_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(failed_request_id)),
    )
    .await??;
    let failed_status: GetAuthStatusResponse = to_response(failed_resp)?;
    assert_eq!(
        failed_status,
        GetAuthStatusResponse {
            auth_method: Some(AuthMode::ApiKey),
            auth_token: None,
            requires_odysseythink_auth: Some(true),
        }
    );

    write_api_key_auth(
        ody_home.path(),
        ApiKeyAuthFixture::new("recovered-access-token")
            .refresh_token("recovered-refresh-token")
            .account_id("acct_123")
            .email("user@example.com")
            .plan_type("pro")
            ,
        AuthCredentialsStoreMode::File,
    )?;

    let recovered_request_id = mcp
        .send_get_auth_status_request(GetAuthStatusParams {
            include_token: Some(true),
            refresh_token: Some(false),
        })
        .await?;

    let recovered_resp: JSONRPCResponse = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(recovered_request_id)),
    )
    .await??;
    let recovered_status: GetAuthStatusResponse = to_response(recovered_resp)?;
    assert_eq!(
        recovered_status,
        GetAuthStatusResponse {
            auth_method: Some(AuthMode::ApiKey),
            auth_token: Some("recovered-access-token".to_string()),
            requires_odysseythink_auth: Some(true),
        }
    );

    server.verify().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn login_api_key_rejected_when_forced_legacy() -> Result<()> {
    let ody_home = TempDir::new()?;
    create_config_toml_forced_login(ody_home.path(), "legacy")?;

    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_login_account_api_key_request("sk-test-key")
        .await?;

    let err: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(
        err.error.message,
        "API key login is disabled. Use Legacy login instead."
    );
    Ok(())
}
