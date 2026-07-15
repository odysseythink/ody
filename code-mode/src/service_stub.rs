//! Stub used when `ody-code-mode` is compiled without the `v8` feature.
//!
//! The public API matches the real V8-backed implementation so downstream
//! crates compile unchanged; every runtime operation returns a clear error
//! explaining that this build has no JS runtime.

use std::sync::Arc;

use ody_code_mode_protocol::CellId;
use ody_code_mode_protocol::CodeModeSession;
use ody_code_mode_protocol::CodeModeSessionDelegate;
use ody_code_mode_protocol::CodeModeSessionProvider;
use ody_code_mode_protocol::CodeModeSessionProviderFuture;
use ody_code_mode_protocol::CodeModeSessionResultFuture;
use ody_code_mode_protocol::ExecuteRequest;
use ody_code_mode_protocol::ExecuteToPendingOutcome;
use ody_code_mode_protocol::StartedCell;
use ody_code_mode_protocol::WaitOutcome;
use ody_code_mode_protocol::WaitRequest;
use ody_code_mode_protocol::WaitToPendingOutcome;
use ody_code_mode_protocol::WaitToPendingRequest;

use super::NoopCodeModeSessionDelegate;

/// Error returned by every runtime operation in a no-`v8` build.
const V8_DISABLED_ERROR: &str =
    "code mode is unavailable: ody-code-mode was compiled without the `v8` feature";

pub struct CodeModeService;

impl CodeModeService {
    pub fn new() -> Self {
        Self::with_delegate(Arc::new(NoopCodeModeSessionDelegate))
    }

    pub fn with_delegate(_delegate: Arc<dyn CodeModeSessionDelegate>) -> Self {
        Self
    }

    pub async fn execute(&self, _request: ExecuteRequest) -> Result<StartedCell, String> {
        Err(V8_DISABLED_ERROR.to_string())
    }

    pub async fn execute_to_pending(
        &self,
        _request: ExecuteRequest,
    ) -> Result<ExecuteToPendingOutcome, String> {
        Err(V8_DISABLED_ERROR.to_string())
    }

    pub async fn wait(&self, _request: WaitRequest) -> Result<WaitOutcome, String> {
        Err(V8_DISABLED_ERROR.to_string())
    }

    pub async fn terminate(&self, _cell_id: CellId) -> Result<WaitOutcome, String> {
        Err(V8_DISABLED_ERROR.to_string())
    }

    pub async fn wait_to_pending(
        &self,
        _request: WaitToPendingRequest,
    ) -> Result<WaitToPendingOutcome, String> {
        Err(V8_DISABLED_ERROR.to_string())
    }

    pub async fn shutdown(&self) -> Result<(), String> {
        Ok(())
    }
}

impl Default for CodeModeService {
    fn default() -> Self {
        Self::new()
    }
}

impl CodeModeSession for CodeModeService {
    fn is_alive(&self) -> bool {
        true
    }

    fn execute<'a>(
        &'a self,
        request: ExecuteRequest,
    ) -> CodeModeSessionResultFuture<'a, StartedCell> {
        Box::pin(CodeModeService::execute(self, request))
    }

    fn wait<'a>(&'a self, request: WaitRequest) -> CodeModeSessionResultFuture<'a, WaitOutcome> {
        Box::pin(CodeModeService::wait(self, request))
    }

    fn terminate<'a>(&'a self, cell_id: CellId) -> CodeModeSessionResultFuture<'a, WaitOutcome> {
        Box::pin(CodeModeService::terminate(self, cell_id))
    }

    fn shutdown<'a>(&'a self) -> CodeModeSessionResultFuture<'a, ()> {
        Box::pin(CodeModeService::shutdown(self))
    }
}

#[derive(Default)]
pub struct InProcessCodeModeSessionProvider;

impl CodeModeSessionProvider for InProcessCodeModeSessionProvider {
    fn create_session<'a>(
        &'a self,
        delegate: Arc<dyn CodeModeSessionDelegate>,
    ) -> CodeModeSessionProviderFuture<'a> {
        Box::pin(async move {
            let session: Arc<dyn CodeModeSession> =
                Arc::new(CodeModeService::with_delegate(delegate));
            Ok(session)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn execute_request() -> ExecuteRequest {
        ExecuteRequest {
            tool_call_id: "call-1".to_string(),
            enabled_tools: Vec::new(),
            source: "return 1;".to_string(),
            yield_time_ms: None,
            max_output_tokens: None,
        }
    }

    #[tokio::test]
    async fn execute_reports_missing_v8_feature() {
        let service = CodeModeService::new();
        let error = service
            .execute(execute_request())
            .await
            .err()
            .expect("stub must reject execution");
        assert!(
            error.contains("`v8` feature"),
            "error should name the missing feature, got: {error}"
        );
    }

    #[tokio::test]
    async fn session_trait_execute_reports_missing_v8_feature() {
        let service = CodeModeService::new();
        let error = CodeModeSession::execute(&service, execute_request())
            .await
            .err()
            .expect("stub must reject execution through the trait");
        assert!(
            error.contains("`v8` feature"),
            "error should name the missing feature, got: {error}"
        );
    }

    #[tokio::test]
    async fn provider_creates_a_live_session_that_rejects_execution() {
        let provider = InProcessCodeModeSessionProvider;
        let session = provider
            .create_session(Arc::new(NoopCodeModeSessionDelegate))
            .await
            .expect("provider should still create a session");
        assert!(session.is_alive());
        let error = session
            .wait(WaitRequest {
                cell_id: CellId::new("cell-1".to_string()),
                yield_time_ms: 0,
            })
            .await
            .expect_err("stub must reject wait");
        assert!(
            error.contains("`v8` feature"),
            "error should name the missing feature, got: {error}"
        );
    }

    #[tokio::test]
    async fn shutdown_is_a_noop() {
        assert!(CodeModeService::new().shutdown().await.is_ok());
    }
}
