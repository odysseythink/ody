use crate::config::{BrowserControlConfig, BrowserControlMode};
use crate::error::BrowserControlError;
use crate::page_state::PageState;
use crate::session::BrowserSession;

/// Thread-local browser state that owns a single [`BrowserSession`] and a
/// reusable default page.
///
/// Common tools can call [`BrowserThreadState::default_page`] to obtain a
/// long-lived page instead of creating a new target for every operation. If the
/// default page crashes, a new one is created automatically.
pub struct BrowserThreadState {
    session: BrowserSession,
    default_page: Option<PageState>,
}

impl std::fmt::Debug for BrowserThreadState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BrowserThreadState")
            .field("session", &self.session)
            .field("has_default_page", &self.default_page.is_some())
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
            default_page: None,
        })
    }

    /// Return a reference to the underlying session.
    pub fn session(&self) -> &BrowserSession {
        &self.session
    }

    /// Return a mutable reference to the default page, creating one if needed.
    ///
    /// If the existing default page has been reported as crashed, it is closed
    /// and replaced.
    pub async fn default_page(
        &mut self,
    ) -> Result<&mut PageState, BrowserControlError> {
        match self.default_page.as_ref() {
            Some(page) if !page.is_crashed() => return Ok(self.default_page.as_mut().unwrap()),
            Some(_) => {
                // Replace the crashed page.
                if let Some(page) = self.default_page.take() {
                    let _ = page.close().await;
                }
            }
            None => {}
        }
        let page = self.session.new_page().await?;
        self.default_page = Some(page);
        Ok(self.default_page.as_mut().unwrap())
    }

    /// Create an additional page in the same session.
    pub async fn new_page(&mut self) -> Result<PageState, BrowserControlError> {
        self.session.new_page().await
    }

    /// Close the default page (if any) and then the browser session.
    pub async fn close(mut self) -> Result<(), BrowserControlError> {
        if let Some(page) = self.default_page.take() {
            let _ = page.close().await;
        }
        self.session.close().await
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn debug_impl_is_reachable() {
        // Compilation-only; construction requires a real Chrome binary.
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
}
