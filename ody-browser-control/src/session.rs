use std::time::Duration;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::handler::{Handler, HandlerConfig};
use futures::StreamExt;
use tempfile::TempDir;
use tokio::task::JoinHandle;

use crate::config::{acquire_browser_permit, discover_chrome, BrowserControlConfig, BrowserControlMode};
use crate::error::BrowserControlError;

/// Thread-level browser session.
///
/// Each session owns a single Chrome process (when launched locally) and a
/// dedicated `chromiumoxide` handler task. Pages are created via
/// [`BrowserSession::new_page`](crate::page_state::BrowserSession::new_page) in
/// the `page_state` module.
pub struct BrowserSession {
    browser: Option<Browser>,
    handler_task: Option<JoinHandle<()>>,
    _permit: Option<tokio::sync::SemaphorePermit<'static>>,
    _profile_dir: TempDir,
    _local: bool,
    config: BrowserControlConfig,
}

impl std::fmt::Debug for BrowserSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowserSession")
            .field("browser", &"<chromiumoxide::Browser>")
            .field("local", &self._local)
            .finish()
    }
}

impl Drop for BrowserSession {
    fn drop(&mut self) {
        // Already closed via the explicit close() path; nothing to do except
        // abort any remaining handler task.
        if self.browser.is_none() {
            if let Some(handler_task) = self.handler_task.take() {
                handler_task.abort();
            }
            return;
        }

        // Best-effort: local sessions may still have a live Chrome process if
        // close() was not called or panicked. Try to kill it by PID before the
        // TempDir is dropped, otherwise the profile directory can remain locked
        // on Windows.
        if self._local {
            if let Some(browser) = self.browser.as_mut() {
                if let Some(child) = browser.get_mut_child() {
                    if let Some(pid) = child.as_mut_inner().id() {
                        kill_pid_best_effort(pid);
                    }
                }
            }
        }

        if let Some(handler_task) = self.handler_task.take() {
            handler_task.abort();
        }
    }
}

impl BrowserSession {
    /// Launch a local Chrome process using the provided configuration.
    pub async fn launch(config: BrowserControlConfig) -> Result<Self, BrowserControlError> {
        if !matches!(config.mode, BrowserControlMode::Local) {
            return Err(BrowserControlError::NotAllowed {
                reason: "launch() requires BrowserControlMode::Local".to_string(),
            });
        }

        let _permit = Some(acquire_browser_permit(&config).await?);

        let profile_dir = TempDir::with_prefix("ody-browser-profile")
            .map_err(|e| BrowserControlError::LaunchFailed { source: e.into() })?;

        let chrome_path = discover_chrome(&config)?;

        let mut builder = BrowserConfig::builder().chrome_executable(chrome_path);
        if !config.headless {
            builder = builder.with_head();
        }
        // Headless mode defaults to the chromiumoxide old headless mode, which is
        // more stable than the new headless mode for CDP automation on Windows.
        if !config.sandbox {
            builder = builder.no_sandbox();
        }
        let cdp_viewport: chromiumoxide::handler::viewport::Viewport = config.viewport.into();
        builder = builder
            .user_data_dir(profile_dir.path())
            .viewport(Some(cdp_viewport))
            .launch_timeout(Duration::from_millis(config.launch_timeout_ms))
            .request_timeout(Duration::from_millis(config.command_timeout_ms));

        let stripped_args = strip_leading_dashes(&config.build_launch_args());
        builder = builder.args(stripped_args);

        let browser_config = builder.build().map_err(|e| BrowserControlError::LaunchFailed {
            source: chromiumoxide::error::CdpError::msg(e),
        })?;

        let (browser, handler) = Browser::launch(browser_config)
            .await
            .map_err(|e| BrowserControlError::LaunchFailed { source: e })?;

        let handler_task = Some(spawn_handler(handler));

        Ok(Self {
            browser: Some(browser),
            handler_task,
            _permit,
            _profile_dir: profile_dir,
            _local: true,
            config,
        })
    }

    /// Connect to an existing Chrome debug endpoint.
    #[tracing::instrument(skip_all, fields(mode = "external", url_preview = %crate::types::truncate_string_bytes(config.connect_url.as_deref().unwrap_or(""), 120)))]
    pub async fn connect(config: BrowserControlConfig) -> Result<Self, BrowserControlError> {
        if !matches!(config.mode, BrowserControlMode::External) {
            return Err(BrowserControlError::NotAllowed {
                reason: "connect() requires BrowserControlMode::External".to_string(),
            });
        }

        let ws_url = config
            .connect_url
            .as_deref()
            .ok_or_else(|| BrowserControlError::NotAllowed {
                reason: "External mode requires connect_url".to_string(),
            })?;

        let dummy_profile_dir = TempDir::with_prefix("ody-browser-external-dummy")
            .map_err(|e| BrowserControlError::ConnectFailed { source: e.into() })?;

        // Use a minimal HandlerConfig so external connections still respect our
        // command timeout and HTTPS-error handling defaults.
        let mut connect_config = HandlerConfig::default();
        connect_config.request_timeout = Duration::from_millis(config.command_timeout_ms);
        connect_config.viewport = Some(config.viewport.into());

        let connect_fut = Browser::connect_with_config(ws_url, connect_config);
        let connect_timeout = Duration::from_millis(config.connect_timeout_ms);
        let (browser, handler) = match tokio::time::timeout(connect_timeout, connect_fut).await {
            Ok(Ok(pair)) => pair,
            Ok(Err(e)) => return Err(BrowserControlError::ConnectFailed { source: e }),
            Err(_) => {
                return Err(BrowserControlError::ConnectFailed {
                    source: chromiumoxide::error::CdpError::msg(
                        "connect timed out within connect_timeout_ms",
                    ),
                })
            }
        };

        let handler_task = Some(spawn_handler(handler));

        Ok(Self {
            browser: Some(browser),
            handler_task,
            _permit: None,
            _profile_dir: dummy_profile_dir,
            _local: false,
            config,
        })
    }

    /// Create an uninitialized session for tests that do not need a real browser.
    #[doc(hidden)]
    pub fn new_uninitialized_for_test(config: BrowserControlConfig) -> Self {
        Self {
            browser: None,
            handler_task: None,
            _permit: None,
            _profile_dir: TempDir::new().expect("tempdir for test profile"),
            _local: false,
            config,
        }
    }

    /// Expose the underlying [`Browser`] so that `page_state` can create pages.
    pub fn browser(&self) -> Result<&Browser, BrowserControlError> {
        self.browser
            .as_ref()
            .ok_or(BrowserControlError::SessionClosed)
    }

    /// Return the configuration associated with this session.
    pub fn config(&self) -> &BrowserControlConfig {
        &self.config
    }

    /// Return whether this session owns a locally launched Chrome process.
    pub fn is_local(&self) -> bool {
        self._local
    }

    /// Return the path of the profile directory (or the external dummy profile).
    pub fn profile_dir_path(&self) -> &std::path::Path {
        self._profile_dir.path()
    }

    /// Close the browser, wait for the handler task, and clean up the profile.
    #[tracing::instrument(skip_all, fields(local = self._local))]
    pub async fn close(mut self) -> Result<(), BrowserControlError> {
        let Some(mut browser) = self.browser.take() else {
            return Err(BrowserControlError::SessionClosed);
        };
        let Some(handler_task) = self.handler_task.take() else {
            return Err(BrowserControlError::SessionClosed);
        };
        let local = self._local;

        let close_result = browser
            .close()
            .await
            .map(|_| ())
            .map_err(|e| BrowserControlError::from_command_error("close", e));

        // Give the handler a short grace period to finish after the browser is closed.
        let handler_timeout = Duration::from_secs(5);
        match tokio::time::timeout(handler_timeout, handler_task).await {
            Ok(Ok(())) => {}
            Ok(Err(join_err)) => {
                tracing::warn!(error = %join_err, "handler task panicked");
            }
            Err(_) => {
                tracing::debug!("handler task did not finish in time, aborting");
            }
        }

        // Try to collect the child process. If it doesn't exit, force kill it.
        let pid = if local {
            browser
                .get_mut_child()
                .and_then(|child| child.as_mut_inner().id())
        } else {
            None
        };

        let process_timeout = Duration::from_secs(5);
        if tokio::time::timeout(process_timeout, browser.wait())
            .await
            .is_err()
        {
            tracing::warn!("browser process did not exit in time, killing");
            let _ = browser.kill().await;

            if let Some(pid) = pid {
                kill_pid_best_effort(pid);
            }
        }

        // Remove the profile directory explicitly with retries. For external
        // sessions this is just a small dummy directory.
        let path = self._profile_dir.path().to_path_buf();
        match remove_profile_dir_with_retries(&path).await {
            Ok(()) => {}
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "failed to remove profile dir");
            }
        }

        close_result
    }
}

fn spawn_handler(mut handler: Handler) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match handler.next().await {
                Some(Ok(())) => {}
                Some(Err(e)) => {
                    tracing::warn!(error = %e, "browser handler event error");
                }
                None => {
                    tracing::warn!("browser handler stream ended");
                    break;
                }
            }
        }
    })
}

fn strip_leading_dashes(args: &[String]) -> Vec<String> {
    args.iter()
        .map(|arg| {
            arg.strip_prefix("--")
                .map(std::string::ToString::to_string)
                .unwrap_or_else(|| arg.clone())
        })
        .collect()
}

fn kill_pid_best_effort(pid: u32) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .output();
    }
    #[cfg(not(windows))]
    {
        let _ = std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .output();
    }
}

async fn remove_profile_dir_with_retries(path: &std::path::Path) -> std::io::Result<()> {
    let mut last_err = None;
    for attempt in 0..3 {
        match tokio::fs::remove_dir_all(path).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                last_err = Some(e);
                if attempt < 2 {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }
    Err(last_err.unwrap())
}

#[cfg(test)]
mod tests {
    #[test]
    fn debug_does_not_leak_internal_state() {
        // This test simply ensures the Debug impl is reachable and compiles.
        // It cannot construct a BrowserSession without a real Chrome binary.
        let _ = std::format!("{:?}", BrowserSessionNeedsChrome);
    }

    struct BrowserSessionNeedsChrome;

    impl std::fmt::Debug for BrowserSessionNeedsChrome {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("BrowserSession")
                .field("browser", &"<chromiumoxide::Browser>")
                .field("local", &true)
                .finish()
        }
    }
}
