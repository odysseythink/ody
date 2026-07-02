use std::time::Duration;

use anyhow::Result;
use app_test_support::TestAppServer;
use app_test_support::to_response;
use ody_app_server_protocol::JSONRPCResponse;
use ody_app_server_protocol::PluginUninstallParams;
use ody_app_server_protocol::PluginUninstallResponse;
use ody_app_server_protocol::RequestId;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
const REMOTE_PLUGIN_ID: &str = "plugins~Plugin_linear";

#[tokio::test]
async fn plugin_uninstall_removes_plugin_cache_and_config_entry() -> Result<()> {
    let ody_home = TempDir::new()?;
    write_installed_plugin(&ody_home, "debug", "sample-plugin")?;
    std::fs::write(
        ody_home.path().join("config.toml"),
        r#"[features]
plugins = true

[plugins."sample-plugin@debug"]
enabled = true
"#,
    )?;

    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let params = PluginUninstallParams {
        plugin_id: "sample-plugin@debug".to_string(),
    };

    let request_id = mcp.send_plugin_uninstall_request(params.clone()).await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let response: PluginUninstallResponse = to_response(response)?;
    assert_eq!(response, PluginUninstallResponse {});

    assert!(
        !ody_home
            .path()
            .join("plugins/cache/debug/sample-plugin")
            .exists()
    );
    let config = std::fs::read_to_string(ody_home.path().join("config.toml"))?;
    assert!(!config.contains(r#"[plugins."sample-plugin@debug"]"#));

    let request_id = mcp.send_plugin_uninstall_request(params).await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let response: PluginUninstallResponse = to_response(response)?;
    assert_eq!(response, PluginUninstallResponse {});

    Ok(())
}

#[tokio::test]
async fn plugin_uninstall_rejects_remote_plugin_id() -> Result<()> {
    let ody_home = TempDir::new()?;
    std::fs::write(
        ody_home.path().join("config.toml"),
        r#"[features]
plugins = false
"#,
    )?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_uninstall_request(PluginUninstallParams {
            plugin_id: "plugins~Plugin_sample".to_string(),
        })
        .await?;

    let err = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(err.error.code, -32600);
    assert_eq!(
        err.error.message,
        "plugin/uninstall by remote plugin id is no longer supported"
    );
    Ok(())
}

#[tokio::test]
async fn plugin_uninstall_rejects_remote_plugin_id_without_network_call() -> Result<()> {
    let ody_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_uninstall_request(PluginUninstallParams {
            plugin_id: REMOTE_PLUGIN_ID.to_string(),
        })
        .await?;
    let err = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(err.error.code, -32600);
    assert_eq!(
        err.error.message,
        "plugin/uninstall by remote plugin id is no longer supported"
    );
    Ok(())
}

#[tokio::test]
async fn plugin_uninstall_rejects_remote_plugin_id_with_spaces_before_network_call() -> Result<()> {
    let ody_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_uninstall_request(PluginUninstallParams {
            plugin_id: "sample plugin".to_string(),
        })
        .await?;

    let err = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(err.error.code, -32600);
    assert!(err.error.message.contains("invalid remote plugin id"));
    Ok(())
}

#[tokio::test]
async fn plugin_uninstall_rejects_invalid_remote_plugin_id_before_network_call() -> Result<()> {
    let ody_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_uninstall_request(PluginUninstallParams {
            plugin_id: "linear/../../oops".to_string(),
        })
        .await?;

    let err = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(err.error.code, -32600);
    assert!(err.error.message.contains("invalid remote plugin id"));
    Ok(())
}

#[tokio::test]
async fn plugin_uninstall_rejects_empty_remote_plugin_id() -> Result<()> {
    let ody_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_uninstall_request(PluginUninstallParams {
            plugin_id: String::new(),
        })
        .await?;
    let err = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(err.error.code, -32600);
    assert!(err.error.message.contains("invalid remote plugin id"));

    Ok(())
}

fn write_installed_plugin(
    ody_home: &TempDir,
    marketplace_name: &str,
    plugin_name: &str,
) -> Result<()> {
    let plugin_root = ody_home
        .path()
        .join("plugins/cache")
        .join(marketplace_name)
        .join(plugin_name)
        .join("local/.ody-plugin");
    std::fs::create_dir_all(&plugin_root)?;
    std::fs::write(
        plugin_root.join("plugin.json"),
        format!(r#"{{"name":"{plugin_name}"}}"#),
    )?;
    Ok(())
}
