//! odyBox assistant memory: an independent module layered on top of the Ody
//! memory pipeline, without changing it.
//!
//! ## Isolation contract
//!
//! Three guarantees keep this module from affecting the existing Ody memory
//! system:
//!
//! 1. **Product gate.** The contributors only activate when the session source
//!    maps to [`ody_protocol::protocol::Product::OdyBox`]. Plain `ody` CLI,
//!    `vscode`, `exec`, `mcp`, `atlas` and internal/sub-agent threads are left
//!    completely untouched.
//! 2. **No shared thread state.** When the gate is closed, nothing is inserted
//!    into the thread store, so no tool or prompt contributor can observe
//!    assistant-memory state on those threads.
//! 3. **Separate storage root.** Assistant memory lives under
//!    `$ODY_HOME/odybox_memories`, never under the Ody memory workspace
//!    `$ODY_HOME/memories` owned by `ody-memories-write`.
//!
//! ## Why this is not gated by a Cargo feature
//!
//! Product separation in this workspace is a *runtime* decision expressed
//! through `Product` / `--session-source` — the same mechanism `policy.products`
//! already uses to isolate skills and plugins. Cargo features here cut
//! *capabilities* (`v8`, `windows-sandbox`, `bundled-builtin-mcp`). Which
//! product gets assistant memory is a product question, not a build question,
//! so it stays runtime-side. See `README.md` in this crate for the rationale.
//!
//! ## Status
//!
//! Read path: `read` / `search` / `add_note` tools, mounted on odyBox threads
//! only. Write path: a filesystem-only extraction pipeline that lists rollout
//! files, extracts unprocessed odyBox sessions, and records them in the
//! assistant memory root. Cross-session consolidation is not implemented yet.

mod backend;
mod extension;
mod extract;
mod extractor_model;
mod gate;
mod ledger;
mod pipeline;
mod prompts;
mod scan;
mod store;
mod tools;

pub use extension::AssistantMemoryConfig;
pub use extension::AssistantMemoryExtension;
pub use extension::install;
pub use extract::ExtractError;
pub use extract::ExtractFuture;
pub use extract::ExtractedMemory;
pub use extract::MemoryExtractor;
pub use extract::NoopExtractor;
pub use extract::parse_extracted_memory;
pub use extractor_model::ModelMemoryExtractor;
pub use gate::assistant_memory_enabled;
pub use gate::assistant_memory_enabled_for_source;
pub use gate::product_for_source;
pub use ledger::EXTRACTED_SUBDIR;
pub use ledger::ExtractionLedger;
pub use pipeline::DEFAULT_SESSION_BUDGET;
pub use pipeline::PipelineReport;
pub use pipeline::SessionOutcome;
pub use pipeline::process_transcript;
pub use pipeline::run_once;
pub use scan::odybox_session_source;
pub use scan::odybox_sessions;
pub use store::ASSISTANT_MEMORY_DIR;
pub use store::ODY_MEMORY_DIR;
pub use store::assistant_memory_root;
pub use store::ody_memory_root;
pub use tools::ADD_NOTE_TOOL_NAME;
pub use tools::READ_TOOL_NAME;
pub use tools::SEARCH_TOOL_NAME;
pub use tools::TOOLS_NAMESPACE;
pub use tools::assistant_memory_tools;

#[cfg(test)]
#[path = "contract_tests.rs"]
mod contract_tests;
