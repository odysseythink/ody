use anyhow::Result;
use app_test_support::TestAppServer;
use ody_app_server_protocol::AddCreditsNudgeCreditType;
use ody_app_server_protocol::JSONRPCError;
use ody_app_server_protocol::RequestId;
use ody_app_server_protocol::SendAddCreditsNudgeEmailParams;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const INTERNAL_ERROR_CODE: i64 = -32603;

#[tokio::test]
async fn get_account_rate_limits_returns_not_supported() -> Result<()> {
    let ody_home = TempDir::new()?;

    let mut mcp =
        TestAppServer::new_with_env(ody_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp.send_get_account_rate_limits_request().await?;

    let error: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(error.id, RequestId::Integer(request_id));
    assert_eq!(error.error.code, INTERNAL_ERROR_CODE);
    assert_eq!(
        error.error.message,
        "account/rateLimits/read is not supported in this build"
    );

    Ok(())
}

#[tokio::test]
async fn send_add_credits_nudge_email_returns_not_supported() -> Result<()> {
    let ody_home = TempDir::new()?;

    let mut mcp =
        TestAppServer::new_with_env(ody_home.path(), &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_add_credits_nudge_email_request(SendAddCreditsNudgeEmailParams {
            credit_type: AddCreditsNudgeCreditType::Credits,
        })
        .await?;

    let error: JSONRPCError = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(error.id, RequestId::Integer(request_id));
    assert_eq!(error.error.code, INTERNAL_ERROR_CODE);
    assert_eq!(
        error.error.message,
        "account/sendAddCreditsNudgeEmail is not supported in this build"
    );

    Ok(())
}
