use std::path::PathBuf;

use thiserror::Error;

/// Unified error type for the browser control layer.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum BrowserControlError {
    /// The Chrome/Chromium executable could not be found.
    #[error("chrome not found; searched paths: {searched_paths:?}")]
    ChromeNotFound { searched_paths: Vec<PathBuf> },

    /// Failed to launch a new Chrome process.
    #[error("failed to launch chrome: {source}")]
    LaunchFailed { source: chromiumoxide::error::CdpError },

    /// Failed to connect to an external Chrome debug endpoint.
    #[error("failed to connect to chrome: {source}")]
    ConnectFailed { source: chromiumoxide::error::CdpError },

    /// A CDP command failed after a connection was established.
    #[error("command {command} failed: {source}")]
    CommandFailed {
        command: String,
        source: chromiumoxide::error::CdpError,
    },

    /// A CDP command exceeded its configured timeout.
    #[error("command {command} timed out after {elapsed_ms}ms")]
    Timeout { command: String, elapsed_ms: u64 },

    /// The target page crashed.
    #[error("page crashed")]
    PageCrashed,

    /// The requested operation is not allowed in the current configuration.
    #[error("not allowed: {reason}")]
    NotAllowed { reason: String },

    /// The session has already been closed.
    #[error("session closed")]
    SessionClosed,

    /// The requested operation requires an approval ticket that was rejected or not provided.
    #[error("approval required: {reason}")]
    ApprovalRequired { reason: String },
}

impl BrowserControlError {
    /// Wrap a [`chromiumoxide::error::CdpError`] as a command failure.
    pub fn from_command_error(command: impl Into<String>, source: chromiumoxide::error::CdpError) -> Self {
        Self::CommandFailed {
            command: command.into(),
            source,
        }
    }

    /// Classify the error as retryable or fatal.
    ///
    /// Retryable errors are transient (network, process, timeout) and should be
    /// retried by the caller with a bounded strategy. Fatal errors should not be
    /// retried.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::PageCrashed
                | Self::Timeout { .. }
                | Self::ConnectFailed { .. }
                | Self::LaunchFailed { .. }
        )
    }

    /// Return true if the error is caused by a JS evaluation exception.
    pub fn is_javascript_exception(&self) -> bool {
        matches!(
            self,
            Self::CommandFailed {
                source: chromiumoxide::error::CdpError::JavascriptException(_),
                ..
            }
        )
    }
}

impl From<chromiumoxide::error::CdpError> for BrowserControlError {
    fn from(source: chromiumoxide::error::CdpError) -> Self {
        Self::CommandFailed {
            command: String::new(),
            source,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryable_classification() {
        assert!(BrowserControlError::PageCrashed.is_retryable());
        assert!(BrowserControlError::Timeout {
            command: "navigate".to_string(),
            elapsed_ms: 100,
        }
        .is_retryable());
        assert!(BrowserControlError::ConnectFailed {
            source: chromiumoxide::error::CdpError::msg("ws refused"),
        }
        .is_retryable());
        assert!(BrowserControlError::LaunchFailed {
            source: chromiumoxide::error::CdpError::msg("spawn failed"),
        }
        .is_retryable());

        assert!(!BrowserControlError::ChromeNotFound {
            searched_paths: vec![PathBuf::from("/usr/bin/chrome")],
        }
        .is_retryable());
        assert!(!BrowserControlError::NotAllowed {
            reason: "external browser".to_string(),
        }
        .is_retryable());
        assert!(!BrowserControlError::CommandFailed {
            command: "evaluate".to_string(),
            source: chromiumoxide::error::CdpError::msg("syntax error"),
        }
        .is_retryable());
    }

    #[test]
    fn from_command_error_sets_command() {
        let err = BrowserControlError::from_command_error(
            "navigate",
            chromiumoxide::error::CdpError::msg("boom"),
        );
        let s = err.to_string();
        assert!(s.contains("navigate"));
        assert!(s.contains("boom"));
    }
}
