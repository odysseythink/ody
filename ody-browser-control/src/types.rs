use serde::{Deserialize, Serialize};

/// Maximum size of a DOM snapshot returned to the model.
pub const DOM_MAX_BYTES: usize = 8 * 1024;

/// Maximum size of a logs snapshot returned to the model.
pub const LOGS_MAX_BYTES: usize = 1024 * 1024;

/// Maximum size of a base64-encoded screenshot returned to the model.
pub const SCREENSHOT_MAX_BYTES: usize = 2 * 1024 * 1024;

/// When a page navigation should be considered complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WaitCondition {
    /// Wait until the `load` event fires.
    Load,
    /// Wait until the `DOMContentLoaded` event fires.
    DomContentLoaded,
    /// Wait until the network is idle.
    NetworkIdle,
}

/// Result of a successful navigation.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NavigationResult {
    pub url: String,
    pub title: String,
    pub wait_until: Option<WaitCondition>,
}

/// A point in CSS pixels used for mouse interactions.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// Result of a screenshot capture.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ScreenshotResult {
    /// Base64-encoded PNG image data.
    pub data: String,
    /// MIME type of the image (`image/png`).
    pub mime_type: String,
    /// Whether the image was truncated to [`SCREENSHOT_MAX_BYTES`].
    pub truncated: bool,
}

/// Result of a JavaScript evaluation.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EvaluateResult {
    /// Serialized return value.
    pub value: serde_json::Value,
    /// Exception message if the evaluation threw.
    pub exception: Option<String>,
}

/// Which log entries to include when reading the page event buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LogKind {
    Console,
    Network,
    All,
}

/// Minimum severity to include when reading the page event buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    /// Least restrictive: include all log levels.
    #[default]
    Verbose,
    Info,
    Warning,
    Error,
}

impl LogLevel {
    /// Return whether `entry_level` is at least as severe as `self`.
    ///
    /// Console entries from CDP use title-case strings such as `"Info"` or
    /// `"Error"`. Unknown levels are treated as `Info`.
    pub fn allows(self, entry_level: &str) -> bool {
        let entry = LogLevel::from_cdp(entry_level);
        self.severity() <= entry.severity()
    }

    fn severity(self) -> u8 {
        match self {
            LogLevel::Verbose => 0,
            LogLevel::Info => 1,
            LogLevel::Warning => 2,
            LogLevel::Error => 3,
        }
    }

    fn from_cdp(level: &str) -> Self {
        match level.to_lowercase().as_str() {
            "verbose" => LogLevel::Verbose,
            "info" => LogLevel::Info,
            "warning" => LogLevel::Warning,
            "error" => LogLevel::Error,
            _ => LogLevel::Info,
        }
    }
}

impl LogKind {
    pub fn includes_console(self) -> bool {
        matches!(self, LogKind::Console | LogKind::All)
    }

    pub fn includes_network(self) -> bool {
        matches!(self, LogKind::Network | LogKind::All)
    }
}

impl Default for LogKind {
    fn default() -> Self {
        LogKind::All
    }
}

/// Truncate `text` to at most `max_bytes` UTF-8 bytes at a character boundary.
///
/// If truncated, a notice is appended that includes the original byte count.
pub fn truncate_string_bytes(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut cut = max_bytes;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut result = text[..cut].to_string();
    result.push_str(&format!(
        "\n... [truncated from {} bytes to {} bytes]",
        text.len(),
        cut
    ));
    result
}

/// Truncate a base64-encoded payload to at most `max_bytes` ASCII characters.
///
/// The notice is appended to the truncated string without breaking the base64
/// padding.
pub fn truncate_base64_bytes(data: &str, max_bytes: usize) -> String {
    if data.len() <= max_bytes {
        return data.to_string();
    }
    let mut cut = max_bytes;
    // base64 characters are single-byte ASCII; walk back to a multiple of 4 so
    // the truncated string remains valid base64.
    while cut > 0 && cut % 4 != 0 {
        cut -= 1;
    }
    let mut result = data[..cut].to_string();
    result.push_str(&format!(
        "\n... [truncated from {} bytes to {} bytes]",
        data.len(),
        cut
    ));
    result
}

/// Return a short, truncated preview of a JavaScript expression for logging and
/// approval tickets without leaking the full script body.
///
/// The limit is intentionally small (200 bytes) so that tracing fields and log
/// lines stay compact even when the model passes a long snippet.
pub fn expression_preview(js: &str) -> String {
    truncate_string_bytes(js, 200)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_string_respects_char_boundary() {
        let text = "aéb"; // 4 bytes: a (1), é (2), b (1)
        assert_eq!(truncate_string_bytes(text, 2), "a\n... [truncated from 4 bytes to 1 bytes]");
    }

    #[test]
    fn truncate_string_noop_when_under_limit() {
        assert_eq!(truncate_string_bytes("hello", 8), "hello");
    }

    #[test]
    fn truncate_base64_keeps_multiple_of_four() {
        let data = "ABCDEFGHIJKLMNOPQRSTUVWXYZ"; // 26 bytes
        let result = truncate_base64_bytes(data, 10);
        assert_eq!(result.len(), 8 + "\n... [truncated from 26 bytes to 8 bytes]".len());
        assert_eq!(&result[..8], "ABCDEFGH");
    }

    #[test]
    fn log_level_filters_correctly() {
        assert!(LogLevel::Info.allows("error"));
        assert!(LogLevel::Error.allows("Error"));
        assert!(!LogLevel::Error.allows("info"));
        assert!(LogLevel::Verbose.allows("warning"));
    }

    #[test]
    fn log_kind_includes_all() {
        assert!(LogKind::All.includes_console());
        assert!(LogKind::All.includes_network());
        assert!(!LogKind::Console.includes_network());
        assert!(!LogKind::Network.includes_console());
    }

    #[test]
    fn expression_preview_passes_short_expression() {
        assert_eq!(expression_preview("1 + 1"), "1 + 1");
    }

    #[test]
    fn expression_preview_truncates_long_expression() {
        let long = "a".repeat(500);
        let preview = expression_preview(&long);
        assert!(preview.len() < 400);
        assert!(preview.contains("truncated"));
    }
}
