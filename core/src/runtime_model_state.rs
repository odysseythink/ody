//! Unified runtime model-state resolution.
//!
//! `resolve_runtime_model_state` collapses provider alias resolution, provider
//! info lookup, model manager construction, and active-model/catalog collection
//! into a single consistent snapshot. D2 (cold-start) and D3 (reload) will
//! consume this snapshot instead of repeating the resolution chain at their own
//! call sites.

use crate::config::Config;
use ody_model_provider::create_model_provider_with_id;
use ody_model_provider_info::ModelProviderInfo;
use ody_model_provider_info::model_ref::ModelRef;
use ody_model_provider_info::resolve_provider_info;
use ody_models_manager::manager::SharedModelsManager;
use ody_protocol::model_metadata::ModelsResponse;
use std::collections::HashMap;

/// Consistent runtime snapshot of model/provider state.
///
/// All fields are fully resolved from a single `Config` so downstream consumers
/// (D2 cold-start, D3 reload, and eventually session refresh) do not need to
/// re-derive any part of the chain.
#[derive(Debug, Clone)]
pub struct RuntimeModelState {
    /// Active model manager, already wired to the resolved provider.
    pub models_manager: SharedModelsManager,
    /// Provider alias that is effectively active.
    ///
    /// This is the alias used to resolve `provider_info`: the explicit alias from
    /// a qualified `active_model`, the configured `model_provider_id`, or a
    /// built-in alias selected as a fallback.
    pub provider_alias: String,
    /// Resolved provider metadata used to construct `models_manager`.
    pub provider_info: ModelProviderInfo,
    /// User-configured provider map from `config.model_providers`.
    pub providers: HashMap<String, ModelProviderInfo>,
    /// Optional authoritative catalog loaded from `model_catalog_json`.
    ///
    /// When present, `models_manager` is a `StaticModelsManager` backed by this
    /// catalog.
    pub model_catalog: Option<ModelsResponse>,
    /// Aggregated catalog built from all `[models."provider/model"]` entries.
    pub configured_model_catalog: Option<ModelsResponse>,
    /// Canonical provider alias + bare model id for the active model.
    pub active_model: Option<ModelRef>,
}

/// Resolve a consistent `RuntimeModelState` from the given `Config`.
///
/// Resolution order:
/// 1. `Config::active_model_ref()` returns the canonical `ModelRef`.
/// 2. The provider alias is taken from that reference when non-empty; otherwise
///    it falls back to `config.model_provider_id`.
/// 3. `resolve_provider_info` looks up the alias in the configured providers map
///    and then falls back to built-in `kimi`/`deepseek`/`glm` definitions.
/// 4. If the alias is still unknown, the function falls back to
///    `config.model_provider` (the provider already resolved at config load
///    time).
/// 5. A runtime provider is constructed with `create_model_provider_with_id` and
///    asked for its `models_manager`.
pub fn resolve_runtime_model_state(config: &Config) -> RuntimeModelState {
    let active_model = config.active_model_ref();

    let provider_alias = active_model
        .as_ref()
        .map(|m| {
            if m.provider_alias.is_empty() {
                config.model_provider_id.clone()
            } else {
                m.provider_alias.clone()
            }
        })
        .unwrap_or_else(|| config.model_provider_id.clone());

    let provider_info = resolve_provider_info(&provider_alias, &config.model_providers)
        .unwrap_or_else(|| config.model_provider.clone());

    let provider = create_model_provider_with_id(provider_alias.clone(), provider_info.clone());
    let models_manager =
        provider.models_manager(config.ody_home.to_path_buf(), config.model_catalog.clone());

    RuntimeModelState {
        models_manager,
        provider_alias,
        provider_info,
        providers: config.model_providers.clone(),
        model_catalog: config.model_catalog.clone(),
        configured_model_catalog: config.configured_model_catalog.clone(),
        active_model,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::test_config;
    use ody_model_provider_info::create_kimi_provider;
    use ody_models_manager::manager::RefreshStrategy;

    #[tokio::test]
    async fn no_active_model_uses_configured_provider() {
        let config = test_config().await;
        assert!(config.model.is_none(), "fixture should start with no model");

        let state = resolve_runtime_model_state(&config);

        assert!(state.active_model.is_none());
        assert_eq!(state.provider_alias, config.model_provider_id);
        assert_eq!(state.provider_info, config.model_provider);
        assert_eq!(state.providers, config.model_providers);
        assert!(
            state.model_catalog.is_some(),
            "fixture loads a model catalog"
        );
        assert!(
            !state
                .models_manager
                .raw_model_catalog(RefreshStrategy::Offline)
                .await
                .models
                .is_empty(),
            "static manager should expose the configured catalog"
        );
    }

    #[tokio::test]
    async fn configured_alias_is_resolved() {
        let mut config = test_config().await;
        let custom_alias = "custom";
        let custom_info = create_kimi_provider();
        config
            .model_providers
            .insert(custom_alias.to_string(), custom_info.clone());
        config.model = Some(format!("{custom_alias}/custom-model"));

        let state = resolve_runtime_model_state(&config);

        assert_eq!(
            state.active_model,
            Some(ModelRef::from_parts(custom_alias, "custom-model"))
        );
        assert_eq!(state.provider_alias, custom_alias);
        assert_eq!(state.provider_info, custom_info);
    }

    #[tokio::test]
    async fn built_in_fallback_when_alias_not_configured() {
        let mut config = test_config().await;
        config.model_providers.clear();
        config.model = Some("kimi/kimi-fallback-model".to_string());

        let state = resolve_runtime_model_state(&config);

        assert_eq!(
            state.active_model,
            Some(ModelRef::from_parts("kimi", "kimi-fallback-model"))
        );
        assert_eq!(state.provider_alias, "kimi");
        assert_eq!(state.provider_info, create_kimi_provider());
    }

    #[tokio::test]
    async fn bare_model_falls_back_to_config_provider_id() {
        let mut config = test_config().await;
        config.model = Some("bare-model-id".to_string());

        let state = resolve_runtime_model_state(&config);

        assert_eq!(
            state.active_model,
            Some(ModelRef::from_parts(
                &config.model_provider_id,
                "bare-model-id"
            ))
        );
        assert_eq!(state.provider_alias, config.model_provider_id);
        assert_eq!(state.provider_info, config.model_provider);
    }

    #[tokio::test]
    async fn bare_model_with_unknown_provider_falls_back_to_config_provider() {
        let mut config = test_config().await;
        config.model_providers.clear();
        config.model_provider_id = "unknown".to_string();
        // Keep config.model_provider as the fixture default so the fallback path
        // is exercised when `resolve_provider_info` returns None.
        config.model = Some("bare-model-id".to_string());

        let state = resolve_runtime_model_state(&config);

        assert_eq!(state.provider_alias, "unknown");
        assert_eq!(state.provider_info, config.model_provider);
    }

    #[tokio::test]
    async fn with_model_catalog_uses_static_manager() {
        let mut config = test_config().await;
        config.model = Some("test/static-model".to_string());
        // model_catalog is already present in the fixture.

        let state = resolve_runtime_model_state(&config);

        let catalog = state
            .models_manager
            .raw_model_catalog(RefreshStrategy::Offline)
            .await;
        assert!(
            !catalog.models.is_empty(),
            "static manager should return the configured catalog models"
        );
        // The fixture catalog is shared across tests; only assert shape, not ids.
        assert!(state.model_catalog.is_some());
    }

    #[tokio::test]
    async fn without_model_catalog_uses_openai_compatible_manager() {
        let mut config = test_config().await;
        config.model_catalog = None;
        config.model = Some("test/openai-model".to_string());

        let state = resolve_runtime_model_state(&config);

        assert!(state.model_catalog.is_none());
        let catalog = state
            .models_manager
            .raw_model_catalog(RefreshStrategy::Offline)
            .await;
        // The test provider has no auth, so the OpenAI-compatible manager starts
        // with an empty remote catalog rather than the bundled catalog.
        assert!(catalog.models.is_empty());
    }
}
