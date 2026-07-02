use std::ffi::OsStr;
use std::ffi::OsString;
use std::io::ErrorKind;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use app_test_support::TestAppServer;
use app_test_support::to_response;
use ody_app_server::AppServerRuntimeOptions;
use ody_app_server::AppServerTransport;
use ody_app_server::AppServerWebsocketAuthSettings;
use ody_app_server::PluginStartupTasks;
use ody_app_server::RemoteControlStartupMode;
use ody_app_server::run_main_with_transport_options;
use ody_app_server_protocol::JSONRPCError;
use ody_app_server_protocol::JSONRPCResponse;
use ody_app_server_protocol::RemoteControlClientsListParams;
use ody_app_server_protocol::RemoteControlClientsRevokeParams;
use ody_app_server_protocol::RemoteControlConnectionStatus;
use ody_app_server_protocol::RemoteControlPairingStartParams;
use ody_app_server_protocol::RemoteControlPairingStatusParams;
use ody_app_server_protocol::RemoteControlStatusChangedNotification;
use ody_app_server_protocol::RemoteControlStatusReadResponse;
use ody_app_server_protocol::RequestId;
use ody_arg0::Arg0DispatchPaths;
use ody_config::LoaderOverrides;
use ody_protocol::protocol::SessionSource;
use ody_state::RemoteControlEnrollmentRecord;
use ody_state::StateRuntime;
use ody_utils_cli::CliConfigOverrides;
use pretty_assertions::assert_eq;
use serial_test::serial;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::time::timeout;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const REMOTE_CONTROL_DISABLED_BY_REQUIREMENTS_MESSAGE: &str =
    "remote control is disabled by managed requirements";
const REMOTE_CONTROL_REQUIRES_ACCOUNT_AUTH_MESSAGE: &str =
    "remote control requires account authentication; API key auth is not supported";

struct EnvVarGuard {
    key: &'static str,
    original: Option<OsString>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &OsStr) -> Self {
        let original = std::env::var_os(key);
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, original }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.original {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

async fn assert_remote_control_disabled_by_requirements(
    mcp: &mut TestAppServer,
    request_id: i64,
) -> Result<()> {
    let JSONRPCError { error, .. } = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;
    assert_eq!(error.code, -32600);
    assert_eq!(
        error.message,
        REMOTE_CONTROL_DISABLED_BY_REQUIREMENTS_MESSAGE
    );
    Ok(())
}

async fn assert_remote_control_requires_account_auth(
    mcp: &mut TestAppServer,
    request_id: i64,
) -> Result<()> {
    let JSONRPCError { error, .. } = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;
    assert_eq!(error.code, -32600);
    assert_eq!(error.message, REMOTE_CONTROL_REQUIRES_ACCOUNT_AUTH_MESSAGE);
    Ok(())
}

#[tokio::test]
async fn managed_requirements_reject_all_remote_control_rpcs() -> Result<()> {
    let ody_home = TempDir::new()?;
    std::fs::write(
        ody_home.path().join("requirements.toml"),
        "allow_remote_control = false\n",
    )?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let notification = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_notification_message("remoteControl/status/changed"),
    )
    .await??;
    let status: RemoteControlStatusChangedNotification = serde_json::from_value(
        notification
            .params
            .context("remote-control status notification should include params")?,
    )?;
    assert_eq!(status.status, RemoteControlConnectionStatus::Disabled);
    assert_eq!(status.environment_id, None);

    let request_ids = [
        mcp.send_remote_control_enable_request().await?,
        mcp.send_remote_control_disable_request().await?,
        mcp.send_remote_control_status_read_request().await?,
        mcp.send_remote_control_pairing_start_request(RemoteControlPairingStartParams {
            manual_code: false,
        })
        .await?,
        mcp.send_remote_control_pairing_status_request(RemoteControlPairingStatusParams {
            pairing_code: Some("pairing-code".to_string()),
            manual_pairing_code: None,
        })
        .await?,
        mcp.send_remote_control_clients_list_request(RemoteControlClientsListParams {
            environment_id: "environment-id".to_string(),
            cursor: None,
            limit: None,
            order: None,
        })
        .await?,
        mcp.send_remote_control_clients_revoke_request(RemoteControlClientsRevokeParams {
            environment_id: "environment-id".to_string(),
            client_id: "client-id".to_string(),
        })
        .await?,
    ];

    for request_id in request_ids {
        assert_remote_control_disabled_by_requirements(&mut mcp, request_id).await?;
    }

    Ok(())
}

#[tokio::test]
async fn managed_requirements_allow_remote_control_true_does_not_enable_or_block_it() -> Result<()>
{
    let ody_home = TempDir::new()?;
    std::fs::write(
        ody_home.path().join("requirements.toml"),
        "allow_remote_control = true\n",
    )?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp.send_remote_control_status_read_request().await?;
    let response = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let received: RemoteControlStatusReadResponse = to_response(response)?;
    assert_eq!(received.status, RemoteControlConnectionStatus::Disabled);
    Ok(())
}

#[tokio::test]
#[serial]
async fn explicit_remote_control_startup_fails_when_disabled_by_requirements() -> Result<()> {
    let ody_home = TempDir::new()?;
    std::fs::write(
        ody_home.path().join("requirements.toml"),
        "allow_remote_control = false\n",
    )?;
    let managed_config_path = ody_home.path().join("managed_config.toml");
    let socket_path = ody_home.path().join("app-server.sock");
    let transport =
        AppServerTransport::from_listen_url(&format!("unix://{}", socket_path.display()))?;
    let _ody_home_guard = EnvVarGuard::set("ODY_HOME", ody_home.path().as_os_str());

    let result = timeout(
        STARTUP_TIMEOUT,
        run_main_with_transport_options(
            Arg0DispatchPaths {
                ody_self_exe: Some(std::env::current_exe()?),
                ody_linux_sandbox_exe: None,
                main_execve_wrapper_exe: None,
            },
            CliConfigOverrides::default(),
            LoaderOverrides::with_managed_config_path_for_tests(managed_config_path),
            /*strict_config*/ false,
            /*default_analytics_enabled*/ false,
            transport,
            SessionSource::VSCode,
            AppServerWebsocketAuthSettings::default(),
            AppServerRuntimeOptions {
                plugin_startup_tasks: PluginStartupTasks::Skip,
                remote_control_startup_mode: RemoteControlStartupMode::EnabledEphemeral,
                install_shutdown_signal_handler: false,
            },
        ),
    )
    .await?;
    let err = result.expect_err("managed requirements should reject explicit remote control");
    assert_eq!(err.kind(), ErrorKind::InvalidInput);
    assert_eq!(
        err.to_string(),
        REMOTE_CONTROL_DISABLED_BY_REQUIREMENTS_MESSAGE
    );
    assert!(!socket_path.exists());
    Ok(())
}

#[tokio::test]
async fn listen_off_ignores_persisted_enable_when_disabled_by_requirements() -> Result<()> {
    let ody_home = TempDir::new()?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    std::fs::write(
        ody_home.path().join("requirements.toml"),
        "allow_remote_control = false\n",
    )?;
    let websocket_url = format!(
        "ws://{}/backend-api/wham/remote/control/server",
        listener.local_addr()?
    );
    let state_db =
        StateRuntime::init(ody_home.path().to_path_buf(), "test-provider".to_string()).await?;
    state_db
        .upsert_remote_control_enrollment(&RemoteControlEnrollmentRecord {
            websocket_url: websocket_url.clone(),
            account_id: "account_id".to_string(),
            app_server_client_name: None,
            server_id: "server-id".to_string(),
            environment_id: "environment-id".to_string(),
            server_name: "server-name".to_string(),
            remote_control_enabled: Some(true),
        })
        .await?;

    let mut app_server =
        TestAppServer::new_with_args(ody_home.path(), &["--listen", "off"]).await?;
    let status = timeout(STARTUP_TIMEOUT, app_server.wait_for_exit()).await??;
    assert!(!status.success());
    timeout(Duration::from_millis(100), listener.accept())
        .await
        .expect_err("managed requirements should prevent a remote-control connection");
    assert_eq!(
        state_db
            .get_remote_control_enrollment(
                &websocket_url,
                "account_id",
                /*app_server_client_name*/ None
            )
            .await?
            .context("enrollment should remain persisted")?
            .remote_control_enabled,
        Some(true)
    );
    Ok(())
}

#[tokio::test]
async fn listen_off_exits_without_persisted_remote_control_enable() -> Result<()> {
    for persisted_preference in [None, Some(false)] {
        let ody_home = TempDir::new()?;
        if let Some(remote_control_enabled) = persisted_preference {
            let websocket_url = "ws://127.0.0.1:0/backend-api/wham/remote/control/server".to_string();
            let state_db =
                StateRuntime::init(ody_home.path().to_path_buf(), "test-provider".to_string())
                    .await?;
            state_db
                .upsert_remote_control_enrollment(&RemoteControlEnrollmentRecord {
                    websocket_url,
                    account_id: "account_id".to_string(),
                    app_server_client_name: None,
                    server_id: "server-id".to_string(),
                    environment_id: "environment-id".to_string(),
                    server_name: "server-name".to_string(),
                    remote_control_enabled: Some(remote_control_enabled),
                })
                .await?;
        }

        let mut app_server =
            TestAppServer::new_with_args(ody_home.path(), &["--listen", "off"]).await?;
        let status = timeout(STARTUP_TIMEOUT, app_server.wait_for_exit()).await??;
        assert!(!status.success());
    }
    Ok(())
}

#[tokio::test]
async fn remote_control_disable_requires_account_auth() -> Result<()> {
    let ody_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp.send_remote_control_disable_request().await?;
    assert_remote_control_requires_account_auth(&mut mcp, request_id).await?;
    Ok(())
}

#[tokio::test]
async fn remote_control_enable_requires_account_auth() -> Result<()> {
    let ody_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp.send_remote_control_enable_request().await?;
    assert_remote_control_requires_account_auth(&mut mcp, request_id).await?;
    Ok(())
}

#[tokio::test]
async fn remote_control_status_read_returns_disabled_status() -> Result<()> {
    let ody_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp.send_remote_control_status_read_request().await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let received: RemoteControlStatusReadResponse = to_response(response)?;

    assert_eq!(received.status, RemoteControlConnectionStatus::Disabled);
    assert!(!received.server_name.is_empty());
    assert_eq!(received.environment_id, None);
    assert!(!received.installation_id.is_empty());
    Ok(())
}
