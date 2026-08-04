//! Browser process lifecycle layer.
//!
//! The original roadmap sketched a [`BrowserProcess`] trait covering Chrome
//! discovery, launch with a temporary profile, attach to an existing debug
//! endpoint, and process termination. The implementation uses `chromiumoxide`
//! for launching and connecting, and `crate::config` for executable discovery
//! and launch-argument construction.
//!
//! This module is kept as the architectural placeholder required by the roadmap.
//! The process responsibilities listed in the roadmap are fulfilled by:
//!
//! * `crate::config::discover_chrome` — executable discovery with platform
//!   fallback paths and the `ODY_CHROME_EXECUTABLE` / `chrome_executable` config
//!   override.
//! * `crate::config::acquire_browser_permit` and
//!   `crate::config::available_browser_permits` — cross-process concurrency
//!   quota for Chrome launches.
//! * `crate::session::BrowserSession::launch_local` — start a headless Chrome
//!   process with a temporary `--user-data-dir`.
//! * `crate::session::BrowserSession::connect_external` — attach to a user
//!   provided `debugger_url`.
//! * `crate::session::BrowserSession::close` / `Drop` — kill the local process
//!   and clean up the temporary profile directory.
//!
//! Future work could re-introduce a thin `BrowserProcess` trait wrapper if the
//! project ever needs to abstract over multiple CDP process backends.

pub use crate::config::{acquire_browser_permit, available_browser_permits, discover_chrome};
pub use crate::session::BrowserSession;
