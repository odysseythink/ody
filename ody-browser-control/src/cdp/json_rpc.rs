//! CDP JSON-RPC 1.0 framing.
//!
//! The original roadmap planned a manual JSON-RPC encoder/decoder with command
//! IDs and error responses. The current implementation relies on
//! `chromiumoxide` to serialize CDP commands and parse CDP responses/events.
//!
//! This module is an architectural placeholder documenting the boundary.
