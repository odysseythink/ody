//! Status output formatting and display adapters for the TUI.
//!
//! This module turns protocol-level snapshots into stable display structures used by `/status`
//! output and footer/status-line helpers, while keeping rendering concerns out of transport-facing
//! code.
//!
//! `rate_limits` is the main integration point for status-line usage-limit items: it converts raw
//! window snapshots into local-time labels and classifies data as available, stale, or missing.
mod auth;
mod card;
mod format;
mod helpers;
pub(crate) mod remote_connection;

pub(crate) use auth::StatusAuthDisplay;
pub(crate) use card::StatusHistoryHandle;
#[cfg(test)]
pub(crate) use card::new_status_output;
pub(crate) use card::new_status_output_with_handle;
pub(crate) use helpers::compose_agents_summary;
pub(crate) use helpers::format_directory_display;
pub(crate) use helpers::format_tokens_compact;

#[cfg(test)]
mod tests;
