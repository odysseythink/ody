use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex as StdMutex;
use std::time::Duration;
use std::time::Instant;

pub use ody_app_server_protocol::AppBranding;
pub use ody_app_server_protocol::AppInfo;
pub use ody_app_server_protocol::AppMetadata;
use ody_connectors::app_is_enabled;
use ody_connectors::apps_config_from_layer_stack;
use ody_connectors::merge::merge_connectors;
use ody_connectors::merge::merge_plugin_connectors;
use ody_plugin::AppConnectorId;
use ody_exec_server::EnvironmentManager;
use ody_exec_server::ExecServerRuntimePaths;
use ody_tools::DiscoverableTool;
use tracing::instrument;

use crate::config::Config;
use crate::mcp::McpManager;
use crate::plugins::list_tool_suggest_discoverable_plugins;
use ody_config::types::ApprovalsReviewer;
use ody_config::types::ToolSuggestDiscoverableType;
use ody_core_plugins::PluginsManager;
use ody_features::Feature;
use ody_mcp::ODY_APPS_MCP_SERVER_NAME;
use ody_mcp::MCP_TOOL_ODY_APPS_META_KEY;
use ody_mcp::McpConnectionManager;
use ody_mcp::ToolInfo;
use ody_mcp::ToolPluginProvenance;

const CONNECTORS_READY_TIMEOUT_ON_EMPTY_TOOLS: Duration = Duration::from_secs(30);

#[derive(Clone, PartialEq, Eq)]
struct AccessibleConnectorsCacheKey {
    account_id: Option<String>,
    is_workspace_plan: bool,
}

#[derive(Clone)]
struct CachedAccessibleConnectors {
    key: AccessibleConnectorsCacheKey,
    expires_at: Instant,
    connectors: Vec<AppInfo>,
}

static ACCESSIBLE_CONNECTORS_CACHE: LazyLock<StdMutex<Option<CachedAccessibleConnectors>>> =
    LazyLock::new(|| StdMutex::new(None));

#[derive(Debug, Clone)]
pub struct AccessibleConnectorsStatus {
    pub connectors: Vec<AppInfo>,
    pub ody_apps_ready: bool,
}

pub async fn list_accessible_connectors_from_mcp_tools(
    config: &Config,
) -> anyhow::Result<Vec<AppInfo>> {
    Ok(
        list_accessible_connectors_from_mcp_tools_with_options_and_status(
            config, /*force_refetch*/ false,
        )
        .await?
        .connectors,
    )
}

pub(crate) async fn list_accessible_and_enabled_connectors_from_manager(
    mcp_connection_manager: &McpConnectionManager,
    config: &Config,
) -> Vec<AppInfo> {
    with_app_enabled_state(
        accessible_connectors_from_mcp_tools(&mcp_connection_manager.list_all_tools().await),
        config,
    )
    .into_iter()
    .filter(|connector| connector.is_accessible && connector.is_enabled)
    .collect()
}

#[instrument(level = "trace", skip_all)]
pub(crate) async fn list_tool_suggest_discoverable_tools(
    config: &Config,
    plugins_manager: &PluginsManager,
    accessible_connectors: &[AppInfo],
    loaded_plugin_app_connector_ids: &[String],
) -> anyhow::Result<Vec<DiscoverableTool>> {
    let connector_ids = tool_suggest_connector_ids(config, loaded_plugin_app_connector_ids);
    let directory_connectors = ody_connectors::merge::merge_plugin_connectors(
        cached_directory_connectors_for_tool_suggest(config).await,
        connector_ids.iter().cloned(),
    );
    let discoverable_connectors =
        ody_connectors::filter::filter_tool_suggest_discoverable_connectors(
            directory_connectors,
            accessible_connectors,
            &connector_ids,
        )
        .into_iter()
        .map(DiscoverableTool::from);
    let discoverable_plugins = list_tool_suggest_discoverable_plugins(
        config,
        plugins_manager,
        loaded_plugin_app_connector_ids,
    )
    .await?
    .into_iter()
    .map(DiscoverableTool::from);
    Ok(discoverable_connectors
        .chain(discoverable_plugins)
        .collect())
}

pub async fn list_cached_accessible_connectors_from_mcp_tools(
    _config: &Config,
) -> Option<Vec<AppInfo>> {
    // Remote directory connectors are no longer available.
    Some(Vec::new())
}

pub(crate) fn refresh_accessible_connectors_cache_from_mcp_tools(
    config: &Config,
    mcp_tools: &[ToolInfo],
) {
    if !config.features.enabled(Feature::Apps) {
        return;
    }

    let cache_key = accessible_connectors_cache_key(config);
    let accessible_connectors = accessible_connectors_for_app_list_from_mcp_tools(mcp_tools);
    write_cached_accessible_connectors(cache_key, &accessible_connectors);
}

pub async fn list_accessible_connectors_from_mcp_tools_with_options(
    config: &Config,
    force_refetch: bool,
) -> anyhow::Result<Vec<AppInfo>> {
    Ok(
        list_accessible_connectors_from_mcp_tools_with_options_and_status(config, force_refetch)
            .await?
            .connectors,
    )
}

pub async fn list_accessible_connectors_from_mcp_tools_with_options_and_status(
    config: &Config,
    force_refetch: bool,
) -> anyhow::Result<AccessibleConnectorsStatus> {
    // TODO: Wire callers that already own an EnvironmentManager into
    // list_accessible_connectors_from_mcp_tools_with_environment_manager instead
    // of constructing a temporary manager here.
    let local_runtime_paths = ExecServerRuntimePaths::from_optional_paths(
        config.ody_self_exe.clone(),
        None,
    )?;
    let environment_manager =
        EnvironmentManager::from_ody_home(config.ody_home.clone(), Some(local_runtime_paths))
            .await?;
    list_accessible_connectors_from_mcp_tools_with_environment_manager(
        config,
        force_refetch,
        Arc::new(environment_manager),
    )
    .await
}

pub async fn list_accessible_connectors_from_mcp_tools_with_environment_manager(
    config: &Config,
    force_refetch: bool,
    environment_manager: Arc<EnvironmentManager>,
) -> anyhow::Result<AccessibleConnectorsStatus> {
    let plugins_manager = Arc::new(PluginsManager::new(config.ody_home.to_path_buf()));
    let mcp_manager = Arc::new(McpManager::new(plugins_manager));
    list_accessible_connectors_from_mcp_tools_with_mcp_manager(
        config,
        force_refetch,
        environment_manager,
        mcp_manager,
    )
    .await
}

pub async fn list_accessible_connectors_from_mcp_tools_with_mcp_manager(
    config: &Config,
    _force_refetch: bool,
    environment_manager: Arc<EnvironmentManager>,
    mcp_manager: Arc<McpManager>,
) -> anyhow::Result<AccessibleConnectorsStatus> {
    // Remote directory connectors are no longer available.
    let _ = (config, environment_manager, mcp_manager);
    Ok(AccessibleConnectorsStatus {
        connectors: Vec::new(),
        ody_apps_ready: true,
    })
}

fn accessible_connectors_cache_key(
    _config: &Config,
) -> AccessibleConnectorsCacheKey {
    // Account-specific connector metadata is no longer available.
    AccessibleConnectorsCacheKey {
        account_id: None,
        is_workspace_plan: false,
    }
}

fn read_cached_accessible_connectors(
    cache_key: &AccessibleConnectorsCacheKey,
) -> Option<Vec<AppInfo>> {
    let mut cache_guard = ACCESSIBLE_CONNECTORS_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let now = Instant::now();

    if let Some(cached) = cache_guard.as_ref() {
        if now < cached.expires_at && cached.key == *cache_key {
            return Some(cached.connectors.clone());
        }
        if now >= cached.expires_at {
            *cache_guard = None;
        }
    }

    None
}

fn write_cached_accessible_connectors(
    cache_key: AccessibleConnectorsCacheKey,
    connectors: &[AppInfo],
) {
    let mut cache_guard = ACCESSIBLE_CONNECTORS_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *cache_guard = Some(CachedAccessibleConnectors {
        key: cache_key,
        expires_at: Instant::now() + ody_connectors::CONNECTORS_CACHE_TTL,
        connectors: connectors.to_vec(),
    });
}

fn tool_suggest_connector_ids(
    config: &Config,
    loaded_plugin_app_connector_ids: &[String],
) -> HashSet<String> {
    let mut connector_ids = loaded_plugin_app_connector_ids
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    connector_ids.extend(
        config
            .tool_suggest
            .discoverables
            .iter()
            .filter(|discoverable| discoverable.kind == ToolSuggestDiscoverableType::Connector)
            .map(|discoverable| discoverable.id.clone()),
    );
    let disabled_connector_ids = config
        .tool_suggest
        .disabled_tools
        .iter()
        .filter(|disabled_tool| disabled_tool.kind == ToolSuggestDiscoverableType::Connector)
        .map(|disabled_tool| disabled_tool.id.as_str())
        .collect::<HashSet<_>>();
    connector_ids.retain(|connector_id| !disabled_connector_ids.contains(connector_id.as_str()));
    connector_ids
}

#[instrument(level = "trace", skip_all)]
async fn cached_directory_connectors_for_tool_suggest(
    _config: &Config,
) -> Vec<AppInfo> {
    // Remote directory connectors are no longer available.
    Vec::new()
}

pub(crate) fn accessible_connectors_from_mcp_tools(mcp_tools: &[ToolInfo]) -> Vec<AppInfo> {
    collect_accessible_connectors_from_mcp_tools(mcp_tools.iter())
}

fn collect_accessible_connectors_from_mcp_tools<'a>(
    mcp_tools: impl Iterator<Item = &'a ToolInfo>,
) -> Vec<AppInfo> {
    // ToolInfo already carries plugin provenance, so app-level plugin sources
    // can be derived here instead of requiring a separate enrichment pass.
    let tools = mcp_tools.filter_map(|tool| {
        if tool.server_name != ODY_APPS_MCP_SERVER_NAME {
            return None;
        }
        let connector_id = tool.connector_id.as_deref()?;
        Some(ody_connectors::accessible::AccessibleConnectorTool {
            connector_id: connector_id.to_string(),
            connector_name: tool.connector_name.clone(),
            connector_description: tool.namespace_description.clone(),
            plugin_display_names: tool.plugin_display_names.clone(),
        })
    });
    ody_connectors::accessible::collect_accessible_connectors(tools)
}

fn accessible_connectors_for_app_list_from_mcp_tools(mcp_tools: &[ToolInfo]) -> Vec<AppInfo> {
    let non_synthetic_tools = mcp_tools.iter().filter(|tool| {
        tool.tool
            .meta
            .as_deref()
            .and_then(|meta| meta.get(MCP_TOOL_ODY_APPS_META_KEY))
            .and_then(serde_json::Value::as_object)
            .and_then(|meta| meta.get("synthetic_link"))
            .and_then(serde_json::Value::as_bool)
            != Some(true)
    });
    collect_accessible_connectors_from_mcp_tools(non_synthetic_tools)
}

pub fn with_app_enabled_state(mut connectors: Vec<AppInfo>, config: &Config) -> Vec<AppInfo> {
    let user_apps_config = apps_config_from_layer_stack(&config.config_layer_stack);
    let requirements_apps_config = config.config_layer_stack.requirements_toml().apps.as_ref();
    if user_apps_config.is_none() && requirements_apps_config.is_none() {
        return connectors;
    }

    for connector in &mut connectors {
        if let Some(apps_config) = user_apps_config.as_ref()
            && (apps_config.default.is_some()
                || apps_config.apps.contains_key(connector.id.as_str()))
        {
            connector.is_enabled = app_is_enabled(apps_config, Some(connector.id.as_str()));
        }

        if requirements_apps_config
            .and_then(|apps| apps.apps.get(connector.id.as_str()))
            .is_some_and(|app| app.enabled == Some(false))
        {
            connector.is_enabled = false;
        }
    }

    connectors
}

pub fn with_app_plugin_sources(
    mut connectors: Vec<AppInfo>,
    tool_plugin_provenance: &ToolPluginProvenance,
) -> Vec<AppInfo> {
    for connector in &mut connectors {
        connector.plugin_display_names = tool_plugin_provenance
            .plugin_display_names_for_connector_id(connector.id.as_str())
            .to_vec();
    }
    connectors
}

pub(crate) fn mcp_approvals_reviewer(
    config: &Config,
    server_name: &str,
    connector_id: Option<&str>,
) -> ApprovalsReviewer {
    let app_reviewer = if server_name == ODY_APPS_MCP_SERVER_NAME {
        apps_config_from_layer_stack(&config.config_layer_stack).and_then(|apps_config| {
            connector_id
                .and_then(|connector_id| apps_config.apps.get(connector_id))
                .and_then(|app| app.approvals_reviewer)
                .or_else(|| {
                    apps_config
                        .default
                        .and_then(|defaults| defaults.approvals_reviewer)
                })
        })
    } else {
        None
    };

    if let Some(reviewer) = app_reviewer
        && config
            .config_layer_stack
            .requirements()
            .approvals_reviewer
            .can_set(&reviewer)
            .is_ok()
    {
        return reviewer;
    }

    config.approvals_reviewer
}

/// The remote connector directory is no longer available.
pub async fn list_all_connectors_with_options(
    _config: &Config,
    _force_refetch: bool,
    _plugin_apps: &[AppConnectorId],
) -> anyhow::Result<Vec<AppInfo>> {
    Ok(Vec::new())
}

/// The remote connector directory is no longer available.
pub async fn list_cached_all_connectors(
    _config: &Config,
    _plugin_apps: &[AppConnectorId],
) -> Option<Vec<AppInfo>> {
    Some(Vec::new())
}

/// Merge a full connector directory listing with the set of connectors that are
/// accessible via MCP tools. When the full listing is known to be complete,
/// inaccessible connectors not present in the full listing are dropped.
pub fn merge_connectors_with_accessible(
    connectors: Vec<AppInfo>,
    accessible_connectors: Vec<AppInfo>,
    all_connectors_loaded: bool,
) -> Vec<AppInfo> {
    let accessible_connectors = if all_connectors_loaded {
        let connector_ids: HashSet<&str> = connectors
            .iter()
            .map(|connector| connector.id.as_str())
            .collect();
        accessible_connectors
            .into_iter()
            .filter(|connector| connector_ids.contains(connector.id.as_str()))
            .collect()
    } else {
        accessible_connectors
    };
    merge_connectors(connectors, accessible_connectors)
}

/// Filters a connector listing down to only the connectors referenced by a plugin's
/// declared apps, synthesizing placeholder entries for apps that have no directory
/// metadata.
pub fn connectors_for_plugin_apps(
    connectors: Vec<AppInfo>,
    plugin_apps: &[AppConnectorId],
) -> Vec<AppInfo> {
    let connectors = merge_plugin_connectors(
        connectors,
        plugin_apps
            .iter()
            .map(|connector_id| connector_id.0.clone()),
    );
    let mut connectors_by_id = connectors
        .into_iter()
        .map(|connector| (connector.id.clone(), connector))
        .collect::<HashMap<_, _>>();

    plugin_apps
        .iter()
        .filter_map(|connector_id| connectors_by_id.remove(connector_id.0.as_str()))
        .collect()
}

#[cfg(test)]
#[path = "connectors_tests.rs"]
mod tests;
