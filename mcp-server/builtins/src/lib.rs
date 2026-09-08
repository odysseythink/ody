//! Rust implementations of the odyBox builtin MCP servers.
//!
//! Each module exposes one MCP server (stdio transport) that mirrors the
//! tool surface of the corresponding chatbox builtin server:
//!
//! - [`fetch`]: web page fetching with HTML -> markdown conversion
//! - [`sequentialthinking`]: structured, reflective problem solving
//! - [`arxiv`]: arXiv paper search / download / list / read
//! - [`context7`]: up-to-date library documentation lookup
//!
//! All servers are served by the `ody-builtin-mcp` binary, one subcommand
//! per server, so ody can spawn them as stdio child processes.

pub mod arxiv;
pub mod context7;
pub mod fetch;
pub mod sequentialthinking;
