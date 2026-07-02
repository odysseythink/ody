use std::time::Duration;

use anyhow::Result;
use app_test_support::TestAppServer;
use ody_app_server_protocol::JSONRPCError;
use ody_app_server_protocol::RequestId;
use serde_json::json;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::test]
async fn plugin_share_save_is_no_longer_supported() -> Result<()> {
    let ody_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_raw_request(
            "plugin/share/save",
            Some(json!({
                "pluginPath": ody_home.path().join("demo-plugin"),
            })),
        )
        .await?;
    let error: JSONRPCError = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(error.error.code, -32600);
    assert_eq!(
        error.error.message,
        "plugin/share/save is no longer supported"
    );
    Ok(())
}

#[tokio::test]
async fn plugin_share_update_targets_is_no_longer_supported() -> Result<()> {
    let ody_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_raw_request(
            "plugin/share/updateTargets",
            Some(json!({
                "remotePluginId": "plugins_123",
                "discoverability": "UNLISTED",
                "shareTargets": [],
            })),
        )
        .await?;
    let error: JSONRPCError = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(error.error.code, -32600);
    assert_eq!(
        error.error.message,
        "plugin/share/updateTargets is no longer supported"
    );
    Ok(())
}

#[tokio::test]
async fn plugin_share_list_is_no_longer_supported() -> Result<()> {
    let ody_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_raw_request("plugin/share/list", Some(json!({})))
        .await?;
    let error: JSONRPCError = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(error.error.code, -32600);
    assert_eq!(
        error.error.message,
        "plugin/share/list is no longer supported"
    );
    Ok(())
}

#[tokio::test]
async fn plugin_share_checkout_is_no_longer_supported() -> Result<()> {
    let ody_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_raw_request(
            "plugin/share/checkout",
            Some(json!({
                "remotePluginId": "plugins_123",
            })),
        )
        .await?;
    let error: JSONRPCError = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(error.error.code, -32600);
    assert_eq!(
        error.error.message,
        "plugin/share/checkout is no longer supported"
    );
    Ok(())
}

#[tokio::test]
async fn plugin_share_delete_is_no_longer_supported() -> Result<()> {
    let ody_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_raw_request(
            "plugin/share/delete",
            Some(json!({
                "remotePluginId": "plugins_123",
            })),
        )
        .await?;
    let error: JSONRPCError = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(error.error.code, -32600);
    assert_eq!(
        error.error.message,
        "plugin/share/delete is no longer supported"
    );
    Ok(())
}
