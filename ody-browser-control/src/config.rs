use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;

use crate::error::BrowserControlError;

/// Browser control mode.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema, ts_rs::TS,
)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum BrowserControlMode {
    /// Launch a local Chrome process per thread with a temporary profile.
    #[default]
    Local,
    /// Connect to an externally managed Chrome debug endpoint.
    External,
}

/// Viewport configuration passed to Chrome on launch.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, schemars::JsonSchema, ts_rs::TS)]
#[ts(export)]
pub struct ViewportConfig {
    /// Viewport width in CSS pixels.
    #[serde(default = "default_viewport_width")]
    pub width: u32,
    /// Viewport height in CSS pixels.
    #[serde(default = "default_viewport_height")]
    pub height: u32,
    /// Optional device scale factor (e.g. 2.0 for retina-like emulation).
    #[serde(default)]
    pub device_scale_factor: Option<f64>,
}

impl Default for ViewportConfig {
    fn default() -> Self {
        Self {
            width: default_viewport_width(),
            height: default_viewport_height(),
            device_scale_factor: None,
        }
    }
}

impl From<ViewportConfig> for chromiumoxide::handler::viewport::Viewport {
    fn from(cfg: ViewportConfig) -> Self {
        Self {
            width: cfg.width,
            height: cfg.height,
            device_scale_factor: cfg.device_scale_factor,
            emulating_mobile: false,
            is_landscape: false,
            has_touch: false,
        }
    }
}

/// Configuration for the browser control layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, ts_rs::TS)]
#[ts(export)]
pub struct BrowserControlConfig {
    /// Local launch or external debug connection.
    #[serde(default)]
    pub mode: BrowserControlMode,

    /// Explicit path to the Chrome/Chromium executable.
    /// When omitted, the process-lifecycle layer discovers it.
    #[serde(default)]
    pub chrome_executable: Option<PathBuf>,

    /// Run Chrome in headless mode.
    #[serde(default = "default_headless")]
    pub headless: bool,

    /// Viewport used when launching the browser.
    #[serde(default)]
    pub viewport: ViewportConfig,

    /// Run Chrome with the OS sandbox enabled.
    #[serde(default = "default_sandbox")]
    pub sandbox: bool,

    /// Disable browser extensions and prevent loading them.
    #[serde(default = "default_disable_extensions")]
    pub disable_extensions: bool,

    /// Additional arguments passed to Chrome on launch.
    /// Dangerous arguments are filtered by [`BrowserControlConfig::sanitize_args`].
    #[serde(default)]
    pub extra_args: Vec<String>,

    /// Maximum number of concurrent Chrome processes across the whole process.
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_browsers: usize,

    /// Maximum time to wait for a browser concurrency permit before failing.
    #[serde(default = "default_browser_permit_acquire_timeout_ms")]
    pub browser_permit_acquire_timeout_ms: u64,

    /// Default timeout for CDP commands, in milliseconds.
    #[serde(default = "default_command_timeout_ms")]
    pub command_timeout_ms: u64,

    /// Navigation timeout, in milliseconds.
    #[serde(default = "default_navigation_timeout_ms")]
    pub navigation_timeout_ms: u64,

    /// Timeout for launching a local Chrome process, in milliseconds.
    #[serde(default = "default_launch_timeout_ms")]
    pub launch_timeout_ms: u64,

    /// WebSocket connection timeout for external browsers, in milliseconds.
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
            chrome_executable: None,
            headless: default_headless(),
            viewport: ViewportConfig::default(),
            sandbox: default_sandbox(),
            disable_extensions: default_disable_extensions(),
            extra_args: Vec::new(),
            max_concurrent_browsers: default_max_concurrent(),
            browser_permit_acquire_timeout_ms: default_browser_permit_acquire_timeout_ms(),
            command_timeout_ms: default_command_timeout_ms(),
            navigation_timeout_ms: default_navigation_timeout_ms(),
            launch_timeout_ms: default_launch_timeout_ms(),
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
    /// Return a sanitized copy of `extra_args` with dangerous arguments removed.
    pub fn sanitize_args(&self) -> Vec<String> {
        let denylist: &[&str] = &[
            "user-data-dir",
            "remote-debugging-port",
            "proxy-server",
            "no-sandbox",
            "disable-setuid-sandbox",
            "disable-web-security",
            "disable-features",
            "load-extension",
            "extensions-on-chrome-urls",
        ];
        self.extra_args
            .iter()
            .filter(|arg| {
                let key = arg.strip_prefix("--").unwrap_or(arg);
                let key = key.split_once('=').map(|(k, _)| k).unwrap_or(key);
                !denylist.iter().any(|denied| key.eq_ignore_ascii_case(denied))
            })
            .cloned()
            .collect()
    }

    /// Build the default set of launch arguments that the process layer will
    /// combine with sanitized user-supplied `extra_args`.
    ///
    /// The returned strings include leading `--` dashes. Callers that pass them
    /// to `chromiumoxide::BrowserConfig` should strip the dashes because the
    /// crate internally re-adds them.
    pub fn build_launch_args(&self) -> Vec<String> {
        let mut args = vec![
            "--no-first-run".to_string(),
            "--no-default-browser-check".to_string(),
            "--disable-background-networking".to_string(),
            "--disable-background-timer-throttling".to_string(),
            "--disable-renderer-backgrounding".to_string(),
        ];
        if self.disable_extensions {
            args.push("--disable-extensions".to_string());
        }
        args.push("--disable-features=Translate".to_string());
        args.extend(self.sanitize_args());
        args
    }

    /// Return true if changing from `self` to `other` requires a new Chrome
    /// process or debug connection.
    ///
    /// Timeouts, event-buffer limits, and policy toggles that only affect how
    /// the already-running session is used do not force a restart.
    pub fn requires_restart(&self, other: &Self) -> bool {
        self.mode != other.mode
            || self.chrome_executable != other.chrome_executable
            || self.headless != other.headless
            || self.viewport != other.viewport
            || self.sandbox != other.sandbox
            || self.disable_extensions != other.disable_extensions
            || self.extra_args != other.extra_args
            || self.connect_url != other.connect_url
    }
}

/// Discover a Chrome/Chromium executable using the following sources in order:
///
/// 1. The `CHROME` environment variable.
/// 2. `config.chrome_executable` if set.
/// 3. `chromiumoxide::detection::default_executable` (PATH, registry, platform defaults).
///
/// If all sources fail, returns [`BrowserControlError::ChromeNotFound`] with the
/// paths that were inspected.
pub fn discover_chrome(config: &BrowserControlConfig) -> Result<PathBuf, BrowserControlError> {
    let mut searched_paths: Vec<PathBuf> = Vec::new();

    if let Ok(chrome_env) = std::env::var("CHROME") {
        let path = PathBuf::from(chrome_env);
        if path.exists() {
            return Ok(path);
        }
        searched_paths.push(path);
    }

    if let Some(path) = config.chrome_executable.as_ref() {
        if path.exists() {
            return Ok(path.clone());
        }
        searched_paths.push(path.clone());
    }

    match chromiumoxide::detection::default_executable(
        chromiumoxide::detection::DetectionOptions::default(),
    ) {
        Ok(path) => Ok(path),
        Err(_) => Err(BrowserControlError::ChromeNotFound { searched_paths }),
    }
}

const fn default_headless() -> bool {
    true
}

const fn default_sandbox() -> bool {
    true
}

const fn default_disable_extensions() -> bool {
    true
}

const fn default_viewport_width() -> u32 {
    1280
}

const fn default_viewport_height() -> u32 {
    720
}

const fn default_max_concurrent() -> usize {
    4
}

const fn default_browser_permit_acquire_timeout_ms() -> u64 {
    30_000
}

const fn default_command_timeout_ms() -> u64 {
    30_000
}

const fn default_navigation_timeout_ms() -> u64 {
    60_000
}

const fn default_launch_timeout_ms() -> u64 {
    20_000
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
/// configuration value and reuse the existing semaphore. If a permit is not
/// available within `config.browser_permit_acquire_timeout_ms`, a
/// [`BrowserControlError::QuotaExceeded`] is returned.
pub async fn acquire_browser_permit(
    config: &BrowserControlConfig,
) -> Result<tokio::sync::SemaphorePermit<'static>, BrowserControlError> {
    let semaphore: &'static tokio::sync::Semaphore = CONCURRENT_BROWSER_SEMAPHORE
        .get_or_init(|| tokio::sync::Semaphore::new(config.max_concurrent_browsers.max(1)));
    let timeout = Duration::from_millis(config.browser_permit_acquire_timeout_ms);
    match tokio::time::timeout(timeout, semaphore.acquire()).await {
        Ok(Ok(permit)) => Ok(permit),
        Ok(Err(_)) => Err(BrowserControlError::QuotaExceeded {
            reason: "browser concurrency semaphore closed".to_string(),
        }),
        Err(_) => Err(BrowserControlError::QuotaExceeded {
            reason: format!(
                "max concurrent browser quota ({}) reached",
                config.max_concurrent_browsers
            ),
        }),
    }
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
        assert!(cfg.headless);
        assert!(cfg.sandbox);
        assert!(cfg.disable_extensions);
        assert_eq!(cfg.viewport.width, 1280);
        assert_eq!(cfg.viewport.height, 720);
        assert_eq!(cfg.max_concurrent_browsers, 4);
        assert_eq!(cfg.browser_permit_acquire_timeout_ms, 30_000);
        assert_eq!(cfg.command_timeout_ms, 30_000);
        assert_eq!(cfg.navigation_timeout_ms, 60_000);
        assert_eq!(cfg.launch_timeout_ms, 20_000);
        assert_eq!(cfg.connect_timeout_ms, 10_000);
        assert_eq!(cfg.max_event_entries, 1000);
        assert_eq!(cfg.max_event_buffer_bytes, 1024 * 1024);
        assert_eq!(cfg.max_console_message_bytes, 4096);
    }

    #[test]
    fn sanitize_args_filters_dangerous_flags() {
        let cfg = BrowserControlConfig {
            extra_args: vec![
                "--window-size=1280,720".to_string(),
                "--user-data-dir=/evil".to_string(),
                "--remote-debugging-port=9222".to_string(),
                "--proxy-server=http://evil".to_string(),
                "--disable-extensions".to_string(),
                "--load-extension=/evil".to_string(),
            ],
            ..Default::default()
        };
        let sanitized = cfg.sanitize_args();
        assert!(sanitized.contains(&"--window-size=1280,720".to_string()));
        assert!(!sanitized.iter().any(|a| a.starts_with("--user-data-dir")));
        assert!(!sanitized.iter().any(|a| a.starts_with("--remote-debugging-port")));
        assert!(!sanitized.iter().any(|a| a.starts_with("--proxy-server")));
        assert!(!sanitized.iter().any(|a| a.starts_with("--load-extension")));
    }

    #[test]
    fn build_launch_args_includes_defaults_and_extensions_flag() {
        let cfg = BrowserControlConfig::default();
        let args = cfg.build_launch_args();
        assert!(args.iter().any(|a| a == "--no-first-run"));
        assert!(args.iter().any(|a| a == "--disable-extensions"));
        assert!(args.iter().any(|a| a == "--disable-features=Translate"));
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

    #[test]
    fn requires_restart_detects_launch_affecting_changes() {
        let base = BrowserControlConfig::default();
        let mode_external = BrowserControlConfig {
            mode: BrowserControlMode::External,
            ..Default::default()
        };
        assert!(base.requires_restart(&mode_external));

        let head_changed = BrowserControlConfig {
            headless: false,
            ..Default::default()
        };
        assert!(base.requires_restart(&head_changed));

        let viewport_changed = BrowserControlConfig {
            viewport: ViewportConfig {
                width: 1920,
                height: 1080,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(base.requires_restart(&viewport_changed));

        let extra_args_changed = BrowserControlConfig {
            extra_args: vec!["--window-size=1920,1080".to_string()],
            ..Default::default()
        };
        assert!(base.requires_restart(&extra_args_changed));

        let connect_url_changed = BrowserControlConfig {
            connect_url: Some("ws://localhost:9222/devtools/browser/xxx".to_string()),
            ..Default::default()
        };
        assert!(base.requires_restart(&connect_url_changed));
    }

    #[test]
    fn requires_restart_ignores_runtime_only_changes() {
        let base = BrowserControlConfig::default();
        let timeouts_changed = BrowserControlConfig {
            command_timeout_ms: 5_000,
            navigation_timeout_ms: 10_000,
            launch_timeout_ms: 5_000,
            connect_timeout_ms: 5_000,
            ..Default::default()
        };
        assert!(!base.requires_restart(&timeouts_changed));

        let buffers_changed = BrowserControlConfig {
            max_event_entries: 100,
            max_event_buffer_bytes: 1024,
            max_console_message_bytes: 1024,
            ..Default::default()
        };
        assert!(!base.requires_restart(&buffers_changed));

        let policy_changed = BrowserControlConfig {
            allow_local_network: true,
            external_browser_allow_sensitive: true,
            ..Default::default()
        };
        assert!(!base.requires_restart(&policy_changed));
    }
}
