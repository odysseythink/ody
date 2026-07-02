use std::time::Duration;

use anyhow::Result;
use app_test_support::TestAppServer;
use ody_app_server_protocol::JSONRPCError;
use ody_app_server_protocol::RequestId;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

#[tokio::test]
async fn external_agent_config_import_is_not_supported() -> Result<()> {
    let ody_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_raw_request(
            "externalAgentConfig/import",
            Some(serde_json::json!({
                "migrationItems": [{
                    "itemType": "CONFIG",
                    "description": "Import config",
                    "cwd": null
                }]
            })),
        )
        .await?;

    let error: JSONRPCError = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(error.error.code, -32603);
    assert_eq!(
        error.error.message,
        "external agent config migration is not supported in this build"
    );

    Ok(())
}
