mod app_mcp_routing;
mod curated_sync;
mod discoverable;
pub mod installed_marketplaces;
pub mod loader;
mod manager;
pub mod manifest;
pub mod marketplace;
pub mod marketplace_add;
pub mod marketplace_remove;
pub mod marketplace_upgrade;
mod provider;
pub mod store;
#[cfg(test)]
mod test_support;
pub mod toggles;
mod tool_suggest_metadata;

pub const OPENAI_CURATED_MARKETPLACE_NAME: &str = "odysseythink-curated";
pub const OPENAI_API_CURATED_MARKETPLACE_NAME: &str = "odysseythink-api-curated";
pub const OPENAI_BUNDLED_MARKETPLACE_NAME: &str = "odysseythink-bundled";

/// Marketplace name declared by the upstream openai/plugins repository's
/// `.agents/plugins/marketplace.json`. The local sync in `curated_sync` mirrors that repo
/// verbatim, so its marketplace surfaces under this name.
pub const UPSTREAM_CURATED_MARKETPLACE_NAME: &str = "openai-curated";

pub fn is_odysseythink_curated_marketplace_name(marketplace_name: &str) -> bool {
    marketplace_name == OPENAI_CURATED_MARKETPLACE_NAME
        || marketplace_name == OPENAI_API_CURATED_MARKETPLACE_NAME
        || marketplace_name == UPSTREAM_CURATED_MARKETPLACE_NAME
}

#[cfg(test)]
mod tests {
    use super::UPSTREAM_CURATED_MARKETPLACE_NAME;
    use super::is_odysseythink_curated_marketplace_name;

    #[test]
    fn upstream_curated_marketplace_name_is_recognized_as_curated() {
        assert!(is_odysseythink_curated_marketplace_name(
            UPSTREAM_CURATED_MARKETPLACE_NAME
        ));
        assert!(is_odysseythink_curated_marketplace_name("odysseythink-curated"));
        assert!(is_odysseythink_curated_marketplace_name(
            "odysseythink-api-curated"
        ));
        assert!(!is_odysseythink_curated_marketplace_name("some-other-marketplace"));
    }
}

pub type LoadedPlugin = ody_plugin::LoadedPlugin<ody_config::McpServerConfig>;
pub type PluginLoadOutcome = ody_plugin::PluginLoadOutcome<ody_config::McpServerConfig>;

pub use app_mcp_routing::apps_route_available;
pub use discoverable::ToolSuggestDiscoverablePlugin;
pub use discoverable::ToolSuggestPluginDiscoveryInput;
pub use loader::PluginHookLoadOutcome;
pub use manager::ConfiguredMarketplace;
pub use manager::ConfiguredMarketplaceListOutcome;
pub use manager::ConfiguredMarketplacePlugin;
pub use manager::PluginDetail;
pub use manager::PluginDetailsUnavailableReason;
pub use manager::PluginInstallError;
pub use manager::PluginInstallOutcome;
pub use manager::PluginInstallRequest;
pub use manager::PluginListBackgroundTaskOptions;
pub use manager::PluginReadOutcome;
pub use manager::PluginReadRequest;
pub use manager::PluginUninstallError;
pub use manager::PluginsConfigInput;
pub use manager::PluginsManager;
pub use marketplace_upgrade::ConfiguredMarketplaceUpgradeError as PluginMarketplaceUpgradeError;
pub use marketplace_upgrade::ConfiguredMarketplaceUpgradeOutcome as PluginMarketplaceUpgradeOutcome;
pub use provider::ExecutorPluginProvider;
pub use provider::ExecutorPluginProviderError;
pub use provider::ResolvedExecutorPlugin;
