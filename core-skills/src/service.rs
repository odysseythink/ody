use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::RwLock;

use ody_config::ConfigLayerStack;
use ody_exec_server::ExecutorFileSystem;
use ody_protocol::config_types::ModeKind;
use ody_protocol::protocol::Product;
use ody_protocol::protocol::SkillScope;
use ody_utils_absolute_path::AbsolutePathBuf;
use ody_utils_plugins::PluginSkillRoot;
use tracing::info;
use tracing::instrument;
use tracing::warn;

use crate::HostSkillsSnapshot;
use crate::PluginSkillSnapshots;
use crate::SkillLoadOutcome;
use crate::build_implicit_skill_path_indexes;
use crate::config_rules::SkillConfigRules;
use crate::config_rules::resolve_disabled_skill_paths;
use crate::config_rules::skill_config_rules_from_stack;
use crate::loader::SkillRoot;
use crate::loader::load_skills_from_roots;
use crate::loader::skill_roots;
use crate::system::install_system_skills;
use crate::system::uninstall_system_skills;
use ody_config::SkillsConfig;

#[derive(Debug, Clone)]
pub struct SkillsLoadInput {
    pub cwd: AbsolutePathBuf,
    pub effective_skill_roots: Vec<PluginSkillRoot>,
    pub config_layer_stack: ConfigLayerStack,
    pub bundled_skills_enabled: bool,
    /// The collaboration mode the catalog is being built for, when the caller
    /// knows it. `Game` narrows away the bundled game skills (they are only
    /// listed in that mode) and `Frontend` narrows the catalog down to the
    /// frontend craft bundle; every other mode leaves the catalog untouched.
    /// `None` means "no mode context" — the full catalog, which is what the
    /// skill-browser and watcher paths want.
    pub collaboration_mode: Option<ModeKind>,
    plugin_skill_snapshots: Option<PluginSkillSnapshots>,
}

impl SkillsLoadInput {
    pub fn new(
        cwd: AbsolutePathBuf,
        effective_skill_roots: Vec<PluginSkillRoot>,
        config_layer_stack: ConfigLayerStack,
        bundled_skills_enabled: bool,
    ) -> Self {
        Self {
            cwd,
            effective_skill_roots,
            config_layer_stack,
            bundled_skills_enabled,
            collaboration_mode: None,
            plugin_skill_snapshots: None,
        }
    }

    /// Scopes the load to a collaboration mode's skill visibility.
    pub fn with_collaboration_mode(mut self, mode: ModeKind) -> Self {
        self.collaboration_mode = Some(mode);
        self
    }

    /// Attaches plugin skill snapshots parsed during plugin loading, when available.
    pub fn with_plugin_skill_snapshots(
        mut self,
        plugin_skill_snapshots: Option<PluginSkillSnapshots>,
    ) -> Self {
        self.plugin_skill_snapshots = plugin_skill_snapshots;
        self
    }
}

/// Owns host skill discovery, immutable snapshots, cache invalidation, and extra roots.
///
/// Source-specific model exposure remains the responsibility of the skills extension.
pub struct SkillsService {
    ody_home: AbsolutePathBuf,
    restriction_product: Option<Product>,
    extra_roots: RwLock<Vec<AbsolutePathBuf>>,
    cache_by_cwd: RwLock<HashMap<CwdSkillsCacheKey, HostSkillsSnapshot>>,
    cache_by_config: RwLock<HashMap<ConfigSkillsCacheKey, HostSkillsSnapshot>>,
}

impl SkillsService {
    pub fn new(ody_home: AbsolutePathBuf, bundled_skills_enabled: bool) -> Self {
        Self::new_with_restriction_product(ody_home, bundled_skills_enabled, Some(Product::Ody))
    }

    pub fn new_with_restriction_product(
        ody_home: AbsolutePathBuf,
        bundled_skills_enabled: bool,
        restriction_product: Option<Product>,
    ) -> Self {
        let service = Self {
            ody_home,
            restriction_product,
            extra_roots: RwLock::new(Vec::new()),
            cache_by_cwd: RwLock::new(HashMap::new()),
            cache_by_config: RwLock::new(HashMap::new()),
        };
        if !bundled_skills_enabled {
            // The loader caches bundled skills under `skills/.system`. Clearing that directory is
            // best-effort cleanup; root selection still enforces the config even if removal fails.
            uninstall_system_skills(&service.ody_home);
        } else if let Err(err) = install_system_skills(&service.ody_home) {
            tracing::error!("failed to install system skills: {err}");
        }
        service
    }

    pub fn set_extra_roots(&self, extra_roots: Vec<AbsolutePathBuf>) {
        {
            let mut roots = self
                .extra_roots
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *roots = extra_roots;
        }
        self.clear_cache();
    }

    /// Load skills for an already-constructed [`Config`], avoiding any additional config-layer
    /// loading.
    ///
    /// This path uses a cache keyed by the effective skill-relevant config state rather than just
    /// cwd so role-local and session-local skill overrides cannot bleed across sessions that happen
    /// to share a directory.
    #[instrument(
        name = "skills_for_config",
        level = "info",
        skip_all,
        fields(otel.name = "skills_for_config")
    )]
    pub async fn snapshot_for_config(
        &self,
        input: &SkillsLoadInput,
        fs: Option<Arc<dyn ExecutorFileSystem>>,
    ) -> HostSkillsSnapshot {
        let roots = self.skill_roots_for_config(input, fs).await;
        let skill_config_rules = skill_config_rules_from_stack(&input.config_layer_stack);
        let cache_key =
            config_skills_cache_key(&roots, &skill_config_rules, input.collaboration_mode);
        if let Some(snapshot) = self.cached_snapshot_for_config(&cache_key) {
            return snapshot;
        }

        let snapshot = HostSkillsSnapshot::new(Arc::new(
            self.build_skill_outcome(input, roots, &skill_config_rules)
                .await,
        ));
        let mut cache = self
            .cache_by_config
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache.insert(cache_key, snapshot.clone());
        snapshot
    }

    pub async fn skill_roots_for_config(
        &self,
        input: &SkillsLoadInput,
        fs: Option<Arc<dyn ExecutorFileSystem>>,
    ) -> Vec<SkillRoot> {
        let mut roots = skill_roots(
            fs,
            &input.config_layer_stack,
            &input.cwd,
            input.effective_skill_roots.clone(),
            self.extra_roots(),
        )
        .await;
        // Frontend mode takes its skills from the bundled system root, so
        // honouring `bundled.enabled = false` while the mode is selectable
        // would silently produce an empty catalog. The explicit opt-in wins.
        if !input.bundled_skills_enabled
            && !frontend_mode_enabled_from_stack(&input.config_layer_stack)
        {
            roots.retain(|root| root.scope != SkillScope::System);
        }
        roots
    }

    pub async fn snapshot_for_cwd(
        &self,
        input: &SkillsLoadInput,
        force_reload: bool,
        fs: Option<Arc<dyn ExecutorFileSystem>>,
    ) -> HostSkillsSnapshot {
        let use_cwd_cache = fs.is_some();
        if use_cwd_cache
            && !force_reload
            && let Some(snapshot) =
                self.cached_snapshot_for_cwd(&input.cwd, input.collaboration_mode)
        {
            return snapshot;
        }

        let mut roots = skill_roots(
            fs.clone(),
            &input.config_layer_stack,
            &input.cwd,
            input.effective_skill_roots.clone(),
            self.extra_roots(),
        )
        .await;
        if !bundled_skills_enabled_from_stack(&input.config_layer_stack)
            && !frontend_mode_enabled_from_stack(&input.config_layer_stack)
        {
            roots.retain(|root| root.scope != SkillScope::System);
        }
        let skill_config_rules = skill_config_rules_from_stack(&input.config_layer_stack);
        let snapshot = HostSkillsSnapshot::new(Arc::new(
            self.build_skill_outcome(input, roots, &skill_config_rules)
                .await,
        ));
        if use_cwd_cache {
            let mut cache = self
                .cache_by_cwd
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            cache.insert(
                CwdSkillsCacheKey {
                    cwd: input.cwd.clone(),
                    collaboration_mode: input.collaboration_mode,
                },
                snapshot.clone(),
            );
        }
        snapshot
    }

    #[instrument(level = "trace", skip_all)]
    async fn build_skill_outcome(
        &self,
        input: &SkillsLoadInput,
        roots: Vec<SkillRoot>,
        skill_config_rules: &SkillConfigRules,
    ) -> SkillLoadOutcome {
        let outcome = load_skills_from_roots(roots, input.plugin_skill_snapshots.as_ref()).await;
        let outcome =
            crate::filter_skill_load_outcome_for_product(outcome, self.restriction_product);
        let outcome = crate::filter_skill_load_outcome_for_game(
            outcome,
            input.collaboration_mode == Some(ModeKind::Game),
        );
        let frontend_mode_active = input.collaboration_mode == Some(ModeKind::Frontend);
        let outcome = crate::filter_skill_load_outcome_for_frontend(outcome, frontend_mode_active);
        if frontend_mode_active {
            info!(
                skills = outcome.skills.len(),
                visible = ?ody_skills::embedded_frontend_skill_names(),
                "frontend mode: skill catalog restricted to the bundled frontend craft skills"
            );
        }
        let disabled_paths = resolve_disabled_skill_paths(&outcome.skills, skill_config_rules);
        finalize_skill_outcome(outcome, disabled_paths)
    }

    pub fn clear_cache(&self) {
        let cleared_cwd = {
            let mut cache = self
                .cache_by_cwd
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let cleared = cache.len();
            cache.clear();
            cleared
        };
        let cleared_config = {
            let mut cache = self
                .cache_by_config
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let cleared = cache.len();
            cache.clear();
            cleared
        };
        let cleared = cleared_cwd + cleared_config;
        info!("skills cache cleared ({cleared} entries)");
    }

    fn cached_snapshot_for_cwd(
        &self,
        cwd: &AbsolutePathBuf,
        collaboration_mode: Option<ModeKind>,
    ) -> Option<HostSkillsSnapshot> {
        let key = CwdSkillsCacheKey {
            cwd: cwd.clone(),
            collaboration_mode,
        };
        match self.cache_by_cwd.read() {
            Ok(cache) => cache.get(&key).cloned(),
            Err(err) => err.into_inner().get(&key).cloned(),
        }
    }

    fn cached_snapshot_for_config(
        &self,
        cache_key: &ConfigSkillsCacheKey,
    ) -> Option<HostSkillsSnapshot> {
        match self.cache_by_config.read() {
            Ok(cache) => cache.get(cache_key).cloned(),
            Err(err) => err.into_inner().get(cache_key).cloned(),
        }
    }

    fn extra_roots(&self) -> Vec<AbsolutePathBuf> {
        match self.extra_roots.read() {
            Ok(roots) => roots.clone(),
            Err(err) => err.into_inner().clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ConfigSkillsCacheKey {
    roots: Vec<(AbsolutePathBuf, u8, Option<String>, Option<String>)>,
    skill_config_rules: SkillConfigRules,
    /// Opt-in modes narrow the catalog, so the active mode belongs in the key:
    /// two sessions in the same directory must not share a bundle-only catalog.
    collaboration_mode: Option<ModeKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CwdSkillsCacheKey {
    cwd: AbsolutePathBuf,
    collaboration_mode: Option<ModeKind>,
}

pub fn bundled_skills_enabled_from_stack(
    config_layer_stack: &ody_config::ConfigLayerStack,
) -> bool {
    let effective_config = config_layer_stack.effective_config();
    let Some(skills_value) = effective_config
        .as_table()
        .and_then(|table| table.get("skills"))
    else {
        return true;
    };

    let skills: SkillsConfig = match skills_value.clone().try_into() {
        Ok(skills) => skills,
        Err(err) => {
            warn!("invalid skills config: {err}");
            return true;
        }
    };

    skills.bundled.unwrap_or_default().enabled
}

/// Returns whether the game-development *mode* is available in this
/// invocation. Defaults to `false`; `ody --game` layers
/// `skills.bundled.game = true` onto the config stack.
///
/// This is availability, not visibility: the bundled game skills are listed
/// only while [`ModeKind::Game`] is the active collaboration mode — see
/// [`SkillsLoadInput::collaboration_mode`]. Before Game became a mode this flag
/// doubled as the filter, which meant a `--game` session could never leave the
/// game catalog.
pub fn game_skills_enabled_from_stack(config_layer_stack: &ody_config::ConfigLayerStack) -> bool {
    let effective_config = config_layer_stack.effective_config();
    let Some(skills_value) = effective_config
        .as_table()
        .and_then(|table| table.get("skills"))
    else {
        return false;
    };

    let skills: SkillsConfig = match skills_value.clone().try_into() {
        Ok(skills) => skills,
        Err(err) => {
            warn!("invalid skills config: {err}");
            return false;
        }
    };

    skills
        .bundled
        .and_then(|bundled| bundled.game)
        .unwrap_or_default()
}

/// Returns whether the frontend *mode* is available in this invocation.
/// Defaults to `false`; `ody --frontend` layers
/// `skills.bundled.frontend = true` onto the config stack.
///
/// Like the game flag this is availability: the catalog is narrowed only while
/// [`ModeKind::Frontend`] is active, so leaving the mode restores every skill.
///
/// Fails safe: a malformed `skills` table leaves the mode unavailable rather
/// than hiding everything the user installed.
pub fn frontend_mode_enabled_from_stack(config_layer_stack: &ody_config::ConfigLayerStack) -> bool {
    let effective_config = config_layer_stack.effective_config();
    let Some(skills_value) = effective_config
        .as_table()
        .and_then(|table| table.get("skills"))
    else {
        return false;
    };

    let skills: SkillsConfig = match skills_value.clone().try_into() {
        Ok(skills) => skills,
        Err(err) => {
            warn!("invalid skills config: {err}");
            return false;
        }
    };

    skills
        .bundled
        .and_then(|bundled| bundled.frontend)
        .unwrap_or_default()
}

fn config_skills_cache_key(
    roots: &[SkillRoot],
    skill_config_rules: &SkillConfigRules,
    collaboration_mode: Option<ModeKind>,
) -> ConfigSkillsCacheKey {
    ConfigSkillsCacheKey {
        roots: roots
            .iter()
            .map(|root| {
                let scope_rank = match root.scope {
                    SkillScope::Repo => 0,
                    SkillScope::User => 1,
                    SkillScope::System => 2,
                    SkillScope::Admin => 3,
                };
                (
                    root.path.clone(),
                    scope_rank,
                    root.plugin_id.clone(),
                    root.plugin_namespace.clone(),
                )
            })
            .collect(),
        skill_config_rules: skill_config_rules.clone(),
        collaboration_mode,
    }
}

fn finalize_skill_outcome(
    mut outcome: SkillLoadOutcome,
    disabled_paths: HashSet<AbsolutePathBuf>,
) -> SkillLoadOutcome {
    outcome.disabled_paths = disabled_paths;
    let (by_scripts_dir, by_doc_path) =
        build_implicit_skill_path_indexes(outcome.allowed_skills_for_implicit_invocation());
    outcome.implicit_skills_by_scripts_dir = Arc::new(by_scripts_dir);
    outcome.implicit_skills_by_doc_path = Arc::new(by_doc_path);
    outcome
}

#[cfg(test)]
#[path = "service_tests.rs"]
mod tests;
