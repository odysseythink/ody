use std::sync::Arc;

use ody_browser_control::{all_tools, BrowserThreadState};
use ody_core::config::Config;
use ody_extension_api::{
    ConfigContributor, ExtensionData, ExtensionFuture, ExtensionRegistryBuilder,
    ThreadLifecycleContributor, ThreadStartInput, ToolCall, ToolContributor,
};
use ody_features::Feature;
use ody_tools::ToolExecutor;
use ody_tools::ToolName;

/// Per-thread browser state stored in the extension data map.
#[derive(Clone)]
struct BrowserControlHandle {
    state: Arc<BrowserThreadState>,
    full_cdp_access: bool,
    computer_use: bool,
    browser_use: bool,
}

/// App-server extension that wires the browser tool namespace into the thread
/// runtime when the `BrowserUse`/`ComputerUse`/`BrowserUseExternal` features are
/// enabled. A missing `[services.browser]` table falls back to the default
/// browser configuration.
#[derive(Clone)]
pub struct BrowserControlExtension;

impl BrowserControlExtension {
    fn browser_enabled(config: &Config) -> bool {
        config.features.enabled(Feature::BrowserUse)
            || config.features.enabled(Feature::ComputerUse)
            || config.features.enabled(Feature::BrowserUseExternal)
    }

    fn browser_config(config: &Config) -> ody_browser_control::BrowserControlConfig {
        config
            .services
            .as_ref()
            .and_then(|services| services.browser.clone())
            .unwrap_or_default()
    }

    /// Prefer external connection when the `BrowserUseExternal` feature is enabled
    /// and a `connect_url` is configured. Otherwise leave the configured mode alone.
    fn effective_browser_config(config: &Config) -> ody_browser_control::BrowserControlConfig {
        let mut cfg = Self::browser_config(config);
        if config.features.enabled(Feature::BrowserUseExternal) {
            if cfg.connect_url.is_some() {
                cfg.mode = ody_browser_control::BrowserControlMode::External;
            } else {
                tracing::warn!(
                    "BrowserUseExternal is enabled but services.browser.connect_url is missing; falling back to local launch"
                );
            }
        }
        cfg
    }

    fn tool_visibility_flags(config: &Config) -> (bool, bool, bool) {
        let full_cdp_access = config.features.enabled(Feature::BrowserUseFullCdpAccess);
        let computer_use = config.features.enabled(Feature::ComputerUse);
        let browser_use = config.features.enabled(Feature::BrowserUse)
            || config.features.enabled(Feature::BrowserUseExternal);
        (full_cdp_access, computer_use, browser_use)
    }

    /// Return true if `tool_name` should be exposed to the model given the
    /// effective feature flags in `handle`.
    fn is_tool_visible(handle: &BrowserControlHandle, tool_name: &ToolName) -> bool {
        if tool_name.namespace.as_deref() != Some("browser") {
            return true;
        }
        match tool_name.name.as_str() {
            "execute_raw_cdp" => {
                handle.full_cdp_access && (handle.browser_use || handle.computer_use)
            }
            // ComputerUse exposes the direct UI-interaction subset.
            "click" | "type" => handle.computer_use,
            // BrowserUse exposes the browser navigation / inspection subset.
            "navigate" | "go_back" | "go_forward" | "reload" | "evaluate" | "get_dom"
            | "read_logs" => handle.browser_use,
            // Screenshot is useful for both modes.
            "screenshot" => handle.browser_use || handle.computer_use,
            _ => false,
        }
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
            let browser_cfg = Self::effective_browser_config(input.config);
            let state = match BrowserThreadState::new(browser_cfg).await {
                Ok(state) => Arc::new(state),
                Err(err) => {
                    tracing::warn!(error = %err, "failed to start browser thread state");
                    return;
                }
            };
            let (full_cdp_access, computer_use, browser_use) =
                Self::tool_visibility_flags(input.config);
            input.thread_store.insert(BrowserControlHandle {
                state,
                full_cdp_access,
                computer_use,
                browser_use,
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
        let was_enabled = Self::browser_enabled(previous_config);
        let is_enabled = Self::browser_enabled(new_config);

        if !is_enabled {
            let _ = thread_store.remove::<BrowserControlHandle>();
            return;
        }

        let Some(handle) = thread_store.get::<BrowserControlHandle>() else {
            return;
        };

        let old_config = handle.state.config();
        let new_browser_config = Self::effective_browser_config(new_config);
        let needs_restart = old_config.requires_restart(&new_browser_config);
        handle.state.set_config(new_browser_config);
        if needs_restart {
            handle.state.mark_stale();
        }

        let (full_cdp_access, computer_use, browser_use) = Self::tool_visibility_flags(new_config);
        thread_store.insert(BrowserControlHandle {
            state: Arc::clone(&handle.state),
            full_cdp_access,
            computer_use,
            browser_use,
        });
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
        all_tools(Arc::clone(&handle.state))
            .into_iter()
            .filter(|tool| Self::is_tool_visible(&handle, &tool.tool_name()))
            .collect()
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

    fn make_handle(browser_use: bool, computer_use: bool, full_cdp_access: bool) -> BrowserControlHandle {
        BrowserControlHandle {
            state: uninitialized_state(),
            full_cdp_access,
            computer_use,
            browser_use,
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
        thread_store.insert(make_handle(true, true, false));

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

    #[tokio::test]
    async fn config_marks_stale_when_restart_required() {
        let thread_store = ExtensionData::new("thread");
        thread_store.insert(make_handle(true, true, false));

        let previous_config = config_with_browser(true).await;
        let mut new_config = config_with_browser(true).await;
        new_config.services = Some(ServicesConfig {
            web_search: None,
            browser: Some(ody_browser_control::BrowserControlConfig {
                headless: false,
                ..Default::default()
            }),
        });
        BrowserControlExtension.on_config_changed(
            &ExtensionData::new("session"),
            &thread_store,
            &previous_config,
            &new_config,
        );
        let handle = thread_store
            .get::<BrowserControlHandle>()
            .expect("handle still present");
        assert!(handle.state.is_stale(), "headless change requires restart");
    }

    #[tokio::test]
    async fn config_does_not_mark_stale_for_timeout_change() {
        let thread_store = ExtensionData::new("thread");
        thread_store.insert(make_handle(true, true, false));

        let previous_config = config_with_browser(true).await;
        let mut new_config = config_with_browser(true).await;
        new_config.services = Some(ServicesConfig {
            web_search: None,
            browser: Some(ody_browser_control::BrowserControlConfig {
                command_timeout_ms: 5_000,
                ..Default::default()
            }),
        });
        BrowserControlExtension.on_config_changed(
            &ExtensionData::new("session"),
            &thread_store,
            &previous_config,
            &new_config,
        );
        let handle = thread_store
            .get::<BrowserControlHandle>()
            .expect("handle still present");
        assert!(
            !handle.state.is_stale(),
            "timeout-only change should not require restart"
        );
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
            .features
            .set_enabled(Feature::BrowserUseExternal, enabled)
            .expect("toggle BrowserUseExternal");
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
        thread_store.insert(make_handle(true, true, false));
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
        thread_store.insert(make_handle(true, true, true));
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

    #[test]
    fn computer_use_only_exposes_click_type_screenshot() {
        let session_store = ExtensionData::new("session");
        let thread_store = ExtensionData::new("thread");
        thread_store.insert(make_handle(false, true, false));
        let extension = BrowserControlExtension;
        let tools = extension.tools(&session_store, &thread_store);
        let names: Vec<String> = tools
            .iter()
            .map(|t| t.tool_name().name.clone())
            .collect();
        assert!(names.contains(&"click".to_string()));
        assert!(names.contains(&"type".to_string()));
        assert!(names.contains(&"screenshot".to_string()));
        assert!(!names.contains(&"navigate".to_string()));
        assert!(!names.contains(&"evaluate".to_string()));
        assert!(!names.contains(&"execute_raw_cdp".to_string()));
    }

    #[test]
    fn browser_use_only_exposes_browser_subset() {
        let session_store = ExtensionData::new("session");
        let thread_store = ExtensionData::new("thread");
        thread_store.insert(make_handle(true, false, false));
        let extension = BrowserControlExtension;
        let tools = extension.tools(&session_store, &thread_store);
        let names: Vec<String> = tools
            .iter()
            .map(|t| t.tool_name().name.clone())
            .collect();
        assert!(names.contains(&"navigate".to_string()));
        assert!(names.contains(&"evaluate".to_string()));
        assert!(names.contains(&"screenshot".to_string()));
        assert!(!names.contains(&"click".to_string()));
        assert!(!names.contains(&"type_text".to_string()));
    }

    #[test]
    fn no_browser_or_computer_use_returns_empty_tools() {
        let session_store = ExtensionData::new("session");
        let thread_store = ExtensionData::new("thread");
        thread_store.insert(make_handle(false, false, true));
        let extension = BrowserControlExtension;
        let tools = extension.tools(&session_store, &thread_store);
        assert!(tools.is_empty());
    }
}
