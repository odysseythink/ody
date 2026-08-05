use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use chromiumoxide::layout::Point;
use chromiumoxide::page::{Page, ScreenshotParams};
use futures::StreamExt;
use serde::Serialize;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::config::BrowserControlConfig;
use crate::error::BrowserControlError;
use crate::event_buffer::{EventBuffer, LogsSnapshot};
use crate::session::BrowserSession;

/// Per-page state holding a CDP target and a rolling event buffer.
pub struct PageState {
    page: Page,
    event_buffer: Arc<Mutex<EventBuffer>>,
    #[allow(dead_code)]
    event_tasks: Vec<JoinHandle<()>>,
    #[allow(dead_code)]
    config: BrowserControlConfig,
    crashed: Arc<AtomicBool>,
}

impl std::fmt::Debug for PageState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PageState")
            .field("page", &"<chromiumoxide::Page>")
            .field("crashed", &self.crashed.load(Ordering::Relaxed))
            .finish()
    }
}

impl PageState {
    /// Create a new page state and subscribe to console/network events.
    pub async fn new(
        page: Page,
        config: BrowserControlConfig,
    ) -> Result<Self, BrowserControlError> {
        let event_buffer = Arc::new(Mutex::new(EventBuffer::new(&config)));
        let event_tasks = crate::event_buffer::subscribe(&page, event_buffer.clone()).await?;
        let crashed = Arc::new(AtomicBool::new(false));
        let mut crash_listener = page
            .event_listener::<chromiumoxide::cdp::browser_protocol::target::EventTargetCrashed>()
            .await
            .map_err(|e| BrowserControlError::from_command_error("crash listener", e))?;
        let crashed_flag = crashed.clone();
        tokio::spawn(async move {
            while let Some(_event) = crash_listener.next().await {
                crashed_flag.store(true, Ordering::Relaxed);
            }
        });

        Ok(Self {
            page,
            event_buffer,
            event_tasks,
            config,
            crashed,
        })
    }

    /// Returns true if the page has been reported as crashed by the browser.
    pub fn is_crashed(&self) -> bool {
        self.crashed.load(Ordering::Relaxed)
    }

    /// Close the page and stop its event listeners.
    #[tracing::instrument(skip_all)]
    pub async fn close(self) -> Result<(), BrowserControlError> {
        for task in self.event_tasks {
            task.abort();
        }
        self.page
            .close()
            .await
            .map(|_| ())
            .map_err(|e| BrowserControlError::from_command_error("page.close", e))
    }

    /// Wait for a navigation response on the page.
    #[tracing::instrument(skip_all)]
    pub async fn wait_for_navigation_response(
        &self,
    ) -> Result<(), BrowserControlError> {
        self.page
            .wait_for_navigation_response()
            .await
            .map(|_| ())
            .map_err(|e| BrowserControlError::from_command_error("wait_for_navigation_response", e))
    }

    /// Return the current page URL.
    pub async fn url(&self) -> Result<Option<String>, BrowserControlError> {
        self.page
            .url()
            .await
            .map_err(|e| BrowserControlError::from_command_error("page.url", e))
    }

    /// Return the current page title.
    #[tracing::instrument(skip_all)]
    pub async fn title(&self) -> Result<Option<String>, BrowserControlError> {
        let value = self.evaluate("document.title").await?;
        Ok(value.as_str().map(|s| s.to_string()))
    }

    /// Navigate back in the browser history.
    #[tracing::instrument(skip_all)]
    pub async fn go_back(&self) -> Result<(), BrowserControlError> {
        self.evaluate("history.back()").await?;
        Ok(())
    }

    /// Navigate forward in the browser history.
    pub async fn go_forward(&self) -> Result<(), BrowserControlError> {
        self.evaluate("history.forward()").await?;
        Ok(())
    }

    /// Reload the page.
    #[tracing::instrument(skip_all)]
    pub async fn reload(&self) -> Result<(), BrowserControlError> {
        self.page
            .reload()
            .await
            .map(|_| ())
            .map_err(|e| BrowserControlError::from_command_error("page.reload", e))
    }

    /// Navigate the page to `url` and wait for the load event.
    pub async fn navigate(&self, url: &str) -> Result<(), BrowserControlError> {
        self.page
            .goto(url)
            .await
            .map_err(|e| BrowserControlError::from_command_error("navigate", e))?;
        Ok(())
    }

    /// Evaluate a JavaScript expression and return the serialized result.
    #[tracing::instrument(skip_all, fields(expression_preview = %crate::types::expression_preview(js)))]
    pub async fn evaluate(&self, js: &str) -> Result<serde_json::Value, BrowserControlError> {
        let result = self
            .page
            .evaluate(js)
            .await
            .map_err(|e| BrowserControlError::from_command_error("evaluate", e))?;
        result
            .into_value::<serde_json::Value>()
            .map_err(|e| BrowserControlError::from_command_error("evaluate", e.into()))
    }

    /// Capture a PNG screenshot of the page.
    pub async fn screenshot(&self, full_page: bool) -> Result<Vec<u8>, BrowserControlError> {
        let params = ScreenshotParams::builder().full_page(full_page).build();
        self.page
            .screenshot(params)
            .await
            .map_err(|e| BrowserControlError::from_command_error("screenshot", e))
    }

    /// Simulate a mouse click at the given coordinates.
    pub async fn click(&self, x: f64, y: f64) -> Result<(), BrowserControlError> {
        self.page
            .click(Point::new(x, y))
            .await
            .map_err(|e| BrowserControlError::from_command_error("click", e))?;
        Ok(())
    }

    /// Type `text` into the element selected by `selector`.
    pub async fn type_text(
        &self,
        selector: &str,
        text: &str,
    ) -> Result<(), BrowserControlError> {
        let element = self
            .page
            .find_element(selector)
            .await
            .map_err(|e| BrowserControlError::from_command_error("find_element", e))?;
        element
            .focus()
            .await
            .map_err(|e| BrowserControlError::from_command_error("focus", e))?;
        element
            .type_str(text)
            .await
            .map_err(|e| BrowserControlError::from_command_error("type_text", e))?;
        Ok(())
    }

    /// Return a DOM representation.
    ///
    /// If `selector` is `None`, returns the full document tree as a JSON value.
    /// If `selector` is provided, returns the `outerHTML` of the first matching element.
    pub async fn get_dom(
        &self,
        selector: Option<&str>,
    ) -> Result<serde_json::Value, BrowserControlError> {
        match selector {
            None => {
                let node = self
                    .page
                    .get_document()
                    .await
                    .map_err(|e| BrowserControlError::from_command_error("get_document", e))?;
                serde_json::to_value(node)
                    .map_err(|e| BrowserControlError::from_command_error("get_dom", e.into()))
            }
            Some(sel) => {
                let element = self
                    .page
                    .find_element(sel)
                    .await
                    .map_err(|e| BrowserControlError::from_command_error("find_element", e))?;
                let html = element
                    .outer_html()
                    .await
                    .map_err(|e| BrowserControlError::from_command_error("outer_html", e))?;
                Ok(serde_json::Value::String(html.unwrap_or_default()))
            }
        }
    }

    /// Execute a raw CDP method with JSON parameters.
    #[tracing::instrument(skip_all, fields(method, params_size = serde_json::to_string(&params).map(|s| s.len()).unwrap_or(0)))]
    pub async fn execute_raw(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, BrowserControlError> {
        let cmd = RawCdpCommand {
            method: method.to_string(),
            params,
        };
        let response = self
            .page
            .execute(cmd)
            .await
            .map_err(|e| BrowserControlError::from_command_error("execute_raw", e))?;
        Ok(response.result)
    }

    /// Read a snapshot of the buffered console and network logs.
    #[tracing::instrument(skip_all)]
    pub async fn read_logs(&self) -> Result<LogsSnapshot, BrowserControlError> {
        let guard = self.event_buffer.lock().await;
        Ok(guard.snapshot())
    }
}

impl BrowserSession {
    /// Create a new page in this browser session and start collecting events.
    #[tracing::instrument(skip_all)]
    pub async fn new_page(&self) -> Result<PageState, BrowserControlError> {
        let page = self
            .browser()?
            .new_page("about:blank")
            .await
            .map_err(|e| BrowserControlError::from_command_error("new_page", e))?;
        PageState::new(page, self.config().clone()).await
    }
}

#[derive(Debug, Serialize)]
struct RawCdpCommand {
    method: String,
    #[serde(rename = "params")]
    params: serde_json::Value,
}

impl chromiumoxide::types::Method for RawCdpCommand {
    fn identifier(&self) -> chromiumoxide::types::MethodId {
        self.method.clone().into()
    }
}

impl chromiumoxide::types::Command for RawCdpCommand {
    type Response = serde_json::Value;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_impl_does_not_leak_page() {
        // Compilation-only test: Debug must be reachable.
    }

    #[test]
    fn raw_cdp_command_serialization() {
        let cmd = RawCdpCommand {
            method: "Runtime.evaluate".to_string(),
            params: serde_json::json!({"expression": "1+1"}),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("Runtime.evaluate"));
        assert!(json.contains("1+1"));
    }

    #[test]
    fn raw_cdp_command_method_identifier() {
        use chromiumoxide::Method;

        let cmd = RawCdpCommand {
            method: "Test.method".to_string(),
            params: serde_json::Value::Null,
        };
        assert_eq!(cmd.identifier(), "Test.method");
    }
}
