//! Provider adapters implementing the `ChatProvider` trait.
//!
//! Each subdirectory exposes a concrete adapter for a specific wire API.

pub mod chat;
pub(crate) mod common;
pub mod core;
pub mod responses;
