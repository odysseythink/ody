use super::LoadedPlugin;
use super::PluginLoadOutcome;
use crate::app_mcp_routing::apply_app_mcp_routing_policy;
use crate::installed_marketplaces::installed_marketplace_roots_from_layer_stack;
use crate::loader::PluginHookLoadOutcome;
use crate::loader::curated_plugins_api_marketplace_path;
use crate::loader::curated_plugins_repo_path;
use crate::loader::load_plugin_apps_from_manifest;
use crate::loader::load_plugin_hooks;
use crate::loader::load_plugin_hooks_from_layer_stack;
use crate::loader::load_plugin_mcp_servers_from_manifest;
use crate::loader::load_plugin_skills;
use crate::loader::load_plugins_from_layer_stack;
use crate::loader::log_plugin_load_errors;
use crate::loader::materialize_marketplace_plugin_source;
use crate::loader::plugin_capability_summary_from_root;
use crate::loader::refresh_non_curated_plugin_cache;
use crate::loader::refresh_non_curated_plugin_cache_force_reinstall;
use crate::manifest::PluginManifestInterface;
use crate::manifest::load_plugin_manifest;
use crate::marketplace::MarketplaceError;
use crate::marketplace::MarketplaceInterface;
use crate::marketplace::MarketplaceListError;
use crate::marketplace::MarketplaceListOutcome;
use crate::marketplace::MarketplacePluginAuthPolicy;
use crate::marketplace::MarketplacePluginManifestFallback;
use crate::marketplace::MarketplacePluginPolicy;
use crate::marketplace::MarketplacePluginSource;
use crate::marketplace::ResolvedMarketplacePlugin;
use crate::marketplace::find_installable_marketplace_plugin;
use crate::marketplace::find_marketplace_plugin;
use crate::marketplace::list_marketplaces;
use crate::marketplace::plugin_interface_with_marketplace_category;
use crate::marketplace_upgrade::ConfiguredMarketplaceUpgradeError;
use crate::marketplace_upgrade::ConfiguredMarketplaceUpgradeOutcome;
use crate::marketplace_upgrade::configured_git_marketplace_names;
use crate::marketplace_upgrade::upgrade_configured_git_marketplaces;
use crate::store::PluginInstallResult as StorePluginInstallResult;
use crate::store::PluginStore;
use crate::store::PluginStoreError;
use crate::tool_suggest_metadata::ToolSuggestMetadataCache;
use ody_analytics::AnalyticsEventsClient;
use ody_app_server_protocol::AuthMode;
use ody_config::ConfigLayerStack;
use ody_config::clear_user_plugin;
use ody_config::set_user_plugin_enabled;
use ody_config::types::PluginConfig;
use ody_core_skills::PluginSkillSnapshots;
use ody_core_skills::SkillMetadata;
use ody_core_skills::config_rules::SkillConfigRules;
use ody_core_skills::config_rules::skill_config_rules_from_stack;
use ody_hooks::plugin_hook_declarations;
use ody_login::AuthManager;
use ody_plugin::AppConnectorId;
use ody_plugin::PluginCapabilitySummary;
use ody_plugin::PluginId;
use ody_plugin::PluginIdError;
use ody_plugin::PluginTelemetryMetadata;
use ody_plugin::app_connector_ids_from_declarations;
use ody_plugin::prompt_safe_plugin_description;
use ody_protocol::protocol::HookEventName;
use ody_protocol::protocol::Product;
use ody_utils_absolute_path::AbsolutePathBuf;
use ody_utils_plugins::PluginSkillRoot;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::RwLock;
use tokio::sync::Semaphore;
use tracing::instrument;
use tracing::warn;

#[derive(Debug, Clone)]
pub struct PluginsConfigInput {
    pub config_layer_stack: ConfigLayerStack,
    pub plugins_enabled: bool,
}

impl PluginsConfigInput {
    pub fn new(config_layer_stack: ConfigLayerStack, plugins_enabled: bool) -> Self {
        Self {
            config_layer_stack,
            plugins_enabled,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PluginListBackgroundTaskOptions {}

#[derive(Clone, PartialEq, Eq)]
struct NonCuratedCacheRefreshRequest {
    roots: Vec<AbsolutePathBuf>,
    mode: NonCuratedCacheRefreshMode,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NonCuratedCacheRefreshMode {
    IfVersionChanged,
    ForceReinstall,
}

#[derive(Default)]
struct NonCuratedCacheRefreshState {
    requested: Option<NonCuratedCacheRefreshRequest>,
    last_refreshed: Option<NonCuratedCacheRefreshRequest>,
    in_flight: bool,
}

#[derive(Default)]
struct ConfiguredMarketplaceUpgradeState {
    in_flight: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginInstallRequest {
    pub plugin_name: String,
    pub marketplace_path: AbsolutePathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginReadRequest {
    pub plugin_name: String,
    pub marketplace_path: AbsolutePathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginInstallOutcome {
    pub plugin_id: PluginId,
    pub plugin_version: String,
    pub installed_path: AbsolutePathBuf,
    pub auth_policy: MarketplacePluginAuthPolicy,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PluginReadOutcome {
    pub marketplace_name: String,
    pub marketplace_path: Option<AbsolutePathBuf>,
    pub plugin: PluginDetail,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PluginDetail {
    pub id: String,
    pub name: String,
    pub local_version: Option<String>,
    pub description: Option<String>,
    pub source: MarketplacePluginSource,
    pub policy: MarketplacePluginPolicy,
    pub interface: Option<PluginManifestInterface>,
    pub keywords: Vec<String>,
    pub installed: bool,
    pub enabled: bool,
    pub skills: Vec<SkillMetadata>,
    pub disabled_skill_paths: HashSet<AbsolutePathBuf>,
    pub hooks: Vec<PluginHookSummary>,
    pub apps: Vec<AppConnectorId>,
    pub app_category_by_id: HashMap<String, String>,
    pub mcp_server_names: Vec<String>,
    pub details_unavailable_reason: Option<PluginDetailsUnavailableReason>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PluginHookSummary {
    pub key: String,
    pub event_name: HookEventName,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginDetailsUnavailableReason {
    InstallRequiredForRemoteSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfiguredMarketplace {
    pub name: String,
    pub path: AbsolutePathBuf,
    pub interface: Option<MarketplaceInterface>,
    pub plugins: Vec<ConfiguredMarketplacePlugin>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfiguredMarketplacePlugin {
    pub id: String,
    pub name: String,
    pub local_version: Option<String>,
    pub installed_version: Option<String>,
    pub source: MarketplacePluginSource,
    pub policy: MarketplacePluginPolicy,
    pub interface: Option<PluginManifestInterface>,
    pub keywords: Vec<String>,
    pub manifest_fallback: Option<MarketplacePluginManifestFallback>,
    pub installed: bool,
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfiguredMarketplaceListOutcome {
    pub marketplaces: Vec<ConfiguredMarketplace>,
    pub errors: Vec<MarketplaceListError>,
}

impl From<PluginDetail> for PluginCapabilitySummary {
    fn from(value: PluginDetail) -> Self {
        let has_skills = value.skills.iter().any(|skill| {
            !value
                .disabled_skill_paths
                .contains(&skill.path_to_skills_md)
        });
        Self {
            config_name: value.id,
            display_name: value.name,
            description: prompt_safe_plugin_description(value.description.as_deref()),
            has_skills,
            mcp_server_names: value.mcp_server_names,
            app_connector_ids: value.apps,
        }
    }
}

pub struct PluginsManager {
    ody_home: PathBuf,
    store: PluginStore,
    configured_marketplace_upgrade_state: RwLock<ConfiguredMarketplaceUpgradeState>,
    non_curated_cache_refresh_state: RwLock<NonCuratedCacheRefreshState>,
    // Keep the cache auth-independent so auth changes only need to resolve capabilities again.
    loaded_plugins_cache: RwLock<LoadedPluginsCache>,
    loaded_plugins_load_semaphore: Semaphore,
    tool_suggest_metadata_cache: ToolSuggestMetadataCache,
    restriction_product: Option<Product>,
    auth_mode: RwLock<Option<AuthMode>>,
    analytics_events_client: RwLock<Option<AnalyticsEventsClient>>,
}

#[derive(Clone)]
struct LoadedPluginsCacheEntry {
    key: PluginLoadCacheKey,
    plugins: Vec<LoadedPlugin>,
    plugin_skill_snapshots: PluginSkillSnapshots,
}

#[derive(Default)]
struct LoadedPluginsCache {
    generation: u64,
    entry: Option<LoadedPluginsCacheEntry>,
}

#[derive(Clone, PartialEq, Eq)]
struct PluginLoadCacheKey {
    configured_plugins: HashMap<String, PluginConfig>,
    skill_config_rules: SkillConfigRules,
}

impl PluginLoadCacheKey {
    fn from_config(config: &PluginsConfigInput) -> Self {
        Self {
            configured_plugins: configured_plugins_from_stack(&config.config_layer_stack),
            skill_config_rules: skill_config_rules_from_stack(&config.config_layer_stack),
        }
    }
}

impl PluginsManager {
    pub fn new(ody_home: PathBuf) -> Self {
        Self::new_with_options(ody_home, Some(Product::Ody), /*auth_mode*/ None)
    }

    pub fn new_with_options(
        ody_home: PathBuf,
        restriction_product: Option<Product>,
        auth_mode: Option<AuthMode>,
    ) -> Self {
        // Product restrictions are enforced at marketplace admission time for a given ODY_HOME:
        // listing, install, and curated refresh all consult this restriction context before new
        // plugins enter local config or cache. After admission, runtime plugin loading trusts the
        // contents of that ODY_HOME and does not re-filter configured plugins by product, so
        // already-admitted plugins may continue exposing MCP servers/tools from shared local state.
        //
        // This assumes a single ODY_HOME is only used by one product.
        Self {
            ody_home: ody_home.clone(),
            store: PluginStore::new(ody_home),
            configured_marketplace_upgrade_state: RwLock::new(
                ConfiguredMarketplaceUpgradeState::default(),
            ),
            non_curated_cache_refresh_state: RwLock::new(NonCuratedCacheRefreshState::default()),
            loaded_plugins_cache: RwLock::new(LoadedPluginsCache::default()),
            loaded_plugins_load_semaphore: Semaphore::new(/*permits*/ 1),
            tool_suggest_metadata_cache: ToolSuggestMetadataCache::new(),
            restriction_product,
            auth_mode: RwLock::new(auth_mode),
            analytics_events_client: RwLock::new(None),
        }
    }

    pub fn set_auth_mode(&self, auth_mode: Option<AuthMode>) -> bool {
        let mut stored_auth_mode = match self.auth_mode.write() {
            Ok(auth_mode_guard) => auth_mode_guard,
            Err(err) => err.into_inner(),
        };
        if *stored_auth_mode == auth_mode {
            return false;
        }
        *stored_auth_mode = auth_mode;
        true
    }

    pub fn auth_mode(&self) -> Option<AuthMode> {
        match self.auth_mode.read() {
            Ok(auth_mode_guard) => *auth_mode_guard,
            Err(err) => *err.into_inner(),
        }
    }

    pub fn set_analytics_events_client(&self, analytics_events_client: AnalyticsEventsClient) {
        let mut stored_client = match self.analytics_events_client.write() {
            Ok(client_guard) => client_guard,
            Err(err) => err.into_inner(),
        };
        *stored_client = Some(analytics_events_client);
    }

    fn restriction_product_matches(&self, products: Option<&[Product]>) -> bool {
        match products {
            None => true,
            Some([]) => false,
            Some(products) => self
                .restriction_product
                .is_some_and(|product| product.matches_product_restriction(products)),
        }
    }

    pub async fn plugins_for_config(&self, config: &PluginsConfigInput) -> PluginLoadOutcome {
        self.plugins_for_config_with_force_reload(config, /*force_reload*/ false)
            .await
    }

    /// Returns skill snapshots parsed while loading the matching plugin cache entry.
    pub fn plugin_skill_snapshots_for_config(
        &self,
        config: &PluginsConfigInput,
    ) -> Option<PluginSkillSnapshots> {
        if !config.plugins_enabled {
            return None;
        }
        let key = PluginLoadCacheKey::from_config(config);
        self.loaded_plugins_cache
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry
            .as_ref()
            .filter(|cached| cached.key == key)
            .map(|cached| cached.plugin_skill_snapshots.clone())
    }

    #[instrument(
        name = "plugins_for_config",
        level = "info",
        skip_all,
        fields(
            otel.name = "plugins_for_config",
            force_reload,
            plugins_enabled = config.plugins_enabled
        )
    )]
    pub(crate) async fn plugins_for_config_with_force_reload(
        &self,
        config: &PluginsConfigInput,
        force_reload: bool,
    ) -> PluginLoadOutcome {
        if !config.plugins_enabled {
            return PluginLoadOutcome::default();
        }

        let cache_key = PluginLoadCacheKey::from_config(config);
        if !force_reload && let Some(plugins) = self.cached_loaded_plugins(&cache_key) {
            return self.resolve_loaded_plugins_for_auth(plugins);
        }

        let Ok(_load_permit) = self.loaded_plugins_load_semaphore.acquire().await else {
            warn!("plugin load semaphore closed");
            return PluginLoadOutcome::default();
        };
        if !force_reload && let Some(plugins) = self.cached_loaded_plugins(&cache_key) {
            return self.resolve_loaded_plugins_for_auth(plugins);
        }
        let cache_generation = self.loaded_plugins_cache_generation();
        let plugin_skill_snapshots = PluginSkillSnapshots::for_plugin_load();
        let plugins = load_plugins_from_layer_stack(
            &config.config_layer_stack,
            HashMap::new(),
            &self.store,
            Some(&plugin_skill_snapshots),
            self.restriction_product,
            /* prefer_remote_curated_conflicts */ false,
        )
        .await;
        log_plugin_load_errors(&plugins);
        self.cache_loaded_plugins_if_current(
            cache_generation,
            cache_key,
            plugins.clone(),
            plugin_skill_snapshots,
        );
        self.resolve_loaded_plugins_for_auth(plugins)
    }

    fn resolve_loaded_plugins_for_auth(&self, mut plugins: Vec<LoadedPlugin>) -> PluginLoadOutcome {
        let auth_mode = self.auth_mode();
        for plugin in &mut plugins {
            let plugin_active = plugin.is_active();
            apply_app_mcp_routing_policy(
                &mut plugin.apps,
                &mut plugin.mcp_servers,
                auth_mode,
                plugin_active,
            );
        }
        PluginLoadOutcome::from_plugins(plugins)
    }

    pub fn clear_cache(&self) {
        self.clear_loaded_plugins_cache();
    }

    fn clear_loaded_plugins_cache(&self) {
        self.tool_suggest_metadata_cache.clear();
        let mut cache = match self.loaded_plugins_cache.write() {
            Ok(cache) => cache,
            Err(err) => err.into_inner(),
        };
        cache.generation = cache.generation.wrapping_add(1);
        cache.entry = None;
    }

    fn clear_caches_after_marketplace_source_refresh(
        &self,
        installed_plugin_cache_refreshed: bool,
    ) {
        if installed_plugin_cache_refreshed {
            self.clear_cache();
        } else {
            self.tool_suggest_metadata_cache.clear();
        }
    }

    /// Load plugins for a config layer stack without touching the plugins cache.
    pub async fn plugins_for_layer_stack(
        &self,
        config_layer_stack: &ConfigLayerStack,
        config: &PluginsConfigInput,
    ) -> PluginLoadOutcome {
        if !config.plugins_enabled {
            return PluginLoadOutcome::default();
        }
        let plugins = load_plugins_from_layer_stack(
            config_layer_stack,
            HashMap::new(),
            &self.store,
            /*plugin_skill_snapshots*/ None,
            self.restriction_product,
            /* prefer_remote_curated_conflicts */ false,
        )
        .await;
        self.resolve_loaded_plugins_for_auth(plugins)
    }

    /// Resolve plugin hooks for a config layer stack without loading other plugin capabilities.
    pub async fn plugin_hooks_for_layer_stack(
        &self,
        config_layer_stack: &ConfigLayerStack,
        config: &PluginsConfigInput,
    ) -> PluginHookLoadOutcome {
        if !config.plugins_enabled {
            return PluginHookLoadOutcome::default();
        }
        load_plugin_hooks_from_layer_stack(
            config_layer_stack,
            HashMap::new(),
            &self.store,
            /* prefer_remote_curated_conflicts */ false,
        )
        .await
    }

    /// Resolve plugin skill roots for a config layer stack without touching the plugins cache.
    pub async fn effective_skill_roots_for_layer_stack(
        &self,
        config_layer_stack: &ConfigLayerStack,
        config: &PluginsConfigInput,
    ) -> Vec<PluginSkillRoot> {
        self.plugins_for_layer_stack(config_layer_stack, config)
            .await
            .effective_plugin_skill_roots()
    }

    fn cached_loaded_plugins(&self, key: &PluginLoadCacheKey) -> Option<Vec<LoadedPlugin>> {
        self.loaded_plugins_cache
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry
            .as_ref()
            .filter(|cached| cached.key == *key)
            .map(|cached| cached.plugins.clone())
    }

    fn loaded_plugins_cache_generation(&self) -> u64 {
        match self.loaded_plugins_cache.read() {
            Ok(cache) => cache.generation,
            Err(err) => err.into_inner().generation,
        }
    }

    fn cache_loaded_plugins_if_current(
        &self,
        generation: u64,
        key: PluginLoadCacheKey,
        plugins: Vec<LoadedPlugin>,
        plugin_skill_snapshots: PluginSkillSnapshots,
    ) {
        let mut cache = match self.loaded_plugins_cache.write() {
            Ok(cache) => cache,
            Err(err) => err.into_inner(),
        };
        if cache.generation == generation {
            cache.entry = Some(LoadedPluginsCacheEntry {
                key,
                plugins,
                plugin_skill_snapshots,
            });
        }
    }

    pub async fn telemetry_metadata_for_installed_plugin(
        &self,
        plugin_id: &PluginId,
    ) -> PluginTelemetryMetadata {
        let mut metadata = self.telemetry_metadata_for_plugin_id(plugin_id);
        metadata.capability_summary = match self.store.active_plugin_root(plugin_id) {
            Some(plugin_root) => plugin_capability_summary_from_root(plugin_id, &plugin_root).await,
            None => None,
        };
        metadata
    }

    pub async fn telemetry_metadata_for_installed_plugin_with_remote_id(
        &self,
        plugin_id: &PluginId,
        remote_plugin_id: &str,
    ) -> PluginTelemetryMetadata {
        let mut metadata =
            self.telemetry_metadata_for_plugin_id_with_remote_id(plugin_id, remote_plugin_id);
        metadata.capability_summary = match self.store.active_plugin_root(plugin_id) {
            Some(plugin_root) => plugin_capability_summary_from_root(plugin_id, &plugin_root).await,
            None => None,
        };
        metadata
    }

    pub fn telemetry_metadata_for_plugin_id(
        &self,
        plugin_id: &PluginId,
    ) -> PluginTelemetryMetadata {
        PluginTelemetryMetadata {
            plugin_id: plugin_id.clone(),
            remote_plugin_id: None,
            capability_summary: None,
        }
    }

    pub fn telemetry_metadata_for_plugin_id_with_remote_id(
        &self,
        plugin_id: &PluginId,
        remote_plugin_id: &str,
    ) -> PluginTelemetryMetadata {
        PluginTelemetryMetadata {
            remote_plugin_id: Some(remote_plugin_id.to_string()),
            ..self.telemetry_metadata_for_plugin_id(plugin_id)
        }
    }

    pub fn telemetry_metadata_for_capability_summary(
        &self,
        summary: &PluginCapabilitySummary,
    ) -> Option<PluginTelemetryMetadata> {
        let plugin_id = PluginId::parse(&summary.config_name).ok()?;
        Some(PluginTelemetryMetadata {
            plugin_id,
            remote_plugin_id: None,
            capability_summary: Some(summary.clone()),
        })
    }

    pub fn maybe_start_plugin_list_background_tasks_for_config(
        self: &Arc<Self>,
        _config: &PluginsConfigInput,
        roots: &[AbsolutePathBuf],
        _options: PluginListBackgroundTaskOptions,
    ) {
        self.maybe_start_non_curated_plugin_cache_refresh(roots);
    }

    pub async fn install_plugin(
        &self,
        request: PluginInstallRequest,
    ) -> Result<PluginInstallOutcome, PluginInstallError> {
        let resolved = match find_installable_marketplace_plugin(
            &request.marketplace_path,
            &request.plugin_name,
            self.restriction_product,
        ) {
            Ok(resolved) => resolved,
            Err(err) => {
                self.track_plugin_install_resolution_failed(&err);
                return Err(err.into());
            }
        };
        let plugin_id = resolved.plugin_id.clone();
        match self.install_resolved_plugin(resolved).await {
            Ok(outcome) => Ok(outcome),
            Err(err) => {
                self.track_plugin_install_failed(
                    &plugin_id,
                    plugin_install_error_type(&err),
                    err.to_string(),
                );
                Err(err)
            }
        }
    }

    fn track_plugin_install_resolution_failed(&self, err: &MarketplaceError) {
        let plugin_id = match err {
            MarketplaceError::PluginNotFound {
                plugin_name,
                marketplace_name,
            }
            | MarketplaceError::PluginNotAvailable {
                plugin_name,
                marketplace_name,
            } => PluginId::new(plugin_name.clone(), marketplace_name.clone()).ok(),
            MarketplaceError::Io { .. }
            | MarketplaceError::MarketplaceNotFound { .. }
            | MarketplaceError::InvalidMarketplaceFile { .. }
            | MarketplaceError::PluginsDisabled
            | MarketplaceError::InvalidPlugin(_) => None,
        };
        if let Some(plugin_id) = plugin_id {
            self.track_plugin_install_failed(
                &plugin_id,
                marketplace_error_type(err),
                err.to_string(),
            );
        } else {
            tracing::warn!(
                error_type = %marketplace_error_type(err),
                error = %err,
                "plugin install failed while resolving marketplace plugin"
            );
        }
    }

    fn track_plugin_install_failed(
        &self,
        plugin_id: &PluginId,
        error_type: &'static str,
        error_message: String,
    ) {
        tracing::warn!(
            plugin_id = %plugin_id.as_key(),
            error_type = %error_type,
            error = %error_message,
            "plugin install failed"
        );
        let analytics_events_client = match self.analytics_events_client.read() {
            Ok(client) => client.clone(),
            Err(err) => err.into_inner().clone(),
        };
        if let Some(analytics_events_client) = analytics_events_client {
            analytics_events_client.track_plugin_install_failed(
                self.telemetry_metadata_for_plugin_id(plugin_id),
                error_type.to_string(),
            );
        }
    }

    async fn install_resolved_plugin(
        &self,
        resolved: ResolvedMarketplacePlugin,
    ) -> Result<PluginInstallOutcome, PluginInstallError> {
        let auth_policy = resolved.policy.authentication;
        // The curated marketplace was previously synced from a remote (chatgpt.com-hosted)
        // catalog and pinned installs to that sync's sha. Remote catalog syncing has been
        // removed, so curated-marketplace installs now behave like any other marketplace
        // install (no pinned version).
        let plugin_version = None;
        let store = self.store.clone();
        let ody_home = self.ody_home.clone();
        let manifest_fallback_contents = resolved
            .manifest_fallback
            .contents_if_has_metadata()
            .map(str::to_string);
        let result: StorePluginInstallResult = tokio::task::spawn_blocking(move || {
            let materialized =
                materialize_marketplace_plugin_source(ody_home.as_path(), &resolved.source)
                    .map_err(PluginStoreError::Invalid)?;
            let source_path = materialized.path;
            match (plugin_version, manifest_fallback_contents.as_deref()) {
                (Some(plugin_version), Some(manifest_contents)) => store
                    .install_with_version_and_fallback_manifest(
                        source_path,
                        resolved.plugin_id,
                        plugin_version,
                        manifest_contents,
                    ),
                (Some(plugin_version), None) => {
                    store.install_with_version(source_path, resolved.plugin_id, plugin_version)
                }
                (None, Some(manifest_contents)) => store.install_with_fallback_manifest(
                    source_path,
                    resolved.plugin_id,
                    manifest_contents,
                ),
                (None, None) => store.install(source_path, resolved.plugin_id),
            }
        })
        .await
        .map_err(PluginInstallError::join)??;

        set_user_plugin_enabled(
            &self.ody_home,
            result.plugin_id.as_key(),
            /*enabled*/ true,
        )
        .await
        .map_err(anyhow::Error::from)?;

        let analytics_events_client = match self.analytics_events_client.read() {
            Ok(client) => client.clone(),
            Err(err) => err.into_inner().clone(),
        };
        if let Some(analytics_events_client) = analytics_events_client {
            analytics_events_client.track_plugin_installed(
                self.telemetry_metadata_for_installed_plugin(&result.plugin_id)
                    .await,
            );
        }

        Ok(PluginInstallOutcome {
            plugin_id: result.plugin_id,
            plugin_version: result.plugin_version,
            installed_path: result.installed_path,
            auth_policy,
        })
    }

    pub async fn uninstall_plugin(&self, plugin_id: String) -> Result<(), PluginUninstallError> {
        let plugin_id = PluginId::parse(&plugin_id)?;
        self.uninstall_plugin_id(plugin_id).await
    }

    async fn uninstall_plugin_id(&self, plugin_id: PluginId) -> Result<(), PluginUninstallError> {
        let plugin_telemetry = if self.store.active_plugin_root(&plugin_id).is_some() {
            Some(
                self.telemetry_metadata_for_installed_plugin(&plugin_id)
                    .await,
            )
        } else {
            None
        };
        let store = self.store.clone();
        let plugin_id_for_store = plugin_id.clone();
        tokio::task::spawn_blocking(move || store.uninstall(&plugin_id_for_store))
            .await
            .map_err(PluginUninstallError::join)??;

        clear_user_plugin(&self.ody_home, plugin_id.as_key())
            .await
            .map_err(anyhow::Error::from)?;

        let analytics_events_client = match self.analytics_events_client.read() {
            Ok(client) => client.clone(),
            Err(err) => err.into_inner().clone(),
        };
        if let Some(plugin_telemetry) = plugin_telemetry
            && let Some(analytics_events_client) = analytics_events_client
        {
            analytics_events_client.track_plugin_uninstalled(plugin_telemetry);
        }

        Ok(())
    }

    pub fn list_marketplaces_for_config(
        &self,
        config: &PluginsConfigInput,
        additional_roots: &[AbsolutePathBuf],
        include_odysseythink_curated: bool,
    ) -> Result<ConfiguredMarketplaceListOutcome, MarketplaceError> {
        if !config.plugins_enabled {
            return Ok(ConfiguredMarketplaceListOutcome::default());
        }

        let (installed_plugins, enabled_plugins) = self.configured_plugin_states(config);
        let marketplace_roots =
            self.marketplace_roots(config, additional_roots, include_odysseythink_curated);
        let marketplace_outcome = list_marketplaces(&marketplace_roots)?;
        let mut seen_plugin_keys = HashSet::new();
        let marketplaces = marketplace_outcome
            .marketplaces
            .into_iter()
            .filter_map(|marketplace| {
                let marketplace_name = marketplace.name.clone();
                let plugins = marketplace
                    .plugins
                    .into_iter()
                    .filter_map(|plugin| {
                        let plugin_key = format!("{}@{marketplace_name}", plugin.name);
                        if !seen_plugin_keys.insert(plugin_key.clone()) {
                            return None;
                        }
                        if !self.restriction_product_matches(plugin.policy.products.as_deref()) {
                            return None;
                        }
                        let plugin_id =
                            PluginId::new(plugin.name.clone(), marketplace_name.clone()).ok();
                        let installed = installed_plugins.contains(&plugin_key);
                        let installed_version = installed.then_some(()).and_then(|_| {
                            plugin_id
                                .as_ref()
                                .and_then(|plugin_id| self.store.active_plugin_version(plugin_id))
                        });
                        let enabled = enabled_plugins.contains(&plugin_key);
                        let mut interface = plugin.interface;
                        let mut local_version = plugin.local_version;
                        let manifest_fallback = plugin.manifest_fallback.clone();
                        if installed
                            && matches!(&plugin.source, MarketplacePluginSource::Git { .. })
                            && let Some(plugin_id) = plugin_id.as_ref()
                            && let Some(plugin_root) = self.store.active_plugin_root(plugin_id)
                            && let Some(manifest) = load_plugin_manifest(plugin_root.as_path())
                        {
                            local_version = manifest.version.clone();
                            let marketplace_category = interface
                                .as_ref()
                                .and_then(|interface| interface.category.clone());
                            interface = plugin_interface_with_marketplace_category(
                                manifest.interface,
                                marketplace_category,
                            );
                        }

                        Some(ConfiguredMarketplacePlugin {
                            // Enabled state is keyed by `<plugin>@<marketplace>`, so duplicate
                            // plugin entries from duplicate marketplace files intentionally
                            // resolve to the first discovered source.
                            id: plugin_key,
                            installed_version,
                            installed,
                            enabled,
                            name: plugin.name,
                            local_version,
                            source: plugin.source,
                            policy: plugin.policy,
                            keywords: plugin.keywords,
                            interface,
                            manifest_fallback,
                        })
                    })
                    .collect::<Vec<_>>();

                (!plugins.is_empty()).then_some(ConfiguredMarketplace {
                    name: marketplace.name,
                    path: marketplace.path,
                    interface: marketplace.interface,
                    plugins,
                })
            })
            .collect();

        Ok(ConfiguredMarketplaceListOutcome {
            marketplaces,
            errors: marketplace_outcome.errors,
        })
    }

    pub fn discover_marketplaces_for_config(
        &self,
        config: &PluginsConfigInput,
        additional_roots: &[AbsolutePathBuf],
    ) -> Result<MarketplaceListOutcome, MarketplaceError> {
        if !config.plugins_enabled {
            return Ok(MarketplaceListOutcome::default());
        }

        list_marketplaces(&self.marketplace_roots(
            config,
            additional_roots,
            /*include_odysseythink_curated*/ true,
        ))
    }

    pub(crate) async fn tool_suggest_metadata_for_marketplace_plugin(
        &self,
        marketplace_name: &str,
        plugin: &ConfiguredMarketplacePlugin,
        skill_config_rules: &SkillConfigRules,
    ) -> Result<PluginCapabilitySummary, MarketplaceError> {
        let fragment = self
            .tool_suggest_metadata_cache
            .metadata_for_plugin(marketplace_name, plugin, self.restriction_product)
            .await?;
        Ok(fragment.project(skill_config_rules, self.auth_mode()))
    }

    pub async fn read_plugin_for_config(
        &self,
        config: &PluginsConfigInput,
        request: &PluginReadRequest,
    ) -> Result<PluginReadOutcome, MarketplaceError> {
        if !config.plugins_enabled {
            return Err(MarketplaceError::PluginsDisabled);
        }

        let plugin = find_marketplace_plugin(&request.marketplace_path, &request.plugin_name)?;
        if !self.restriction_product_matches(plugin.policy.products.as_deref()) {
            return Err(MarketplaceError::PluginNotFound {
                plugin_name: plugin.plugin_id.plugin_name,
                marketplace_name: plugin.plugin_id.marketplace_name,
            });
        }

        let marketplace_name = plugin.plugin_id.marketplace_name.clone();
        let plugin_key = plugin.plugin_id.as_key();
        let manifest_fallback = plugin
            .manifest_fallback
            .contents_if_has_metadata()
            .map(|_| plugin.manifest_fallback.clone());
        let (installed_plugins, enabled_plugins) = self.configured_plugin_states(config);
        let installed = installed_plugins.contains(&plugin_key);
        let installed_version = if installed {
            self.store.active_plugin_version(&plugin.plugin_id)
        } else {
            None
        };
        let plugin = self
            .read_plugin_detail_for_marketplace_plugin(
                config,
                &marketplace_name,
                ConfiguredMarketplacePlugin {
                    id: plugin_key.clone(),
                    name: plugin.plugin_id.plugin_name,
                    local_version: plugin
                        .manifest
                        .as_ref()
                        .and_then(|manifest| manifest.version.clone()),
                    installed_version,
                    source: plugin.source,
                    policy: plugin.policy,
                    interface: plugin.interface,
                    keywords: plugin
                        .manifest
                        .as_ref()
                        .map(|manifest| manifest.keywords.clone())
                        .unwrap_or_default(),
                    manifest_fallback,
                    installed,
                    enabled: enabled_plugins.contains(&plugin_key),
                },
            )
            .await?;

        Ok(PluginReadOutcome {
            marketplace_name,
            marketplace_path: Some(request.marketplace_path.clone()),
            plugin,
        })
    }

    #[instrument(level = "trace", skip_all)]
    pub async fn read_plugin_detail_for_marketplace_plugin(
        &self,
        config: &PluginsConfigInput,
        marketplace_name: &str,
        plugin: ConfiguredMarketplacePlugin,
    ) -> Result<PluginDetail, MarketplaceError> {
        if !self.restriction_product_matches(plugin.policy.products.as_deref()) {
            return Err(MarketplaceError::PluginNotFound {
                plugin_name: plugin.name,
                marketplace_name: marketplace_name.to_string(),
            });
        }

        let plugin_id =
            PluginId::new(plugin.name.clone(), marketplace_name.to_string()).map_err(|err| {
                match err {
                    PluginIdError::Invalid(message) => MarketplaceError::InvalidPlugin(message),
                }
            })?;
        let plugin_key = plugin_id.as_key();
        if matches!(plugin.source, MarketplacePluginSource::Git { .. }) && !plugin.installed {
            let description = remote_plugin_install_required_description(&plugin.source);
            return Ok(PluginDetail {
                id: plugin_key,
                name: plugin.name,
                local_version: None,
                description: Some(description),
                source: plugin.source,
                policy: plugin.policy,
                interface: plugin.interface,
                keywords: plugin.keywords,
                installed: plugin.installed,
                enabled: plugin.enabled,
                skills: Vec::new(),
                disabled_skill_paths: HashSet::new(),
                hooks: Vec::new(),
                apps: Vec::new(),
                app_category_by_id: HashMap::new(),
                mcp_server_names: Vec::new(),
                details_unavailable_reason: Some(
                    PluginDetailsUnavailableReason::InstallRequiredForRemoteSource,
                ),
            });
        }

        let source_path =
            if matches!(plugin.source, MarketplacePluginSource::Git { .. }) && plugin.installed {
                self.store.active_plugin_root(&plugin_id).ok_or_else(|| {
                    MarketplaceError::InvalidPlugin(format!(
                        "installed plugin cache entry is missing for {plugin_key}"
                    ))
                })?
            } else {
                let ody_home = self.ody_home.clone();
                let source = plugin.source.clone();
                let materialized = tokio::task::spawn_blocking(move || {
                    materialize_marketplace_plugin_source(ody_home.as_path(), &source)
                })
                .await
                .map_err(|err| {
                    MarketplaceError::InvalidPlugin(format!(
                        "failed to materialize plugin source: {err}"
                    ))
                })?
                .map_err(MarketplaceError::InvalidPlugin)?;
                materialized.path.clone()
            };
        if !source_path.as_path().is_dir() {
            return Err(MarketplaceError::InvalidPlugin(
                "path does not exist or is not a directory".to_string(),
            ));
        }
        let manifest =
            if ody_utils_plugins::find_plugin_manifest_path(source_path.as_path()).is_some() {
                load_plugin_manifest(source_path.as_path())
            } else {
                plugin
                    .manifest_fallback
                    .as_ref()
                    .and_then(|fallback| fallback.parse_for_plugin_root(source_path.as_path()))
            }
            .ok_or_else(|| {
                MarketplaceError::InvalidPlugin("missing or invalid plugin.json".to_string())
            })?;
        let description = manifest.description.clone();
        let marketplace_category = plugin
            .interface
            .as_ref()
            .and_then(|interface| interface.category.clone());
        let interface = plugin_interface_with_marketplace_category(
            manifest.interface.clone(),
            marketplace_category,
        );
        let resolved_skills = load_plugin_skills(
            &source_path,
            &plugin_id,
            &manifest,
            self.restriction_product,
            &ody_core_skills::config_rules::skill_config_rules_from_stack(
                &config.config_layer_stack,
            ),
            /*plugin_skill_snapshots*/ None,
        )
        .await;
        let plugin_data_root = self.store.plugin_data_root(&plugin_id);
        let (hook_sources, _hook_load_warnings) =
            load_plugin_hooks(&source_path, &plugin_id, &plugin_data_root, &manifest.paths);
        let hooks = plugin_hook_declarations(&hook_sources)
            .into_iter()
            .map(|hook| PluginHookSummary {
                key: hook.key,
                event_name: hook.event_name,
            })
            .collect();
        let auth_mode = self.auth_mode();
        let mut app_declarations =
            load_plugin_apps_from_manifest(source_path.as_path(), &manifest.paths).await;
        let mut mcp_servers = load_plugin_mcp_servers_from_manifest(
            source_path.as_path(),
            &manifest.paths,
            /*plugin_policy*/ None,
        )
        .await;
        if auth_mode.is_some() {
            apply_app_mcp_routing_policy(
                &mut app_declarations,
                &mut mcp_servers,
                auth_mode,
                /*plugin_active*/ true,
            );
        }
        let apps = app_connector_ids_from_declarations(&app_declarations);
        let mut seen_app_connector_ids = HashSet::new();
        let mut app_category_by_id = HashMap::new();
        for app in &app_declarations {
            if seen_app_connector_ids.insert(app.connector_id.0.as_str())
                && let Some(category) = &app.category
            {
                app_category_by_id.insert(app.connector_id.0.clone(), category.clone());
            }
        }
        let mut mcp_server_names = mcp_servers.into_keys().collect::<Vec<_>>();
        mcp_server_names.sort_unstable();
        mcp_server_names.dedup();

        Ok(PluginDetail {
            id: plugin.id,
            name: plugin.name,
            local_version: manifest.version.clone(),
            description,
            source: plugin.source,
            policy: plugin.policy,
            interface,
            keywords: manifest.keywords,
            installed: plugin.installed,
            enabled: plugin.enabled,
            skills: resolved_skills.skills,
            disabled_skill_paths: resolved_skills.disabled_skill_paths,
            hooks,
            apps,
            app_category_by_id,
            mcp_server_names,
            details_unavailable_reason: None,
        })
    }

    pub fn maybe_start_plugin_startup_tasks_for_config(
        self: &Arc<Self>,
        config: &PluginsConfigInput,
        _auth_manager: Arc<AuthManager>,
        _on_effective_plugins_changed: Option<Arc<dyn Fn() + Send + Sync + 'static>>,
    ) {
        if config.plugins_enabled {
            let should_spawn_marketplace_auto_upgrade = {
                let mut state = match self.configured_marketplace_upgrade_state.write() {
                    Ok(state) => state,
                    Err(err) => err.into_inner(),
                };
                if state.in_flight {
                    false
                } else {
                    state.in_flight = true;
                    true
                }
            };
            if should_spawn_marketplace_auto_upgrade {
                let manager = Arc::clone(self);
                let config = config.clone();
                if let Err(err) = std::thread::Builder::new()
                    .name("plugins-marketplace-auto-upgrade".to_string())
                    .spawn(move || {
                        let outcome = manager.upgrade_configured_marketplaces_for_config(
                            &config, /*marketplace_name*/ None,
                        );
                        match outcome {
                            Ok(outcome) => {
                                for error in outcome.errors {
                                    warn!(
                                        marketplace = error.marketplace_name,
                                        error = %error.message,
                                        "failed to auto-upgrade configured marketplace"
                                    );
                                }
                            }
                            Err(err) => {
                                warn!("failed to auto-upgrade configured marketplaces: {err}");
                            }
                        }

                        let mut state = match manager.configured_marketplace_upgrade_state.write() {
                            Ok(state) => state,
                            Err(err) => err.into_inner(),
                        };
                        state.in_flight = false;
                    })
                {
                    let mut state = match self.configured_marketplace_upgrade_state.write() {
                        Ok(state) => state,
                        Err(err) => err.into_inner(),
                    };
                    state.in_flight = false;
                    warn!("failed to start configured marketplace auto-upgrade task: {err}");
                }
            }
        }
    }

    pub fn upgrade_configured_marketplaces_for_config(
        &self,
        config: &PluginsConfigInput,
        marketplace_name: Option<&str>,
    ) -> Result<ConfiguredMarketplaceUpgradeOutcome, String> {
        if let Some(marketplace_name) = marketplace_name
            && !configured_git_marketplace_names(&config.config_layer_stack)
                .iter()
                .any(|name| name == marketplace_name)
        {
            return Err(format!(
                "marketplace `{marketplace_name}` is not configured as a Git marketplace"
            ));
        }

        let mut outcome = upgrade_configured_git_marketplaces(
            self.ody_home.as_path(),
            &config.config_layer_stack,
            marketplace_name,
        );
        if !outcome.upgraded_roots.is_empty() {
            match refresh_non_curated_plugin_cache_force_reinstall(
                self.ody_home.as_path(),
                &outcome.upgraded_roots,
            ) {
                Ok(cache_refreshed) => {
                    self.clear_caches_after_marketplace_source_refresh(cache_refreshed);
                }
                Err(err) => {
                    self.clear_cache();
                    outcome.errors.push(ConfiguredMarketplaceUpgradeError {
                        marketplace_name: marketplace_name
                            .unwrap_or("all configured marketplaces")
                            .to_string(),
                        message: format!(
                            "failed to refresh installed plugin cache after marketplace upgrade: {err}"
                        ),
                    });
                }
            }
        }
        Ok(outcome)
    }

    pub fn maybe_start_non_curated_plugin_cache_refresh(
        self: &Arc<Self>,
        roots: &[AbsolutePathBuf],
    ) {
        self.schedule_non_curated_plugin_cache_refresh(
            roots,
            NonCuratedCacheRefreshMode::IfVersionChanged,
        );
    }


    fn schedule_non_curated_plugin_cache_refresh(
        self: &Arc<Self>,
        roots: &[AbsolutePathBuf],
        mode: NonCuratedCacheRefreshMode,
    ) {
        let mut roots = roots.to_vec();
        roots.sort_unstable();
        roots.dedup();
        if roots.is_empty() {
            return;
        }
        let request = NonCuratedCacheRefreshRequest { roots, mode };

        let should_spawn = {
            let mut state = match self.non_curated_cache_refresh_state.write() {
                Ok(state) => state,
                Err(err) => err.into_inner(),
            };
            // Collapse repeated plugin/list requests onto one worker and only queue another pass
            // when the requested roots set actually changes. Forced reinstall requests are not
            // deduped against the last completed pass because the same marketplace root path can
            // point at newly activated files after an auto-upgrade.
            if state.requested.as_ref() == Some(&request)
                || (mode == NonCuratedCacheRefreshMode::IfVersionChanged
                    && !state.in_flight
                    && state.last_refreshed.as_ref() == Some(&request))
            {
                return;
            }
            if mode == NonCuratedCacheRefreshMode::IfVersionChanged
                && state.requested.as_ref().is_some_and(|requested| {
                    requested.mode == NonCuratedCacheRefreshMode::ForceReinstall
                        && requested.roots == request.roots
                })
            {
                return;
            }
            state.requested = Some(request);
            if state.in_flight {
                false
            } else {
                state.in_flight = true;
                true
            }
        };
        if !should_spawn {
            return;
        }

        let manager = Arc::clone(self);
        if let Err(err) = std::thread::Builder::new()
            .name("plugins-non-curated-cache-refresh".to_string())
            .spawn(move || manager.run_non_curated_plugin_cache_refresh_loop())
        {
            let mut state = match self.non_curated_cache_refresh_state.write() {
                Ok(state) => state,
                Err(err) => err.into_inner(),
            };
            state.in_flight = false;
            state.requested = None;
            warn!("failed to start non-curated plugin cache refresh task: {err}");
        }
    }


    fn run_non_curated_plugin_cache_refresh_loop(self: Arc<Self>) {
        loop {
            let request = {
                let state = match self.non_curated_cache_refresh_state.read() {
                    Ok(state) => state,
                    Err(err) => err.into_inner(),
                };
                state.requested.clone()
            };

            let Some(request) = request else {
                let mut state = match self.non_curated_cache_refresh_state.write() {
                    Ok(state) => state,
                    Err(err) => err.into_inner(),
                };
                state.in_flight = false;
                return;
            };

            let refresh_result = match request.mode {
                NonCuratedCacheRefreshMode::IfVersionChanged => {
                    refresh_non_curated_plugin_cache(self.ody_home.as_path(), &request.roots)
                }
                NonCuratedCacheRefreshMode::ForceReinstall => {
                    refresh_non_curated_plugin_cache_force_reinstall(
                        self.ody_home.as_path(),
                        &request.roots,
                    )
                }
            };
            let refreshed = match refresh_result {
                Ok(cache_refreshed) => {
                    if cache_refreshed {
                        self.clear_cache();
                    }
                    true
                }
                Err(err) => {
                    self.clear_cache();
                    warn!("failed to refresh non-curated plugin cache: {err}");
                    false
                }
            };

            let mut state = match self.non_curated_cache_refresh_state.write() {
                Ok(state) => state,
                Err(err) => err.into_inner(),
            };
            if refreshed {
                state.last_refreshed = Some(request.clone());
            }
            if state.requested.as_ref() == Some(&request) {
                state.requested = None;
                state.in_flight = false;
                return;
            }
        }
    }

    fn configured_plugin_states(
        &self,
        config: &PluginsConfigInput,
    ) -> (HashSet<String>, HashSet<String>) {
        let configured_plugins = configured_plugins_from_stack(&config.config_layer_stack);
        let installed_plugins = configured_plugins
            .keys()
            .filter(|plugin_key| {
                PluginId::parse(plugin_key)
                    .ok()
                    .is_some_and(|plugin_id| self.store.is_installed(&plugin_id))
            })
            .cloned()
            .collect::<HashSet<_>>();
        let enabled_plugins = configured_plugins
            .into_iter()
            .filter_map(|(plugin_key, plugin)| plugin.enabled.then_some(plugin_key))
            .collect::<HashSet<_>>();
        (installed_plugins, enabled_plugins)
    }

    fn marketplace_roots(
        &self,
        config: &PluginsConfigInput,
        additional_roots: &[AbsolutePathBuf],
        include_odysseythink_curated: bool,
    ) -> Vec<AbsolutePathBuf> {
        // Treat the curated catalog as an extra marketplace root so plugin listing can surface it
        // without requiring every caller to know where it is stored.
        let mut roots = additional_roots.to_vec();
        roots.extend(installed_marketplace_roots_from_layer_stack(
            &config.config_layer_stack,
            self.ody_home.as_path(),
        ));
        let curated_marketplace_path = if include_odysseythink_curated {
            if matches!(self.auth_mode(), Some(AuthMode::ApiKey)) {
                let api_marketplace_path =
                    curated_plugins_api_marketplace_path(self.ody_home.as_path());
                api_marketplace_path
                    .is_file()
                    .then_some(api_marketplace_path)
            } else {
                let curated_repo_root = curated_plugins_repo_path(self.ody_home.as_path());
                curated_repo_root.is_dir().then_some(curated_repo_root)
            }
        } else {
            None
        };
        if let Some(curated_marketplace_path) = curated_marketplace_path
            && let Ok(curated_marketplace_path) =
                AbsolutePathBuf::try_from(curated_marketplace_path)
        {
            roots.push(curated_marketplace_path);
        }
        roots.sort_unstable();
        roots.dedup();
        roots
    }
}

pub(crate) fn remote_plugin_install_required_description(
    source: &MarketplacePluginSource,
) -> String {
    let source_description = match source {
        MarketplacePluginSource::Git {
            url,
            path,
            ref_name,
            sha,
        } => {
            let mut parts = vec![url.clone()];
            if let Some(path) = path {
                parts.push(format!("path `{path}`"));
            }
            if let Some(ref_name) = ref_name {
                parts.push(format!("ref `{ref_name}`"));
            }
            if let Some(sha) = sha {
                parts.push(format!("sha `{sha}`"));
            }
            parts.join(", ")
        }
        MarketplacePluginSource::Local { path } => path.as_path().display().to_string(),
    };

    format!(
        "This is a cross-repo plugin. Install it to view more detailed information. The source of the plugin is {source_description}."
    )
}

#[derive(Debug, thiserror::Error)]
pub enum PluginInstallError {
    #[error("{0}")]
    Marketplace(#[from] MarketplaceError),

    #[error("{0}")]
    Store(#[from] PluginStoreError),

    #[error("{0}")]
    Config(#[from] anyhow::Error),

    #[error("failed to join plugin install task: {0}")]
    Join(#[from] tokio::task::JoinError),
}

impl PluginInstallError {
    fn join(source: tokio::task::JoinError) -> Self {
        Self::Join(source)
    }

    pub fn is_invalid_request(&self) -> bool {
        matches!(
            self,
            Self::Marketplace(
                MarketplaceError::MarketplaceNotFound { .. }
                    | MarketplaceError::InvalidMarketplaceFile { .. }
                    | MarketplaceError::PluginNotFound { .. }
                    | MarketplaceError::PluginNotAvailable { .. }
                    | MarketplaceError::InvalidPlugin(_)
            ) | Self::Store(PluginStoreError::Invalid(_))
        )
    }
}

fn plugin_install_error_type(err: &PluginInstallError) -> &'static str {
    match err {
        PluginInstallError::Marketplace(err) => marketplace_error_type(err),
        PluginInstallError::Store(err) => plugin_store_error_type(err),
        PluginInstallError::Config(_) => "config",
        PluginInstallError::Join(_) => "join",
    }
}

fn marketplace_error_type(err: &MarketplaceError) -> &'static str {
    match err {
        MarketplaceError::Io { .. } => "marketplace_io",
        MarketplaceError::MarketplaceNotFound { .. } => "marketplace_not_found",
        MarketplaceError::InvalidMarketplaceFile { .. } => "invalid_marketplace_file",
        MarketplaceError::PluginNotFound { .. } => "plugin_not_found",
        MarketplaceError::PluginNotAvailable { .. } => "plugin_not_available",
        MarketplaceError::PluginsDisabled => "plugins_disabled",
        MarketplaceError::InvalidPlugin(_) => "invalid_plugin",
    }
}

fn plugin_store_error_type(err: &PluginStoreError) -> &'static str {
    match err {
        PluginStoreError::Io { .. } => "store_io",
        PluginStoreError::Invalid(_) => "store_invalid",
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PluginUninstallError {
    #[error("{0}")]
    InvalidPluginId(#[from] PluginIdError),

    #[error("{0}")]
    Store(#[from] PluginStoreError),

    #[error("{0}")]
    Config(#[from] anyhow::Error),

    #[error("failed to join plugin uninstall task: {0}")]
    Join(#[from] tokio::task::JoinError),
}

impl PluginUninstallError {
    fn join(source: tokio::task::JoinError) -> Self {
        Self::Join(source)
    }

    pub fn is_invalid_request(&self) -> bool {
        matches!(self, Self::InvalidPluginId(_))
    }
}

pub(crate) fn configured_plugins_from_stack(
    config_layer_stack: &ConfigLayerStack,
) -> HashMap<String, PluginConfig> {
    // Plugin entries remain persisted user config only.
    let Some(user_config) = config_layer_stack.effective_user_config() else {
        return HashMap::new();
    };
    configured_plugins_from_user_config_value(&user_config)
}

fn configured_plugins_from_user_config_value(
    user_config: &toml::Value,
) -> HashMap<String, PluginConfig> {
    let Some(plugins_value) = user_config.get("plugins") else {
        return HashMap::new();
    };
    match plugins_value.clone().try_into() {
        Ok(plugins) => plugins,
        Err(err) => {
            warn!("invalid plugins config: {err}");
            HashMap::new()
        }
    }
}

#[cfg(test)]
#[path = "manager_tests.rs"]
mod tests;
