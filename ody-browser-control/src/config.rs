use std::path::PathBuf;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;

/// Browser control mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserControlMode {
    /// Launch a local Chrome process per thread with a temporary profile.
    #[default]
    Local,
    /// Connect to an externally managed Chrome debug endpoint.
    External,
}

/// Configuration for the browser control layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserControlConfig {
    /// Local launch or external debug connection.
    #[serde(default)]
    pub mode: BrowserControlMode,

    /// Explicit path to the Chrome/Chromium executable.
    /// When omitted, the process-lifecycle layer discovers it.
    pub chrome_path: Option<PathBuf>,

    /// Arguments passed to Chrome on launch.
    /// Dangerous arguments are filtered by [`BrowserControlConfig::sanitize_args`].
    #[serde(default)]
    pub launch_args: Vec<String>,

    /// Maximum number of concurrent Chrome processes across the whole process.
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_browsers: usize,

    /// Default timeout for CDP commands, in milliseconds.
    #[serde(default = "default_command_timeout_ms")]
    pub command_timeout_ms: u64,

    /// Navigation timeout, in milliseconds.
    #[serde(default = "default_navigation_timeout_ms")]
    pub navigation_timeout_ms: u64,

    /// WebSocket connection timeout, in milliseconds.
    #[serde(default = "default_connect_timeout_ms")]
    pub connect_timeout_ms: u64,

    /// External browser debug WebSocket URL (used when `mode == External`).
    pub connect_url: Option<String>,

    /// Whether to allow navigation to local/private network targets.
    #[serde(default)]
    pub allow_local_network: bool,

    /// In external mode, whether to allow sensitive operations (click, type, evaluate).
    #[serde(default)]
    pub external_browser_allow_sensitive: bool,

    /// Maximum size of a single console message stored in the event buffer.
    #[serde(default = "default_max_console_message_bytes")]
    pub max_console_message_bytes: usize,

    /// Maximum number of entries retained in the event buffer.
    #[serde(default = "default_max_event_entries")]
    pub max_event_entries: usize,

    /// Maximum total bytes retained in the event buffer.
    #[serde(default = "default_max_event_buffer_bytes")]
    pub max_event_buffer_bytes: usize,
}

impl Default for BrowserControlConfig {
    fn default() -> Self {
        Self {
            mode: BrowserControlMode::default(),
            chrome_path: None,
            launch_args: Vec::new(),
            max_concurrent_browsers: default_max_concurrent(),
            command_timeout_ms: default_command_timeout_ms(),
            navigation_timeout_ms: default_navigation_timeout_ms(),
            connect_timeout_ms: default_connect_timeout_ms(),
            connect_url: None,
            allow_local_network: false,
            external_browser_allow_sensitive: false,
            max_console_message_bytes: default_max_console_message_bytes(),
            max_event_entries: default_max_event_entries(),
            max_event_buffer_bytes: default_max_event_buffer_bytes(),
        }
    }
}

impl BrowserControlConfig {
    /// Return a sanitized copy of `launch_args` with dangerous arguments removed.
    pub fn sanitize_args(&self) -> Vec<String> {
        let denylist: &[&str] = &[
            "--user-data-dir",
            "--remote-debugging-port",
            "--proxy-server",
            "--no-sandbox",
            "--disable-web-security",
            "--disable-features",
        ];
        self.launch_args
            .iter()
            .filter(|arg| !denylist.iter().any(|denied| arg.starts_with(denied)))
            .cloned()
            .collect()
    }

    /// Return the configured Chrome path, or a placeholder to be resolved later.
    pub fn resolved_chrome_path(&self) -> Option<PathBuf> {
        self.chrome_path.clone()
    }

    /// Build a minimal set of launch arguments that include the user-data-dir.
    /// The process-lifecycle layer is responsible for adding `--remote-debugging-port`.
    pub fn build_launch_args(&self, user_data_dir: &std::path::Path) -> Vec<String> {
        let mut args = vec![
            format!("--user-data-dir={}", user_data_dir.display()),
            "--no-first-run".to_string(),
            "--no-default-browser-check".to_string(),
            "--disable-background-networking".to_string(),
            "--disable-background-timer-throttling".to_string(),
            "--disable-renderer-backgrounding".to_string(),
            "--disable-features=Translate".to_string(),
        ];
        args.extend(self.sanitize_args());
        args
    }
}

const fn default_max_concurrent() -> usize {
    4
}

const fn default_command_timeout_ms() -> u64 {
    30_000
}

const fn default_navigation_timeout_ms() -> u64 {
    60_000
}

const fn default_connect_timeout_ms() -> u64 {
    10_000
}

const fn default_max_console_message_bytes() -> usize {
    4 * 1024
}

const fn default_max_event_entries() -> usize {
    1000
}

const fn default_max_event_buffer_bytes() -> usize {
    1024 * 1024
}

static CONCURRENT_BROWSER_SEMAPHORE: OnceLock<Semaphore> = OnceLock::new();

/// Acquire a global permit to start a new Chrome process.
///
/// The quota is initialized from `config.max_concurrent_browsers` on the first
/// call and remains fixed for the process lifetime. Subsequent calls ignore the
/// configuration value and reuse the existing semaphore.
pub async fn acquire_browser_permit(
    config: &BrowserControlConfig,
) -> Result<tokio::sync::SemaphorePermit<'static>, tokio::sync::AcquireError> {
    let semaphore: &'static tokio::sync::Semaphore = CONCURRENT_BROWSER_SEMAPHORE
        .get_or_init(|| tokio::sync::Semaphore::new(config.max_concurrent_browsers.max(1)));
    semaphore.acquire().await
}

/// Return the current number of available browser permits, if initialized.
pub fn available_browser_permits() -> Option<usize> {
    CONCURRENT_BROWSER_SEMAPHORE
        .get()
        .map(|s| s.available_permits())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_reasonable() {
        let cfg = BrowserControlConfig::default();
        assert_eq!(cfg.mode, BrowserControlMode::Local);
        assert_eq!(cfg.max_concurrent_browsers, 4);
        assert_eq!(cfg.command_timeout_ms, 30_000);
        assert_eq!(cfg.navigation_timeout_ms, 60_000);
        assert_eq!(cfg.max_event_entries, 1000);
        assert_eq!(cfg.max_event_buffer_bytes, 1024 * 1024);
        assert_eq!(cfg.max_console_message_bytes, 4096);
    }

    #[test]
    fn sanitize_args_filters_dangerous_flags() {
        let cfg = BrowserControlConfig {
            launch_args: vec![
                "--window-size=1280,720".to_string(),
                "--user-data-dir=/evil".to_string(),
                "--remote-debugging-port=9222".to_string(),
                "--proxy-server=http://evil".to_string(),
            ],
            ..Default::default()
        };
        let sanitized = cfg.sanitize_args();
        assert!(sanitized.contains(&"--window-size=1280,720".to_string()));
        assert!(!sanitized.iter().any(|a| a.starts_with("--user-data-dir")));
        assert!(!sanitized.iter().any(|a| a.starts_with("--remote-debugging-port")));
        assert!(!sanitized.iter().any(|a| a.starts_with("--proxy-server")));
    }

    #[test]
    fn build_launch_args_includes_user_data_dir() {
        let cfg = BrowserControlConfig::default();
        let dir = std::env::temp_dir();
        let args = cfg.build_launch_args(&dir);
        assert!(args
            .iter()
            .any(|a| a.starts_with("--user-data-dir=")));
    }

    #[tokio::test]
    async fn semaphore_limits_concurrent_launches() {
        let cfg = BrowserControlConfig {
            max_concurrent_browsers: 2,
            ..Default::default()
        };
        let p1 = acquire_browser_permit(&cfg).await.unwrap();
        let p2 = acquire_browser_permit(&cfg).await.unwrap();
        // Third permit should not be available immediately.
        assert!(available_browser_permits().unwrap() == 0);
        drop(p1);
        drop(p2);
    }
}
