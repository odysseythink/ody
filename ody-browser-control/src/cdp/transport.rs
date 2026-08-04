//! CDP WebSocket transport layer.
//!
//! The original roadmap planned a custom `tokio-tungstenite` transport with
//! request/response multiplexing and event subscriptions. The current
//! implementation relies on `chromiumoxide` to manage the WebSocket connection
//! to Chrome and to expose CDP commands through `Browser`/`Page` handles.
//!
//! This module is an architectural placeholder documenting the boundary.
