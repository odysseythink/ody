//! Cloud-hosted configuration data for Ody.
//!
//! The OpenAI/Codex backend-delivered enterprise config bundle was removed in
//! M1.2. This crate now only exposes a no-op bundle loader stub so upstream
//! callers (TUI, exec, app-server) keep compiling without changes.

mod bundle_loader;

pub use bundle_loader::cloud_config_bundle_loader;
pub use bundle_loader::cloud_config_bundle_loader_for_storage;
