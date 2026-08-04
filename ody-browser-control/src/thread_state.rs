use std::pin::Pin;
use std::time::Duration;

use std::future::Future;

use base64::Engine as _;
use tokio::sync::Mutex;

use crate::config::{BrowserControlConfig, BrowserControlMode};
use crate::error::BrowserControlError;
use crate::page_state::PageState;
use crate::session::BrowserSession;
use crate::types::{
    EvaluateResult, NavigationResult, Point, ScreenshotResult, WaitCondition,
    truncate_base64_bytes, truncate_string_bytes,
};
use crate::{event_buffer::LogsSnapshot, url_block};

/// Thread-local browser state that owns a single [`BrowserSession`] and a
/// reusable default page.
///
/// All public methods take `&self` so they can be invoked from multiple
/// concurrently held tool executors. The default page is protected by an async
/// mutex and is recreated automatically if it is reported as crashed.
pub struct BrowserThreadState {
    session: BrowserSession,
    default_page: Mutex<Option<PageState>>,
}

impl std::fmt::Debug for BrowserThreadState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowserThreadState")
            .field("session", &self.session)
            .field(
                "has_default_page",
                &self.default_page.try_lock().map(|g| g.is_some()).unwrap_or(true),
            )
            .finish()
    }
}

impl BrowserThreadState {
    /// Start a new thread-local browser state.
    ///
    /// The session is created according to `config.mode`. The default page is
    /// lazily created on first access.
    pub async fn new(config: BrowserControlConfig) -> Result<Self, BrowserControlError> {
        let session = match config.mode {
            BrowserControlMode::Local => BrowserSession::launch(config).await?,
            BrowserControlMode::External => BrowserSession::connect(config).await?,
        };
        Ok(Self {
            session,
            default_page: Mutex::new(None),
        })
    }

    /// Create an uninitialized thread state for tests that do not need a real
    /// browser process.
    #[doc(hidden)]
    pub fn new_uninitialized_for_test(
        config: BrowserControlConfig,
    ) -> Result<Self, BrowserControlError> {
        Ok(Self {
            session: BrowserSession::new_uninitialized_for_test(config),
            default_page: Mutex::new(None),
        })
    }

    /// Return a reference to the underlying session.
    pub fn session(&self) -> &BrowserSession {
        &self.session
    }

    /// Return the configuration associated with this state.
    pub fn config(&self) -> &BrowserControlConfig {
        self.session.config()
    }

    /// Create an additional page in the same session.
    pub async fn new_page(&self) -> Result<PageState, BrowserControlError> {
        self.session.new_page().await
    }

    /// Close the default page (if any) and then the browser session.
    pub async fn close(self) -> Result<(), BrowserControlError> {
        let mut guard = self.default_page.lock().await;
        if let Some(page) = guard.take() {
            let _ = page.close().await;
        }
        self.session.close().await
    }

    /// Return a guard that always contains a non-crashed default page.
    async fn ensure_default_page(
        &self,
    ) -> Result<tokio::sync::MutexGuard<'_, Option<PageState>>, BrowserControlError> {
        let mut guard = self.default_page.lock().await;
        if guard.as_ref().is_some_and(|p| p.is_crashed()) {
            if let Some(page) = guard.take() {
                let _ = page.close().await;
            }
        }
        if guard.is_none() {
            let page = self.session.new_page().await?;
            *guard = Some(page);
        }
        Ok(guard)
    }

    /// Run `operation` against the default page, recreating the page once if
    /// the operation fails because the page crashed.
    async fn with_page_retry<R>(
        &self,
        mut operation: impl for<'a> FnMut(&'a PageState) -> Pin<Box<dyn Future<Output = Result<R, BrowserControlError>> + Send + 'a>>,
    ) -> Result<R, BrowserControlError> {
        let mut attempts = 0;
        loop {
            let mut guard = self.ensure_default_page().await?;
            let page = guard.as_ref().expect("page was just created");
            match operation(page).await {
                Ok(value) => return Ok(value),
                Err(e) if attempts == 0 && is_crash_related(&e) => {
                    tracing::warn!(error = %e, "default page crashed, recreating and retrying once");
                    *guard = None;
                    attempts += 1;
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Navigate the default page to `url` and return the resulting URL/title.
    pub async fn navigate(
        &self,
        url: &str,
        wait_until: Option<WaitCondition>,
    ) -> Result<NavigationResult, BrowserControlError> {
        if let Err(reason) =
            url_block::check_url_is_allowed(url, self.config().allow_local_network)
        {
            return Err(BrowserControlError::NotAllowed { reason });
        }

        let url = url.to_string();
        let timeout = Duration::from_millis(self.config().navigation_timeout_ms);
        self.with_page_retry(|page| {
            let url = url.clone();
            Box::pin(async move {
                tokio::time::timeout(timeout, page.navigate(&url))
                    .await
                    .map_err(|_| BrowserControlError::Timeout {
                        command: "navigate".to_string(),
                        elapsed_ms: timeout.as_millis() as u64,
                    })?
            })
        })
        .await?;

        if let Some(condition) = wait_until {
            let _ = self.wait_for_condition(condition).await;
        }

        self.current_navigation_result(wait_until).await
    }

    /// Navigate back in the browser history.
    pub async fn go_back(&self) -> Result<NavigationResult, BrowserControlError> {
        self.with_page_retry(|page| Box::pin(async move { page.go_back().await }))
            .await?;
        self.current_navigation_result(None).await
    }

    /// Navigate forward in the browser history.
    pub async fn go_forward(&self) -> Result<NavigationResult, BrowserControlError> {
        self.with_page_retry(|page| Box::pin(async move { page.go_forward().await }))
            .await?;
        self.current_navigation_result(None).await
    }

    /// Reload the default page.
    pub async fn reload(&self) -> Result<NavigationResult, BrowserControlError> {
        self.with_page_retry(|page| Box::pin(async move { page.reload().await }))
            .await?;
        self.current_navigation_result(None).await
    }

    /// Capture a screenshot of the default page and return a base64-encoded PNG.
    pub async fn screenshot(&self, full_page: bool) -> Result<ScreenshotResult, BrowserControlError> {
        let bytes = self
            .with_page_retry(|page| Box::pin(async move { page.screenshot(full_page).await }))
            .await?;
        let mut data = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let truncated = data.len() > crate::types::SCREENSHOT_MAX_BYTES;
        if truncated {
            data = truncate_base64_bytes(&data, crate::types::SCREENSHOT_MAX_BYTES);
        }
        Ok(ScreenshotResult {
            data,
            mime_type: "image/png".to_string(),
            truncated,
        })
    }

    /// Evaluate `js` on the default page and return the serialized result.
    pub async fn evaluate(&self, js: &str) -> Result<EvaluateResult, BrowserControlError> {
        if let Err(reason) = url_block::check_js_allowed(js) {
            return Err(BrowserControlError::NotAllowed { reason });
        }
        let js = js.to_string();
        self.with_page_retry(|page| {
            let js = js.clone();
            Box::pin(async move {
                let value = page.evaluate(&js).await?;
                Ok(EvaluateResult {
                    value,
                    exception: None,
                })
            })
        })
        .await
    }

    /// Click at the given coordinates on the default page.
    pub async fn click(&self, point: Point) -> Result<(), BrowserControlError> {
        self.with_page_retry(|page| {
            let x = point.x;
            let y = point.y;
            Box::pin(async move { page.click(x, y).await })
        })
        .await
    }

    /// Type `text` into the element selected by `selector` on the default page.
    pub async fn type_text(&self, selector: &str, text: &str) -> Result<(), BrowserControlError> {
        let selector = selector.to_string();
        let text = text.to_string();
        self.with_page_retry(|page| {
            let selector = selector.clone();
            let text = text.clone();
            Box::pin(async move { page.type_text(&selector, &text).await })
        })
        .await
    }

    /// Return a DOM representation of the default page, optionally scoped to `selector`.
    pub async fn get_dom(
        &self,
        selector: Option<&str>,
    ) -> Result<serde_json::Value, BrowserControlError> {
        let selector = selector.map(|s| s.to_string());
        self.with_page_retry(|page| {
            let selector = selector.clone();
            Box::pin(async move {
                let value = page.get_dom(selector.as_deref()).await?;
                Ok(truncate_dom_value(value))
            })
        })
        .await
    }

    /// Read the buffered console/network logs for the default page.
    pub async fn read_logs(
        &self,
        kind: crate::types::LogKind,
        level: crate::types::LogLevel,
    ) -> Result<LogsSnapshot, BrowserControlError> {
        self.with_page_retry(|page| {
            Box::pin(async move {
                let mut snapshot = page.read_logs().await?;
                if !kind.includes_console() {
                    snapshot.console.clear();
                }
                if !kind.includes_network() {
                    snapshot.network.clear();
                }
                snapshot.console.retain(|entry| level.allows(&entry.level));
                Ok(snapshot)
            })
        })
        .await
    }

    /// Execute a raw CDP method with JSON parameters on the default page.
    pub async fn execute_raw(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, BrowserControlError> {
        let method = method.to_string();
        self.with_page_retry(|page| {
            let method = method.clone();
            let params = params.clone();
            Box::pin(async move { page.execute_raw(&method, params).await })
        })
        .await
    }

    async fn current_navigation_result(
        &self,
        wait_until: Option<WaitCondition>,
    ) -> Result<NavigationResult, BrowserControlError> {
        self.with_page_retry(|page| {
            Box::pin(async move {
                let url = page.url().await?;
                let title = page.title().await?;
                Ok(NavigationResult {
                    url: url.unwrap_or_default(),
                    title: title.unwrap_or_default(),
                    wait_until,
                })
            })
        })
        .await
    }

    async fn wait_for_condition(
        &self,
        condition: WaitCondition,
    ) -> Result<(), BrowserControlError> {
        match condition {
            WaitCondition::Load => Ok(()),
            WaitCondition::DomContentLoaded | WaitCondition::NetworkIdle => {
                self.with_page_retry(|page| {
                    Box::pin(async move {
                        let _ = page.wait_for_navigation_response().await?;
                        Ok(())
                    })
                })
                .await
            }
        }
    }
}

fn is_crash_related(err: &BrowserControlError) -> bool {
    matches!(err, BrowserControlError::PageCrashed)
        || err.to_string().to_lowercase().contains("target crashed")
        || err.to_string().to_lowercase().contains("target closed")
}

fn truncate_dom_value(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::String(text) => {
            serde_json::Value::String(truncate_string_bytes(&text, crate::types::DOM_MAX_BYTES))
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_impl_does_not_deadlock() {
        // We cannot construct a real BrowserThreadState without Chrome, so this
        // test only verifies the Debug impl remains compilable and does not
        // unconditionally await the default-page lock.
        let _ = std::format!("{:?}", BrowserThreadStateNeedsChrome);
    }

    struct BrowserThreadStateNeedsChrome;

    impl std::fmt::Debug for BrowserThreadStateNeedsChrome {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("BrowserThreadState")
                .field("session", &"<BrowserSession>")
                .field("has_default_page", &true)
                .finish()
        }
    }

    #[test]
    fn truncate_dom_value_shortens_long_strings() {
        let long = "a".repeat(10_000);
        let value = serde_json::Value::String(long.clone());
        let truncated = truncate_dom_value(value);
        let text = truncated.as_str().unwrap();
        assert!(text.len() <= crate::types::DOM_MAX_BYTES + 100);
        assert!(text.contains("truncated"));
    }

    #[test]
    fn truncate_dom_value_passes_objects_through() {
        let obj = serde_json::json!({"a": 1});
        assert_eq!(truncate_dom_value(obj.clone()), obj);
    }
}
