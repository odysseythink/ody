use std::future::Future;
use std::pin::Pin;

use serde_json::Value;

/// Future returned by one host-side flow-run invocation.
pub type FlowRunFuture<'a> =
    Pin<Box<dyn Future<Output = Result<FlowRunOutput, FlowRunError>> + Send + 'a>>;

/// Constructor/store-injected host executor for model-invoked flow skills.
///
/// Extensions (e.g. `skills.flow__run`) validate the request against their
/// own catalog and forward execution to the host through this trait; the
/// model tool itself never holds session-spawn capabilities (M2.3 decision 5).
/// The host implementation reuses the same runtime, checkpointing, and
/// run-before guardian approval as the slash-command path.
pub trait FlowRunner: Send + Sync {
    fn run_flow<'a>(
        &'a self,
        turn_id: &'a str,
        name: &'a str,
        args: serde_json::Map<String, Value>,
    ) -> FlowRunFuture<'a>;
}

/// Structured result of one flow run, returned to the model as tool output.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct FlowRunOutput {
    pub outputs: serde_json::Map<String, Value>,
}

/// Failure modes of one flow run; tool callers map these to model-facing
/// tool errors.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FlowRunError {
    /// No flow skill with this name is available to the model in this turn.
    NotFound {
        name: String,
    },
    /// The turn that issued the call is no longer active.
    InactiveTurn {
        turn_id: String,
    },
    /// The host session is no longer available.
    SessionUnavailable,
    /// Guardian denied the run, or plan validation / execution failed. The
    /// reason is safe to surface to the model.
    Failed {
        reason: String,
    },
}
