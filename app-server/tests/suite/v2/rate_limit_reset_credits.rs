use anyhow::Result;
use app_test_support::TestAppServer;
use ody_app_server_protocol::ConsumeRateLimitResetCreditParams;
use ody_app_server_protocol::JSONRPCError;
use ody_app_server_protocol::RequestId;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const INTERNAL_ERROR_CODE: i64 = -32603;

#[tokio::test]
async fn consume_rate_limit_reset_credit_returns_not_supported() -> Result<()> {
    let ody_home = TempDir::new()?;
    let mut mcp = initialized_app_server(ody_home.path()).await?;

    let consume_id = mcp
        .send_consume_account_rate_limit_reset_credit_request(
            ConsumeRateLimitResetCreditParams {
                idempotency_key: "request-1".to_string(),
            },
        )
        .await?;
    let consume_error = read_error_response(&mut mcp, consume_id).await?;
    assert_eq!(consume_error.error.code, INTERNAL_ERROR_CODE);
    assert_eq!(
        consume_error.error.message,
        "rateLimitResetCredit/consume is not supported in this build"
    );
    Ok(())
}

async fn initialized_app_server(ody_home: &std::path::Path) -> Result<TestAppServer> {
    let mut mcp = TestAppServer::new_with_env(ody_home, &[("OPENAI_API_KEY", None)]).await?;
    timeout(DEFAULT_READ_TIMEOUT, mcp.initialize()).await??;
    Ok(mcp)
}

async fn read_error_response(mcp: &mut TestAppServer, request_id: i64) -> Result<JSONRPCError> {
    let error = timeout(
        DEFAULT_READ_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;
    Ok(error)
}
