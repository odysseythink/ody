use std::sync::Arc;

use ody_browser_control::{all_tools, BrowserThreadState};
use ody_core::config::Config;
use ody_extension_api::{
    ConfigContributor, ExtensionData, ExtensionFuture, ExtensionRegistryBuilder,
    ThreadLifecycleContributor, ThreadStartInput, ToolCall, ToolContributor,
};
use ody_features::Feature;
use ody_tools::ToolExecutor;

/// Per-thread browser state stored in the extension data map.
#[derive(Clone)]
struct BrowserControlHandle {
    state: Arc<BrowserThreadState>,
    full_cdp_access: bool,
}

/// App-server extension that wires the browser tool namespace into the thread
/// runtime when the `BrowserUse`/`ComputerUse` features are enabled and a
/// `[services.browser]` configuration is present.
#[derive(Clone)]
pub struct BrowserControlExtension;

impl BrowserControlExtension {
    fn browser_enabled(config: &Config) -> bool {
        config.features.enabled(Feature::BrowserUse)
            || config.features.enabled(Feature::ComputerUse)
    }
}

impl ThreadLifecycleContributor<Config> for BrowserControlExtension {
    fn on_thread_start<'a>(
        &'a self,
        input: ThreadStartInput<'a, Config>,
    ) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            if !Self::browser_enabled(input.config) {
                return;
            }
            let Some(browser_cfg) = input
                .config
                .services
                .as_ref()
                .and_then(|services| services.browser.as_ref())
            else {
                return;
            };
            let state = match BrowserThreadState::new(browser_cfg.clone()).await {
                Ok(state) => Arc::new(state),
                Err(err) => {
                    tracing::warn!(error = %err, "failed to start browser thread state");
                    return;
                }
            };
            let full_cdp_access = input.config.features.enabled(Feature::BrowserUseFullCdpAccess);
            input.thread_store.insert(BrowserControlHandle {
                state,
                full_cdp_access,
            });
        })
    }
}

impl ConfigContributor<Config> for BrowserControlExtension {
    fn on_config_changed(
        &self,
        _session_store: &ExtensionData,
        thread_store: &ExtensionData,
        previous_config: &Config,
        new_config: &Config,
    ) {
        let had_browser = Self::browser_enabled(previous_config)
            && previous_config
                .services
                .as_ref()
                .and_then(|s| s.browser.as_ref())
                .is_some();
        let has_browser = Self::browser_enabled(new_config)
            && new_config
                .services
                .as_ref()
                .and_then(|s| s.browser.as_ref())
                .is_some();
        if had_browser && !has_browser {
            let _ = thread_store.remove::<BrowserControlHandle>();
        }
    }
}

impl ToolContributor for BrowserControlExtension {
    fn tools(
        &self,
        _session_store: &ExtensionData,
        thread_store: &ExtensionData,
    ) -> Vec<Arc<dyn ToolExecutor<ToolCall>>> {
        let Some(handle) = thread_store.get::<BrowserControlHandle>() else {
            return Vec::new();
        };
        let mut tools = all_tools(Arc::clone(&handle.state));
        if !handle.full_cdp_access {
            tools.retain(|tool| {
                let name = tool.tool_name();
                !(name.namespace.as_deref() == Some("browser") && name.name == "execute_raw_cdp")
            });
        }
        tools
    }
}

pub fn install(registry: &mut ExtensionRegistryBuilder<Config>) {
    let extension = Arc::new(BrowserControlExtension);
    registry.thread_lifecycle_contributor(extension.clone());
    registry.config_contributor(extension.clone());
    registry.tool_contributor(extension);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ody_core::config::ConfigBuilder;
    use ody_web_search::config::ServicesConfig;

    fn services_with_browser() -> ServicesConfig {
        ServicesConfig {
            web_search: None,
            browser: Some(ody_browser_control::BrowserControlConfig::default()),
        }
    }

    #[test]
    fn tools_returns_empty_when_no_browser_handle() {
        let session_store = ExtensionData::new("session");
        let thread_store = ExtensionData::new("thread");
        let extension = BrowserControlExtension;
        let tools = extension.tools(&session_store, &thread_store);
        assert!(tools.is_empty());
    }

    #[tokio::test]
    async fn config_removes_handle_when_browser_disabled() {
        let thread_store = ExtensionData::new("thread");
        thread_store.insert(BrowserControlHandle {
            state: uninitialized_state(),
            full_cdp_access: false,
        });

        let previous_config = config_with_browser(true).await;
        let new_config = config_with_browser(false).await;
        BrowserControlExtension.on_config_changed(
            &ExtensionData::new("session"),
            &thread_store,
            &previous_config,
            &new_config,
        );
        assert!(thread_store.get::<BrowserControlHandle>().is_none());
    }

    async fn config_with_browser(enabled: bool) -> Config {
        let mut config = ConfigBuilder::default().build().await.expect("test config");
        config.services = Some(services_with_browser());
        config
            .features
            .set_enabled(Feature::BrowserUse, enabled)
            .expect("toggle BrowserUse");
        config
            .features
            .set_enabled(Feature::ComputerUse, enabled)
            .expect("toggle ComputerUse");
        config
    }

    fn uninitialized_state() -> Arc<BrowserThreadState> {
        Arc::new(
            BrowserThreadState::new_uninitialized_for_test(
                ody_browser_control::BrowserControlConfig::default(),
            )
            .expect("uninitialized thread state"),
        )
    }

    #[test]
    fn tools_filters_raw_cdp_without_full_access() {
        let session_store = ExtensionData::new("session");
        let thread_store = ExtensionData::new("thread");
        thread_store.insert(BrowserControlHandle {
            state: uninitialized_state(),
            full_cdp_access: false,
        });
        let extension = BrowserControlExtension;
        let tools = extension.tools(&session_store, &thread_store);
        assert!(
            !tools.iter().any(|t| {
                let name = t.tool_name();
                name.namespace.as_deref() == Some("browser") && name.name == "execute_raw_cdp"
            }),
            "execute_raw_cdp should be filtered without BrowserUseFullCdpAccess"
        );
        assert!(tools.iter().any(|t| {
            let name = t.tool_name();
            name.namespace.as_deref() == Some("browser") && name.name == "navigate"
        }));
    }

    #[test]
    fn tools_includes_raw_cdp_with_full_access() {
        let session_store = ExtensionData::new("session");
        let thread_store = ExtensionData::new("thread");
        thread_store.insert(BrowserControlHandle {
            state: uninitialized_state(),
            full_cdp_access: true,
        });
        let extension = BrowserControlExtension;
        let tools = extension.tools(&session_store, &thread_store);
        assert!(tools.iter().any(|t| {
            let name = t.tool_name();
            name.namespace.as_deref() == Some("browser") && name.name == "execute_raw_cdp"
        }));
        assert!(tools.iter().any(|t| {
            let name = t.tool_name();
            name.namespace.as_deref() == Some("browser") && name.name == "navigate"
        }));
    }
}
