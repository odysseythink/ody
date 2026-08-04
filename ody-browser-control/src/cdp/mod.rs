//! CDP transport layer.
//!
//! The original roadmap envisioned a custom `tokio-tungstenite` + `serde_json`
//! CDP client. The current implementation delegates WebSocket transport,
//! JSON-RPC framing, request/response multiplexing, and event subscriptions to
//! `chromiumoxide`. The submodules below are architectural placeholders that
//! document where each responsibility lives.

pub mod json_rpc;
pub mod transport;

pub use chromiumoxide::error::CdpError;
