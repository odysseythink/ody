//! Facade over the code-mode runtime.
//!
//! With the `v8` feature enabled this exposes the real in-process V8-backed
//! runtime. Without it (the default for local dev builds, so V8 is never
//! compiled or linked) the same public API is kept as a stub whose runtime
//! operations return a clear error.

#[cfg(not(feature = "v8"))]
#[path = "service_stub.rs"]
mod imp;
#[cfg(feature = "v8")]
#[path = "service_v8.rs"]
mod imp;

pub use imp::CodeModeService;
pub use imp::InProcessCodeModeSessionProvider;

use ody_code_mode_protocol::CellId;
use ody_code_mode_protocol::CodeModeNestedToolCall;
use ody_code_mode_protocol::CodeModeSessionDelegate;
use ody_code_mode_protocol::NotificationFuture;
use ody_code_mode_protocol::ToolInvocationFuture;
use tokio_util::sync::CancellationToken;

pub struct NoopCodeModeSessionDelegate;

impl CodeModeSessionDelegate for NoopCodeModeSessionDelegate {
    fn invoke_tool<'a>(
        &'a self,
        _invocation: CodeModeNestedToolCall,
        cancellation_token: CancellationToken,
    ) -> ToolInvocationFuture<'a> {
        Box::pin(async move {
            cancellation_token.cancelled().await;
            Err("code mode nested tools are unavailable".to_string())
        })
    }

    fn notify<'a>(
        &'a self,
        _call_id: String,
        _cell_id: CellId,
        _text: String,
        _cancellation_token: CancellationToken,
    ) -> NotificationFuture<'a> {
        Box::pin(async { Ok(()) })
    }

    fn cell_closed(&self, _cell_id: &CellId) {}
}
