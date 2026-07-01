use ody_core::config::Config;
use ody_extension_api::ExtensionRegistryBuilder;

mod executor_plugin;

/// Previously wired a `hosted_plugin_runtime` MCP server contributor that pointed at a
/// Remote-hosted "Apps" plugin runtime endpoint. That remote-hosted-catalog
/// integration has been removed; this function is now a no-op kept so callers do not need
/// to change their extension-registration sequence.
pub fn install(_builder: &mut ExtensionRegistryBuilder<Config>) {}

/// Installs discovery for MCP servers declared by thread-selected executor plugins.
pub fn install_executor_plugins(
    builder: &mut ExtensionRegistryBuilder<Config>,
    environment_manager: std::sync::Arc<ody_exec_server::EnvironmentManager>,
) {
    builder.mcp_server_contributor(std::sync::Arc::new(
        executor_plugin::SelectedExecutorPluginMcpContributor::new(environment_manager),
    ));
}

/// Seeds the per-thread snapshot used by selected executor plugin MCP discovery.
pub fn initialize_executor_plugin_thread_data(
    thread_init: &mut ody_extension_api::ExtensionDataInit,
) {
    executor_plugin::seed_thread_state(thread_init);
}
