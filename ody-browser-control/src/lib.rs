//! Ody browser control crate: CDP transport layer over chromiumoxide.

pub mod config;
pub mod error;
pub mod event_buffer;
pub mod page_state;
pub mod session;
pub mod thread_state;
pub mod tools;
pub mod approval_exemption;
pub mod network_redaction;
pub mod raw_cdp_blocklist;
pub mod types;
pub mod url_block;

pub use approval_exemption::is_approval_exempt;
pub use config::{
    acquire_browser_permit, available_browser_permits, discover_chrome, BrowserControlConfig,
    BrowserControlMode, ViewportConfig,
};
pub use error::BrowserControlError;
pub use event_buffer::{ConsoleEntry, EventBuffer, LogsSnapshot, NetworkEntry};
pub use page_state::PageState;
pub use session::BrowserSession;
pub use thread_state::BrowserThreadState;
pub use tools::all_tools;
pub use tools::BrowserTool;
pub use types::*;
pub use url_block::{check_js_allowed, check_url_is_allowed};

/// Approval ticket emitted by the tool layer for guardian review.
///
/// The exact mapping to `GuardianApprovalRequest::BrowserAction` is handled by
/// the `app-server` extension layer; this crate only carries an opaque action
/// name and JSON details.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BrowserControlApprovalTicket {
    pub action: String,
    pub details: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_ticket_round_trips() {
        let ticket = BrowserControlApprovalTicket {
            action: "navigate".to_string(),
            details: serde_json::json!({"url": "https://example.com"}),
        };
        let json = serde_json::to_string(&ticket).unwrap();
        let back: BrowserControlApprovalTicket = serde_json::from_str(&json).unwrap();
        assert_eq!(back.action, "navigate");
    }
}
