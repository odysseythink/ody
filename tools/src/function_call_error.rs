use thiserror::Error;

/// Error returned while executing a model-visible tool invocation.
#[derive(Debug, Error, PartialEq)]
pub enum FunctionCallError {
    #[error("{0}")]
    RespondToModel(String),
    #[error("Fatal error: {0}")]
    Fatal(String),
    #[error("Retryable error: {0}")]
    Retryable(String),
    /// The tool requires guardian approval before it may execute. The `ticket`
    /// carries a JSON serializable payload describing the action to be reviewed.
    #[error("Approval required: {ticket}")]
    NeedsApproval { ticket: serde_json::Value },
}
