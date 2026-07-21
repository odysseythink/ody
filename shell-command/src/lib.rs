//! Command parsing and safety utilities shared across Ody crates.

pub mod shell_detect;

pub mod bash;
pub use shell_detect::detect_windows_bash;
pub use shell_detect::FsChecker;
pub use shell_detect::RealFsChecker;
pub use shell_detect::WindowsBashDetection;
pub(crate) mod command_safety;
pub mod parse_command;
pub mod powershell;

pub use command_safety::is_dangerous_command;
pub use command_safety::is_safe_command;

#[cfg(test)]
mod shell_detect_windows_tests;
