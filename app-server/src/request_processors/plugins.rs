use super::*;
use crate::error_code::internal_error;
use crate::error_code::invalid_request;
use ody_app_server_protocol::PluginAvailability;
use ody_config::types::McpServerConfig;
use ody_core_plugins::PluginListBackgroundTaskOptions;
use ody_core_plugins::is_odysseythink_curated_marketplace_name;
use ody_mcp::McpOAuthLoginSupport;
use ody_mcp::oauth_login_support;
use ody_mcp::should_retry_without_scopes;
use ody_rmcp_client::perform_oauth_login_silent;

#[derive(Clone)]
pub(crate) struct PluginRequestProcessor {
    auth_manager: Arc<AuthManager>,
    thread_manager: Arc<ThreadManager>,
    outgoing: Arc<OutgoingMessageSender>,
    config_manager: ConfigManager,
    workspace_settings_cache: Arc<workspace_settings::WorkspaceSettingsCache>,
}

fn plugin_skills_to_info(
    skills: &[ody_core::skills::SkillMetadata],
    disabled_skill_paths: &HashSet<AbsolutePathBuf>,
) -> Vec<SkillSummary> {
    skills
        .iter()
        .map(|skill| SkillSummary {
            name: skill.name.clone(),
            description: skill.description.clone(),
            short_description: skill.short_description.clone(),
            interface: skill.interface.clone().map(|interface| {
                ody_app_server_protocol::SkillInterface {
                    display_name: interface.display_name,
                    short_description: interface.short_description,
                    icon_small: interface.icon_small,
                    icon_large: interface.icon_large,
                    brand_color: interface.brand_color,
                    default_prompt: interface.default_prompt,
                }
            }),
            path: Some(skill.path_to_skills_md.clone()),
            enabled: !disabled_skill_paths.contains(&skill.path_to_skills_md),
        })
        .collect()
}

fn local_plugin_interface_to_info(interface: PluginManifestInterface) -> PluginInterface {
    PluginInterface {
        display_name: interface.display_name,
        short_description: interface.short_description,
        long_description: interface.long_description,
        developer_name: interface.developer_name,
        category: interface.category,
        capabilities: interface.capabilities,
        website_url: interface.website_url,
        privacy_policy_url: interface.privacy_policy_url,
        terms_of_service_url: interface.terms_of_service_url,
        default_prompt: interface.default_prompt,
        brand_color: interface.brand_color,
        composer_icon: interface.composer_icon,
        composer_icon_url: None,
        logo: interface.logo,
        logo_dark: interface.logo_dark,
        logo_url: None,
        logo_url_dark: None,
        screenshots: interface.screenshots,
        screenshot_urls: Vec::new(),
    }
}

fn marketplace_plugin_source_to_info(source: MarketplacePluginSource) -> PluginSource {
    match source {
        MarketplacePluginSource::Local { path } => PluginSource::Local { path },
        MarketplacePluginSource::Git {
            url,
            path,
            ref_name,
            sha,
        } => PluginSource::Git {
            url,
            path,
            ref_name,
            sha,
        },
    }
}

fn load_shared_plugin_ids_by_local_path(
    _config: &Config,
) -> Result<std::collections::BTreeMap<AbsolutePathBuf, String>, JSONRPCErrorError> {
    // Plugin sharing was implemented entirely via the remote hosted plugin catalog, which has
    // been removed, so there is no longer any local-path-to-remote-share-id mapping to load.
    Ok(std::collections::BTreeMap::new())
}

fn share_context_for_source(
    source: &MarketplacePluginSource,
    shared_plugin_ids_by_local_path: &std::collections::BTreeMap<AbsolutePathBuf, String>,
) -> Option<PluginShareContext> {
    match source {
        MarketplacePluginSource::Local { path } => shared_plugin_ids_by_local_path
            .get(path)
            .cloned()
            .map(|remote_plugin_id| PluginShareContext {
                remote_plugin_id,
                remote_version: None,
                discoverability: None,
                share_url: None,
                creator_account_user_id: None,
                creator_name: None,
                share_principals: None,
            }),
        MarketplacePluginSource::Git { .. } => None,
    }
}

fn convert_configured_marketplace_plugin_to_plugin_summary(
    plugin: ody_core_plugins::ConfiguredMarketplacePlugin,
    shared_plugin_ids_by_local_path: &std::collections::BTreeMap<AbsolutePathBuf, String>,
) -> PluginSummary {
    let share_context = share_context_for_source(&plugin.source, shared_plugin_ids_by_local_path);
    PluginSummary {
        id: plugin.id,
        remote_plugin_id: None,
        local_version: plugin.local_version,
        installed: plugin.installed,
        enabled: plugin.enabled,
        name: plugin.name,
        share_context,
        source: marketplace_plugin_source_to_info(plugin.source),
        install_policy: plugin.policy.installation.into(),
        auth_policy: plugin.policy.authentication.into(),
        availability: PluginAvailability::Available,
        interface: plugin.interface.map(local_plugin_interface_to_info),
        keywords: plugin.keywords,
    }
}

// The remote hosted plugin catalog (and its "odysseythink-curated-remote" marketplace) has been
// removed. `marketplaces` passed into this function will never contain that marketplace name
// anymore, so this de-conflict pass is now unreachable in practice, but is kept as a harmless
// no-op rather than deleting the (still locally meaningful) curated-marketplace-name check.
const REMOTE_GLOBAL_MARKETPLACE_NAME: &str = "odysseythink-curated-remote";

fn filter_odysseythink_curated_installed_conflicts(
    marketplaces: &mut Vec<PluginMarketplaceEntry>,
    prefer_remote_curated_conflicts: bool,
) {
    let local_installed_plugin_names = marketplaces
        .iter()
        .filter(|marketplace| is_odysseythink_curated_marketplace_name(&marketplace.name))
        .flat_map(|marketplace| installed_plugin_names(&marketplace.plugins))
        .collect::<HashSet<_>>();
    let remote_installed_plugin_names = marketplaces
        .iter()
        .find(|marketplace| marketplace.name == REMOTE_GLOBAL_MARKETPLACE_NAME)
        .map(|marketplace| installed_plugin_names(&marketplace.plugins))
        .unwrap_or_default();
    let conflicting_plugin_names = local_installed_plugin_names
        .intersection(&remote_installed_plugin_names)
        .cloned()
        .collect::<HashSet<_>>();
    if conflicting_plugin_names.is_empty() {
        return;
    }

    for marketplace in marketplaces.iter_mut() {
        if prefer_remote_curated_conflicts {
            if !is_odysseythink_curated_marketplace_name(&marketplace.name) {
                continue;
            }
        } else if marketplace.name != REMOTE_GLOBAL_MARKETPLACE_NAME {
            continue;
        }
        marketplace
            .plugins
            .retain(|plugin| !plugin.installed || !conflicting_plugin_names.contains(&plugin.name));
    }
    marketplaces.retain(|marketplace| !marketplace.plugins.is_empty());
}

fn installed_plugin_names(plugins: &[PluginSummary]) -> HashSet<String> {
    plugins
        .iter()
        .filter(|plugin| plugin.installed)
        .map(|plugin| plugin.name.clone())
        .collect()
}

/// Format check for the plugin-id shape previously used to identify plugins served by the
/// (now removed) remote hosted plugin catalog. Kept only so `plugin/install` and
/// `plugin/uninstall` can distinguish "this looks like a remote-catalog-style id" from "this is
/// not a valid plugin id at all" when producing a "no longer supported" error.
fn is_valid_remote_plugin_id(plugin_id: &str) -> bool {
    !plugin_id.is_empty()
        && plugin_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '~')
}


impl PluginRequestProcessor {
    pub(crate) fn new(
        auth_manager: Arc<AuthManager>,
        thread_manager: Arc<ThreadManager>,
        outgoing: Arc<OutgoingMessageSender>,
        config_manager: ConfigManager,
        workspace_settings_cache: Arc<workspace_settings::WorkspaceSettingsCache>,
    ) -> Self {
        Self {
            auth_manager,
            thread_manager,
            outgoing,
            config_manager,
            workspace_settings_cache,
        }
    }

    pub(crate) async fn plugin_list(
        &self,
        params: PluginListParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.plugin_list_response(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn plugin_installed(
        &self,
        params: PluginInstalledParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.plugin_installed_response(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn plugin_read(
        &self,
        params: PluginReadParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.plugin_read_response(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn plugin_skill_read(
        &self,
        params: PluginSkillReadParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.plugin_skill_read_response(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn plugin_share_save(
        &self,
        params: PluginShareSaveParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.plugin_share_save_response(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn plugin_share_update_targets(
        &self,
        params: PluginShareUpdateTargetsParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.plugin_share_update_targets_response(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn plugin_share_list(
        &self,
        params: PluginShareListParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.plugin_share_list_response(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn plugin_share_checkout(
        &self,
        params: PluginShareCheckoutParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.plugin_share_checkout_response(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn plugin_share_delete(
        &self,
        params: PluginShareDeleteParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.plugin_share_delete_response(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn plugin_install(
        &self,
        params: PluginInstallParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.plugin_install_response(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn plugin_uninstall(
        &self,
        params: PluginUninstallParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.plugin_uninstall_response(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) fn effective_plugins_changed_callback(&self) -> Arc<dyn Fn() + Send + Sync> {
        let thread_manager = Arc::clone(&self.thread_manager);
        let config_manager = self.config_manager.clone();
        Arc::new(move || {
            Self::spawn_effective_plugins_changed_task(
                Arc::clone(&thread_manager),
                config_manager.clone(),
            );
        })
    }

    fn on_effective_plugins_changed(&self) {
        Self::spawn_effective_plugins_changed_task(
            Arc::clone(&self.thread_manager),
            self.config_manager.clone(),
        );
    }

    fn spawn_effective_plugins_changed_task(
        thread_manager: Arc<ThreadManager>,
        config_manager: ConfigManager,
    ) {
        tokio::spawn(async move {
            thread_manager.plugins_manager().clear_cache();
            thread_manager.skills_service().clear_cache();
            if thread_manager.list_thread_ids().await.is_empty() {
                return;
            }
            crate::mcp_refresh::queue_best_effort_refresh(&thread_manager, &config_manager).await;
        });
    }

    fn clear_plugin_related_caches(&self) {
        self.thread_manager.plugins_manager().clear_cache();
        self.thread_manager.skills_service().clear_cache();
    }

    async fn load_latest_config(
        &self,
        fallback_cwd: Option<PathBuf>,
    ) -> Result<Config, JSONRPCErrorError> {
        self.config_manager
            .load_latest_config(fallback_cwd)
            .await
            .map_err(|err| internal_error(format!("failed to reload config: {err}")))
    }

    async fn workspace_ody_plugins_enabled(
        &self,
        config: &Config,
        auth: Option<&OdyAuth>,
    ) -> bool {
        match workspace_settings::ody_plugins_enabled_for_workspace(
            config,
            auth,
            Some(&self.workspace_settings_cache),
        )
        .await
        {
            Ok(enabled) => enabled,
            Err(err) => {
                warn!(
                    "failed to fetch workspace Ody plugins setting; allowing Ody plugins: {err:#}"
                );
                true
            }
        }
    }

    async fn plugin_list_response(
        &self,
        params: PluginListParams,
    ) -> Result<PluginListResponse, JSONRPCErrorError> {
        let plugins_manager = self.thread_manager.plugins_manager();
        let PluginListParams {
            cwds,
            marketplace_kinds,
        } = params;
        let roots = cwds.unwrap_or_default();
        let marketplace_kinds =
            marketplace_kinds.unwrap_or_else(|| vec![PluginListMarketplaceKind::Local]);
        let include_local = marketplace_kinds.contains(&PluginListMarketplaceKind::Local);
        let include_vertical = marketplace_kinds.contains(&PluginListMarketplaceKind::Vertical);

        let config = self.load_latest_config(/*fallback_cwd*/ None).await?;
        let empty_response = || PluginListResponse {
            marketplaces: Vec::new(),
            marketplace_load_errors: Vec::new(),
            featured_plugin_ids: Vec::new(),
        };
        if !config.features.enabled(Feature::Plugins) {
            return Ok(empty_response());
        }
        let auth = self.auth_manager.auth().await;
        if !self
            .workspace_ody_plugins_enabled(&config, auth.as_ref())
            .await
        {
            return Ok(empty_response());
        }
        let auth_mode = auth.as_ref().map(OdyAuth::api_auth_mode);
        plugins_manager.set_auth_mode(auth_mode);
        let plugins_input = config.plugins_config_input();
        // The remote hosted plugin catalog (global/created-by-me remote marketplaces, remote
        // "vertical" collections) has been removed; plugin/list now always serves local
        // marketplaces regardless of the requested marketplace kinds.
        let use_remote_global_catalog = false;
        let (data, marketplace_load_errors) = if include_local {
            let config_for_marketplace_listing = plugins_input.clone();
            let plugins_manager_for_marketplace_listing = plugins_manager.clone();
            let roots_for_marketplace_listing = roots.clone();
            let shared_plugin_ids_by_local_path = load_shared_plugin_ids_by_local_path(&config)?;
            match tokio::task::spawn_blocking(move || {
                let outcome = plugins_manager_for_marketplace_listing
                    .list_marketplaces_for_config(
                        &config_for_marketplace_listing,
                        &roots_for_marketplace_listing,
                        /*include_odysseythink_curated*/ !use_remote_global_catalog,
                    )?;
                Ok::<
                    (
                        Vec<PluginMarketplaceEntry>,
                        Vec<ody_app_server_protocol::MarketplaceLoadErrorInfo>,
                    ),
                    MarketplaceError,
                >((
                    outcome
                        .marketplaces
                        .into_iter()
                        .map(|marketplace| PluginMarketplaceEntry {
                            name: marketplace.name,
                            path: Some(marketplace.path),
                            interface: marketplace.interface.map(|interface| {
                                MarketplaceInterface {
                                    display_name: interface.display_name,
                                }
                            }),
                            plugins: marketplace
                                .plugins
                                .into_iter()
                                .map(|plugin| {
                                    convert_configured_marketplace_plugin_to_plugin_summary(
                                        plugin,
                                        &shared_plugin_ids_by_local_path,
                                    )
                                })
                                .collect(),
                        })
                        .collect(),
                    outcome
                        .errors
                        .into_iter()
                        .map(|err| ody_app_server_protocol::MarketplaceLoadErrorInfo {
                            marketplace_path: err.path,
                            message: err.message,
                        })
                        .collect(),
                ))
            })
            .await
            {
                Ok(Ok(outcome)) => outcome,
                Ok(Err(err)) => {
                    return Err(Self::marketplace_error(err, "list marketplace plugins"));
                }
                Err(err) => {
                    return Err(internal_error(format!(
                        "failed to list marketplace plugins: {err}"
                    )));
                }
            }
        } else {
            (Vec::new(), Vec::new())
        };

        // The remote-hosted "vertical"/global/created-by-me/shared-with-me/workspace-directory
        // marketplace sources have all been removed along with the remote plugin catalog.
        let _ = include_vertical;
        let _ = marketplace_kinds;
        if include_local {
            plugins_manager.maybe_start_plugin_list_background_tasks_for_config(
                &plugins_input,
                &roots,
                PluginListBackgroundTaskOptions {},
            );
        }

        // Featured plugin ids were sourced from the remote plugin catalog and are no longer
        // available.
        let featured_plugin_ids = Vec::new();

        Ok(PluginListResponse {
            marketplaces: data,
            marketplace_load_errors,
            featured_plugin_ids,
        })
    }

    async fn plugin_installed_response(
        &self,
        params: PluginInstalledParams,
    ) -> Result<PluginInstalledResponse, JSONRPCErrorError> {
        let plugins_manager = self.thread_manager.plugins_manager();
        let PluginInstalledParams {
            cwds,
            install_suggestion_plugin_names,
        } = params;
        let roots = cwds.unwrap_or_default();
        let install_suggestion_plugin_names = install_suggestion_plugin_names
            .unwrap_or_default()
            .into_iter()
            .collect::<HashSet<_>>();

        let empty_response = || PluginInstalledResponse {
            marketplaces: Vec::new(),
            marketplace_load_errors: Vec::new(),
        };
        let config = self.load_latest_config(/*fallback_cwd*/ None).await?;
        if !config.features.enabled(Feature::Plugins) {
            return Ok(empty_response());
        }
        let auth = self.auth_manager.auth().await;
        if !self
            .workspace_ody_plugins_enabled(&config, auth.as_ref())
            .await
        {
            return Ok(empty_response());
        }
        plugins_manager.set_auth_mode(auth.as_ref().map(OdyAuth::api_auth_mode));

        let plugins_input = config.plugins_config_input();

        let (mut data, marketplace_load_errors) = self
            .load_local_installed_and_suggested_plugins(
                plugins_manager.clone(),
                &config,
                &plugins_input,
                roots,
                install_suggestion_plugin_names,
            )
            .await?;

        // Remote-installed plugin marketplaces (synced from the removed hosted plugin catalog)
        // no longer exist, so there is nothing to merge in or de-conflict against here.
        filter_odysseythink_curated_installed_conflicts(
            &mut data,
            /*prefer_remote_curated_conflicts*/ false,
        );

        Ok(PluginInstalledResponse {
            marketplaces: data,
            marketplace_load_errors,
        })
    }

    async fn load_local_installed_and_suggested_plugins(
        &self,
        plugins_manager: Arc<ody_core_plugins::PluginsManager>,
        config: &Config,
        plugins_input: &ody_core_plugins::PluginsConfigInput,
        roots: Vec<AbsolutePathBuf>,
        install_suggestion_plugin_names: HashSet<String>,
    ) -> Result<
        (
            Vec<PluginMarketplaceEntry>,
            Vec<ody_app_server_protocol::MarketplaceLoadErrorInfo>,
        ),
        JSONRPCErrorError,
    > {
        let config_for_marketplace_listing = plugins_input.clone();
        let shared_plugin_ids_by_local_path = load_shared_plugin_ids_by_local_path(config)?;
        match tokio::task::spawn_blocking(move || {
            let outcome = plugins_manager.list_marketplaces_for_config(
                &config_for_marketplace_listing,
                &roots,
                /*include_odysseythink_curated*/ true,
            )?;
            Ok::<
                (
                    Vec<PluginMarketplaceEntry>,
                    Vec<ody_app_server_protocol::MarketplaceLoadErrorInfo>,
                ),
                MarketplaceError,
            >((
                outcome
                    .marketplaces
                    .into_iter()
                    .filter_map(|marketplace| {
                        let plugins = marketplace
                            .plugins
                            .into_iter()
                            .filter(|plugin| {
                                plugin.installed
                                    || install_suggestion_plugin_names.contains(&plugin.name)
                            })
                            .map(|plugin| {
                                convert_configured_marketplace_plugin_to_plugin_summary(
                                    plugin,
                                    &shared_plugin_ids_by_local_path,
                                )
                            })
                            .collect::<Vec<_>>();

                        (!plugins.is_empty()).then_some(PluginMarketplaceEntry {
                            name: marketplace.name,
                            path: Some(marketplace.path),
                            interface: marketplace.interface.map(|interface| {
                                MarketplaceInterface {
                                    display_name: interface.display_name,
                                }
                            }),
                            plugins,
                        })
                    })
                    .collect(),
                outcome
                    .errors
                    .into_iter()
                    .map(|err| ody_app_server_protocol::MarketplaceLoadErrorInfo {
                        marketplace_path: err.path,
                        message: err.message,
                    })
                    .collect(),
            ))
        })
        .await
        {
            Ok(Ok(outcome)) => Ok(outcome),
            Ok(Err(err)) => Err(Self::marketplace_error(
                err,
                "list installed and suggested marketplace plugins",
            )),
            Err(err) => Err(internal_error(format!(
                "failed to list installed and suggested plugins: {err}"
            ))),
        }
    }

    async fn plugin_read_response(
        &self,
        params: PluginReadParams,
    ) -> Result<PluginReadResponse, JSONRPCErrorError> {
        let plugins_manager = self.thread_manager.plugins_manager();
        let PluginReadParams {
            marketplace_path,
            remote_marketplace_name,
            plugin_name,
        } = params;
        let read_source = match (marketplace_path, remote_marketplace_name) {
            (Some(marketplace_path), None) => Ok(marketplace_path),
            (None, Some(remote_marketplace_name)) => Err(remote_marketplace_name),
            (Some(_), Some(_)) | (None, None) => {
                return Err(invalid_request(
                    "plugin/read requires exactly one of marketplacePath or remoteMarketplaceName",
                ));
            }
        };
        let config_cwd = read_source.as_ref().ok().and_then(|marketplace_path| {
            marketplace_path.as_path().parent().map(Path::to_path_buf)
        });

        let config = self.load_latest_config(config_cwd).await?;
        let plugins_input = config.plugins_config_input();
        let auth = self.auth_manager.auth().await;
        plugins_manager.set_auth_mode(auth.as_ref().map(OdyAuth::api_auth_mode));

        let plugin = match read_source {
            Ok(marketplace_path) => {
                let request = PluginReadRequest {
                    plugin_name,
                    marketplace_path,
                };
                let outcome = plugins_manager
                    .read_plugin_for_config(&plugins_input, &request)
                    .await
                    .map_err(|err| Self::marketplace_error(err, "read plugin details"))?;
                let shared_plugin_ids_by_local_path =
                    load_shared_plugin_ids_by_local_path(&config)?;
                // This used to be hydrated further with remote share metadata (principals,
                // remote version) fetched from the removed hosted plugin-share service; now the
                // locally-derived share mapping context is used as-is.
                let share_context = share_context_for_source(
                    &outcome.plugin.source,
                    &shared_plugin_ids_by_local_path,
                );
                let app_summaries = load_plugin_app_summaries(
                    &config,
                    &outcome.plugin.apps,
                    &outcome.plugin.app_category_by_id,
                )
                .await;
                let visible_skills = outcome
                    .plugin
                    .skills
                    .iter()
                    .filter(|skill| {
                        skill.matches_product_restriction_for_product(
                            self.thread_manager.session_source().restriction_product(),
                        )
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                PluginDetail {
                    marketplace_name: outcome.marketplace_name,
                    marketplace_path: outcome.marketplace_path,
                    summary: PluginSummary {
                        id: outcome.plugin.id,
                        remote_plugin_id: None,
                        local_version: outcome.plugin.local_version,
                        name: outcome.plugin.name,
                        share_context,
                        source: marketplace_plugin_source_to_info(outcome.plugin.source),
                        installed: outcome.plugin.installed,
                        enabled: outcome.plugin.enabled,
                        install_policy: outcome.plugin.policy.installation.into(),
                        auth_policy: outcome.plugin.policy.authentication.into(),
                        availability: PluginAvailability::Available,
                        interface: outcome.plugin.interface.map(local_plugin_interface_to_info),
                        keywords: outcome.plugin.keywords,
                    },
                    share_url: None,
                    description: outcome.plugin.description,
                    skills: plugin_skills_to_info(
                        &visible_skills,
                        &outcome.plugin.disabled_skill_paths,
                    ),
                    hooks: outcome
                        .plugin
                        .hooks
                        .into_iter()
                        .map(|hook| ody_app_server_protocol::PluginHookSummary {
                            key: hook.key,
                            event_name: hook.event_name.into(),
                        })
                        .collect(),
                    apps: app_summaries,
                    app_templates: Vec::new(),
                    mcp_servers: outcome.plugin.mcp_server_names,
                }
            }
            Err(_remote_marketplace_name) => {
                // The remote hosted plugin catalog has been removed; plugin/read no longer
                // supports reading plugin details by remote marketplace name.
                return Err(invalid_request(
                    "plugin/read by remoteMarketplaceName is no longer supported",
                ));
            }
        };

        Ok(PluginReadResponse { plugin })
    }

    async fn plugin_skill_read_response(
        &self,
        _params: PluginSkillReadParams,
    ) -> Result<PluginSkillReadResponse, JSONRPCErrorError> {
        // This only ever read a skill belonging to a plugin from the remote hosted plugin
        // catalog, which has been removed.
        Err(invalid_request(
            "plugin/skill/read is no longer supported",
        ))
    }

    async fn plugin_share_save_response(
        &self,
        _params: PluginShareSaveParams,
    ) -> Result<PluginShareSaveResponse, JSONRPCErrorError> {
        // Plugin sharing was implemented entirely via the remote hosted plugin catalog, which
        // has been removed.
        Err(invalid_request("plugin/share/save is no longer supported"))
    }

    async fn plugin_share_update_targets_response(
        &self,
        _params: PluginShareUpdateTargetsParams,
    ) -> Result<PluginShareUpdateTargetsResponse, JSONRPCErrorError> {
        Err(invalid_request(
            "plugin/share/updateTargets is no longer supported",
        ))
    }

    async fn plugin_share_list_response(
        &self,
        _params: PluginShareListParams,
    ) -> Result<PluginShareListResponse, JSONRPCErrorError> {
        Err(invalid_request("plugin/share/list is no longer supported"))
    }

    async fn plugin_share_checkout_response(
        &self,
        _params: PluginShareCheckoutParams,
    ) -> Result<PluginShareCheckoutResponse, JSONRPCErrorError> {
        Err(invalid_request(
            "plugin/share/checkout is no longer supported",
        ))
    }

    async fn plugin_share_delete_response(
        &self,
        _params: PluginShareDeleteParams,
    ) -> Result<PluginShareDeleteResponse, JSONRPCErrorError> {
        Err(invalid_request(
            "plugin/share/delete is no longer supported",
        ))
    }


    async fn plugin_install_response(
        &self,
        params: PluginInstallParams,
    ) -> Result<PluginInstallResponse, JSONRPCErrorError> {
        let PluginInstallParams {
            marketplace_path,
            remote_marketplace_name,
            plugin_name,
        } = params;
        let marketplace_path = match (marketplace_path, remote_marketplace_name) {
            (Some(marketplace_path), None) => marketplace_path,
            (None, Some(remote_marketplace_name)) => {
                return self
                    .remote_plugin_install_response(remote_marketplace_name, plugin_name)
                    .await;
            }
            (Some(_), Some(_)) | (None, None) => {
                return Err(invalid_request(
                    "plugin/install requires exactly one of marketplacePath or remoteMarketplaceName",
                ));
            }
        };
        let config_cwd = marketplace_path.as_path().parent().map(Path::to_path_buf);
        let config = self.load_latest_config(config_cwd.clone()).await?;
        let auth = self.auth_manager.auth().await;

        if !self
            .workspace_ody_plugins_enabled(&config, auth.as_ref())
            .await
        {
            return Err(invalid_request(
                "Ody plugins are disabled for this workspace",
            ));
        }

        let plugins_manager = self.thread_manager.plugins_manager();
        let marketplace_display = marketplace_path.display().to_string();
        let plugin_name_for_log = plugin_name.clone();
        let request = PluginInstallRequest {
            plugin_name,
            marketplace_path,
        };

        let result = match plugins_manager.install_plugin(request).await {
            Ok(result) => result,
            Err(err) => {
                warn!(
                    marketplace = %marketplace_display,
                    plugin_name = %plugin_name_for_log,
                    "failed to install plugin: {err}"
                );
                return Err(Self::plugin_install_error(err));
            }
        };
        let config = match self.load_latest_config(config_cwd).await {
            Ok(config) => config,
            Err(err) => {
                warn!(
                    "failed to reload config after plugin install, using current config: {err:?}"
                );
                config
            }
        };

        self.on_effective_plugins_changed();

        let plugin_mcp_servers = load_plugin_mcp_servers(
            result.installed_path.as_path(),
            auth.as_ref().map(OdyAuth::auth_mode),
        )
        .await;
        if !plugin_mcp_servers.is_empty() {
            self.start_plugin_mcp_oauth_logins(&config, plugin_mcp_servers)
                .await;
        }

        let plugin_app_declarations = load_plugin_apps(result.installed_path.as_path()).await;
        let plugin_apps =
            ody_plugin::app_connector_ids_from_declarations(&plugin_app_declarations);
        let apps_needing_auth = self
            .plugin_apps_needing_auth_for_install(
                &config,
                auth.as_ref().is_some_and(OdyAuth::is_api_key_auth),
                &result.plugin_id.as_key(),
                &plugin_apps,
            )
            .await;

        Ok(PluginInstallResponse {
            auth_policy: result.auth_policy.into(),
            apps_needing_auth,
        })
    }

    async fn remote_plugin_install_response(
        &self,
        _remote_marketplace_name: String,
        _remote_plugin_id: String,
    ) -> Result<PluginInstallResponse, JSONRPCErrorError> {
        // Installing a plugin by remote marketplace name/id required the remote hosted plugin
        // catalog, which has been removed. Local marketplace-path installs are unaffected; see
        // `plugin_install_response`.
        Err(invalid_request(
            "plugin/install by remoteMarketplaceName is no longer supported",
        ))
    }


    async fn plugin_apps_needing_auth_for_install(
        &self,
        config: &Config,
        is_api_key_auth: bool,
        plugin_id: &str,
        plugin_apps: &[ody_plugin::AppConnectorId],
    ) -> Vec<AppSummary> {
        if plugin_apps.is_empty() || !config.features.apps_enabled_for_auth(is_api_key_auth) {
            return Vec::new();
        }

        let environment_manager = self.thread_manager.environment_manager();
        let (all_connectors_result, accessible_connectors_result) = tokio::join!(
            connectors::list_all_connectors_with_options(config, /*force_refetch*/ false, &[]),
            connectors::list_accessible_connectors_from_mcp_tools_with_mcp_manager(
                config,
                /*force_refetch*/ true,
                Arc::clone(&environment_manager),
                self.thread_manager.mcp_manager(),
            ),
        );

        let all_connectors = match all_connectors_result {
            Ok(connectors) => connectors,
            Err(err) => {
                warn!(
                    plugin = plugin_id,
                    "failed to load app metadata after plugin install: {err:#}"
                );
                connectors::list_cached_all_connectors(config, &[])
                    .await
                    .unwrap_or_default()
            }
        };
        let all_connectors = connectors::connectors_for_plugin_apps(all_connectors, plugin_apps);
        let (accessible_connectors, ody_apps_ready) = match accessible_connectors_result {
            Ok(status) => (status.connectors, status.ody_apps_ready),
            Err(err) => {
                warn!(
                    plugin = plugin_id,
                    "failed to load accessible apps after plugin install: {err:#}"
                );
                (
                    connectors::list_cached_accessible_connectors_from_mcp_tools(config)
                        .await
                        .unwrap_or_default(),
                    false,
                )
            }
        };
        if !ody_apps_ready {
            warn!(
                plugin = plugin_id,
                "ody_apps MCP not ready after plugin install; skipping appsNeedingAuth check"
            );
        }

        plugin_apps_needing_auth(
            &all_connectors,
            &accessible_connectors,
            plugin_apps,
            ody_apps_ready,
        )
    }

    async fn start_plugin_mcp_oauth_logins(
        &self,
        config: &Config,
        plugin_mcp_servers: HashMap<String, McpServerConfig>,
    ) {
        for (name, server) in plugin_mcp_servers {
            let oauth_config = match oauth_login_support(&server.transport).await {
                McpOAuthLoginSupport::Supported(config) => config,
                McpOAuthLoginSupport::Unsupported => continue,
                McpOAuthLoginSupport::Unknown(err) => {
                    warn!(
                        "MCP server may or may not require login for plugin install {name}: {err}"
                    );
                    continue;
                }
            };

            let resolved_scopes = resolve_oauth_scopes(
                /*explicit_scopes*/ None,
                server.scopes.clone(),
                oauth_config.discovered_scopes.clone(),
            );

            let store_mode = config.mcp_oauth_credentials_store_mode;
            let keyring_backend_kind = config.auth_keyring_backend_kind();
            let callback_port = config.mcp_oauth_callback_port;
            let callback_url = config.mcp_oauth_callback_url.clone();
            let outgoing = Arc::clone(&self.outgoing);
            let notification_name = name.clone();

            tokio::spawn(async move {
                let oauth_client_id = server.oauth_client_id();
                let first_attempt = perform_oauth_login_silent(
                    &name,
                    &oauth_config.url,
                    store_mode,
                    keyring_backend_kind,
                    oauth_config.http_headers.clone(),
                    oauth_config.env_http_headers.clone(),
                    &resolved_scopes.scopes,
                    oauth_client_id,
                    server.oauth_resource.as_deref(),
                    callback_port,
                    callback_url.as_deref(),
                )
                .await;

                let final_result = match first_attempt {
                    Err(err) if should_retry_without_scopes(&resolved_scopes, &err) => {
                        perform_oauth_login_silent(
                            &name,
                            &oauth_config.url,
                            store_mode,
                            keyring_backend_kind,
                            oauth_config.http_headers,
                            oauth_config.env_http_headers,
                            &[],
                            oauth_client_id,
                            server.oauth_resource.as_deref(),
                            callback_port,
                            callback_url.as_deref(),
                        )
                        .await
                    }
                    result => result,
                };

                let (success, error) = match final_result {
                    Ok(()) => (true, None),
                    Err(err) => (false, Some(err.to_string())),
                };

                let notification = ServerNotification::McpServerOauthLoginCompleted(
                    McpServerOauthLoginCompletedNotification {
                        name: notification_name,
                        success,
                        error,
                    },
                );
                outgoing.send_server_notification(notification).await;
            });
        }
    }

    async fn plugin_uninstall_response(
        &self,
        params: PluginUninstallParams,
    ) -> Result<PluginUninstallResponse, JSONRPCErrorError> {
        let PluginUninstallParams { plugin_id } = params;
        if ody_plugin::PluginId::parse(&plugin_id).is_err()
            && !is_valid_remote_plugin_id(&plugin_id)
        {
            return Err(invalid_request("invalid remote plugin id"));
        }
        if is_valid_remote_plugin_id(&plugin_id) {
            return self.remote_plugin_uninstall_response(plugin_id).await;
        }
        let plugins_manager = self.thread_manager.plugins_manager();

        plugins_manager
            .uninstall_plugin(plugin_id)
            .await
            .map_err(Self::plugin_uninstall_error)?;
        match self.load_latest_config(/*fallback_cwd*/ None).await {
            Ok(_) => self.on_effective_plugins_changed(),
            Err(err) => {
                warn!(
                    "failed to reload config after plugin uninstall, clearing plugin-related caches only: {err:?}"
                );
                self.clear_plugin_related_caches();
            }
        }
        Ok(PluginUninstallResponse {})
    }

    fn plugin_install_error(err: CorePluginInstallError) -> JSONRPCErrorError {
        if err.is_invalid_request() {
            return invalid_request(err.to_string());
        }

        match err {
            CorePluginInstallError::Marketplace(err) => {
                Self::marketplace_error(err, "install plugin")
            }
            CorePluginInstallError::Config(err) => {
                internal_error(format!("failed to persist installed plugin config: {err}"))
            }
            CorePluginInstallError::Join(err) => {
                internal_error(format!("failed to install plugin: {err}"))
            }
            CorePluginInstallError::Store(err) => {
                internal_error(format!("failed to install plugin: {err}"))
            }
        }
    }

    fn plugin_uninstall_error(err: CorePluginUninstallError) -> JSONRPCErrorError {
        if err.is_invalid_request() {
            return invalid_request(err.to_string());
        }

        match err {
            CorePluginUninstallError::Config(err) => {
                internal_error(format!("failed to clear plugin config: {err}"))
            }
            CorePluginUninstallError::Join(err) => {
                internal_error(format!("failed to uninstall plugin: {err}"))
            }
            CorePluginUninstallError::Store(err) => {
                internal_error(format!("failed to uninstall plugin: {err}"))
            }
            CorePluginUninstallError::InvalidPluginId(_) => {
                unreachable!("invalid plugin ids are handled above");
            }
        }
    }

    fn marketplace_error(err: MarketplaceError, action: &str) -> JSONRPCErrorError {
        match err {
            MarketplaceError::MarketplaceNotFound { .. }
            | MarketplaceError::InvalidMarketplaceFile { .. }
            | MarketplaceError::PluginNotFound { .. }
            | MarketplaceError::PluginNotAvailable { .. }
            | MarketplaceError::PluginsDisabled
            | MarketplaceError::InvalidPlugin(_) => invalid_request(err.to_string()),
            MarketplaceError::Io { .. } => internal_error(format!("failed to {action}: {err}")),
        }
    }

    async fn remote_plugin_uninstall_response(
        &self,
        _plugin_id: String,
    ) -> Result<PluginUninstallResponse, JSONRPCErrorError> {
        // Uninstalling a plugin by remote plugin id required the remote hosted plugin catalog,
        // which has been removed. Local marketplace-path uninstalls are unaffected; see
        // `plugin_uninstall_response`.
        Err(invalid_request(
            "plugin/uninstall by remote plugin id is no longer supported",
        ))
    }
}

async fn load_plugin_app_summaries(
    config: &Config,
    plugin_apps: &[ody_plugin::AppConnectorId],
    app_category_by_id: &HashMap<String, String>,
) -> Vec<AppSummary> {
    if plugin_apps.is_empty() {
        return Vec::new();
    }

    let connectors = match connectors::list_all_connectors_with_options(
        config,
        /*force_refetch*/ false,
        &[],
    )
    .await
    {
        Ok(connectors) => connectors,
        Err(err) => {
            warn!("failed to load app metadata for plugin/read: {err:#}");
            connectors::list_cached_all_connectors(config, &[])
                .await
                .unwrap_or_default()
        }
    };

    let plugin_connectors = connectors::connectors_for_plugin_apps(connectors, plugin_apps);

    plugin_connectors
        .into_iter()
        .map(|connector| {
            let category = app_category_by_id
                .get(&connector.id)
                .cloned()
                .or_else(|| connector.category());
            AppSummary {
                id: connector.id,
                name: connector.name,
                description: connector.description,
                install_url: connector.install_url,
                category,
            }
        })
        .collect()
}

fn plugin_apps_needing_auth(
    all_connectors: &[AppInfo],
    accessible_connectors: &[AppInfo],
    plugin_apps: &[ody_plugin::AppConnectorId],
    ody_apps_ready: bool,
) -> Vec<AppSummary> {
    if !ody_apps_ready {
        return Vec::new();
    }

    let accessible_ids = accessible_connectors
        .iter()
        .map(|connector| connector.id.as_str())
        .collect::<HashSet<_>>();
    let plugin_app_ids = plugin_apps
        .iter()
        .map(|connector_id| connector_id.0.as_str())
        .collect::<HashSet<_>>();

    all_connectors
        .iter()
        .filter(|connector| {
            plugin_app_ids.contains(connector.id.as_str())
                && !accessible_ids.contains(connector.id.as_str())
        })
        .cloned()
        .map(|connector| {
            let category = connector.category();
            AppSummary {
                category,
                id: connector.id,
                name: connector.name,
                description: connector.description,
                install_url: connector.install_url,
            }
        })
        .collect()
}

