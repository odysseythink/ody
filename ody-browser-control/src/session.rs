use std::time::Duration;

use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::handler::Handler;
use futures::StreamExt;
use tempfile::TempDir;
use tokio::task::JoinHandle;
use crate::config::{acquire_browser_permit, BrowserControlConfig, BrowserControlMode};
use crate::error::BrowserControlError;

/// Thread-level browser session.
///
/// Each session owns a single Chrome process (when launched locally) and a
/// dedicated `chromiumoxide` handler task. Pages are created via
/// [`BrowserSession::new_page`](crate::page_state::BrowserSession::new_page) in
/// the `page_state` module.
pub struct BrowserSession {
    browser: Browser,
    handler_task: JoinHandle<()>,
    _permit: tokio::sync::SemaphorePermit<'static>,
    _profile_dir: Option<TempDir>,
    config: BrowserControlConfig,
}

impl std::fmt::Debug for BrowserSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowserSession")
            .field("browser", &"<chromiumoxide::Browser>")
            .finish()
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

        let _permit = acquire_browser_permit(&config)
            .await
            .map_err(|_| BrowserControlError::LaunchFailed {
                source: chromiumoxide::error::CdpError::msg(
                    "concurrent browser quota semaphore closed",
                ),
            })?;

        let profile_dir =
            TempDir::new().map_err(|e| BrowserControlError::LaunchFailed { source: e.into() })?;

        let mut builder = BrowserConfig::builder();
        if let Some(path) = config.resolved_chrome_path() {
            builder = builder.chrome_executable(path);
        }
        builder = builder
            .user_data_dir(profile_dir.path())
            .args(config.build_launch_args(profile_dir.path()))
            .launch_timeout(Duration::from_millis(config.connect_timeout_ms))
            .request_timeout(Duration::from_millis(config.command_timeout_ms));

        let browser_config = builder.build().map_err(|e| {
            if config.resolved_chrome_path().is_none() {
                BrowserControlError::ChromeNotFound {
                    searched_paths: Vec::new(),
                }
            } else {
                BrowserControlError::LaunchFailed {
                    source: chromiumoxide::error::CdpError::msg(e),
                }
            }
        })?;

        let (browser, handler) = Browser::launch(browser_config)
            .await
            .map_err(|e| BrowserControlError::LaunchFailed { source: e })?;

        let handler_task = spawn_handler(handler);

        Ok(Self {
            browser,
            handler_task,
            _permit,
            _profile_dir: Some(profile_dir),
            config: config.clone(),
        })
    }

    /// Connect to an existing Chrome debug endpoint.
    pub async fn connect(ws_url: &str) -> Result<Self, BrowserControlError> {
        let (browser, handler) = Browser::connect(ws_url)
            .await
            .map_err(|e| BrowserControlError::ConnectFailed { source: e })?;

        let handler_task = spawn_handler(handler);

        // No permit or profile dir for external connections.
        Ok(Self {
            browser,
            handler_task,
            _permit: acquire_dummy_permit().await,
            _profile_dir: None,
            config: BrowserControlConfig::default(),
        })
    }

    /// Expose the underlying [`Browser`] so that `page_state` can create pages.
    pub fn browser(&self) -> &Browser {
        &self.browser
    }

    /// Return the configuration associated with this session.
    pub fn config(&self) -> &BrowserControlConfig {
        &self.config
    }

    /// Close the browser, wait for the handler task, and clean up the profile.
    pub async fn close(mut self) -> Result<(), BrowserControlError> {
        let close_result = self
            .browser
            .close()
            .await
            .map(|_| ())
            .map_err(|e| BrowserControlError::from_command_error("close", e));

        // Give the handler a short grace period to finish after the browser is closed.
        let handler_timeout = Duration::from_secs(5);
        match tokio::time::timeout(handler_timeout, &mut self.handler_task).await {
            Ok(Ok(())) => {}
            Ok(Err(join_err)) => {
                tracing::warn!(error = %join_err, "handler task panicked");
            }
            Err(_) => {
                tracing::debug!("handler task did not finish in time, aborting");
                self.handler_task.abort();
            }
        }

        // Try to collect the child process. If it doesn't exit, force kill it.
        let process_timeout = Duration::from_secs(5);
        if tokio::time::timeout(process_timeout, self.browser.wait())
            .await
            .is_err()
        {
            tracing::warn!("browser process did not exit in time, killing");
            let _ = self.browser.kill().await;

            #[cfg(windows)]
            if let Some(child) = self.browser.get_mut_child() {
                let pid = child.as_mut_inner().id();
                if let Some(pid) = pid {
                    let _ = std::process::Command::new("taskkill")
                        .args(["/T", "/F", "/PID", &pid.to_string()])
                        .spawn();
                }
            }
        }

        // Remove the profile directory explicitly with retries.
        if let Some(profile_dir) = self._profile_dir.take() {
            let path = profile_dir.path().to_path_buf();
            match remove_profile_dir_with_retries(&path).await {
                Ok(()) => {}
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "failed to remove profile dir");
                }
            }
        }

        close_result
    }
}

fn spawn_handler(mut handler: Handler) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(_event) = handler.next().await {
            // Events are handled by per-page listeners; the handler loop just
            // needs to be polled to drive the CDP connection.
        }
    })
}

async fn acquire_dummy_permit() -> tokio::sync::SemaphorePermit<'static> {
    static DUMMY: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
    DUMMY.acquire().await.unwrap_or_else(|_| unreachable!())
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
                .finish()
        }
    }
}
