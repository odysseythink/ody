use std::fmt;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use ody_api::ApiError;
use ody_api::Provider;
use ody_api::SharedAuthProvider;
use ody_login::AuthManager;
use ody_login::OdyAuth;
use ody_model_provider_info::ModelProviderInfo;
use ody_models_manager::manager::OpenAiModelsManager;
use ody_models_manager::manager::SharedModelsManager;
use ody_models_manager::manager::StaticModelsManager;
use ody_models_manager::model_info::chat_provider_models;
use ody_protocol::account::ProviderAccount;
use ody_protocol::error::OdyErr;
use ody_protocol::odysseythink_models::ModelsResponse;

use crate::auth::auth_manager_for_provider;
use crate::auth::resolve_provider_auth;
use crate::models_endpoint::OpenAiModelsEndpoint;

/// Optional provider-backed features that Ody may expose at runtime.
///
/// These capabilities are a provider-owned upper bound. Callers can disable
/// more functionality through normal config, but should not expose a feature
/// that the active provider marks unsupported here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderCapabilities {
    pub namespace_tools: bool,
    pub image_generation: bool,
    pub web_search: bool,
}

impl Default for ProviderCapabilities {
    fn default() -> Self {
        Self {
            namespace_tools: true,
            image_generation: true,
            web_search: true,
        }
    }
}

/// Current app-visible account state for a model provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderAccountState {
    pub account: Option<ProviderAccount>,
    pub requires_odysseythink_auth: bool,
}

pub type ProviderAccountResult = ody_protocol::error::Result<ProviderAccountState>;

/// Default model used for automatic approval review when a provider does not
/// require a backend-specific model ID.
pub const DEFAULT_APPROVAL_REVIEW_PREFERRED_MODEL: &str = "ody-auto-review";

/// Default model used for memory extraction when a provider does not require a
/// backend-specific model ID.
pub const DEFAULT_MEMORY_EXTRACTION_PREFERRED_MODEL: &str = "gpt-5.4-mini";

/// Default model used for memory consolidation when a provider does not require
/// a backend-specific model ID.
pub const DEFAULT_MEMORY_CONSOLIDATION_PREFERRED_MODEL: &str = "gpt-5.4";

/// Runtime provider abstraction used by model execution.
///
/// Implementations own provider-specific behavior for a model backend. The
/// `ModelProviderInfo` returned by `info` is the serialized/configured provider
/// metadata used by the default OpenAI-compatible implementation.
pub trait ModelProvider: fmt::Debug + Send + Sync {
    /// Returns the configured provider metadata.
    fn info(&self) -> &ModelProviderInfo;

    /// Returns the provider-owned capability upper bounds.
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::default()
    }

    /// Returns the preferred model used for automatic approval review.
    ///
    /// Providers that require backend-specific model IDs should override this.
    fn approval_review_preferred_model(&self) -> &'static str {
        DEFAULT_APPROVAL_REVIEW_PREFERRED_MODEL
    }

    /// Returns the preferred model used for memory extraction.
    ///
    /// Providers that require backend-specific model IDs should override this.
    fn memory_extraction_preferred_model(&self) -> &'static str {
        DEFAULT_MEMORY_EXTRACTION_PREFERRED_MODEL
    }

    /// Returns the preferred model used for memory consolidation.
    ///
    /// Providers that require backend-specific model IDs should override this.
    fn memory_consolidation_preferred_model(&self) -> &'static str {
        DEFAULT_MEMORY_CONSOLIDATION_PREFERRED_MODEL
    }

    /// Returns whether requests made through this provider should include attestation.
    fn supports_attestation(&self) -> bool {
        false
    }

    /// Returns the provider-scoped auth manager, when this provider uses one.
    ///
    /// TODO(celia-oai): Make auth manager access internal to this crate so callers
    /// resolve provider-specific auth only through `ModelProvider`. We first need
    /// to think through whether Ody should have a unified provider-specific auth
    /// manager throughout the codebase; that is a larger refactor than this change.
    fn auth_manager(&self) -> Option<Arc<AuthManager>>;

    /// Returns the current provider-scoped auth value, if one is configured.
    fn auth(&self) -> ModelProviderFuture<'_, Option<OdyAuth>>;

    /// Returns the current app-visible account state for this provider.
    fn account_state(&self) -> ProviderAccountResult;

    /// Maps an API client error into the provider's user-facing error representation.
    fn map_api_error(&self, error: ApiError) -> OdyErr {
        ody_api::map_api_error(error)
    }

    /// Returns provider configuration adapted for the API client.
    fn api_provider(&self) -> ModelProviderFuture<'_, ody_protocol::error::Result<Provider>> {
        Box::pin(async move {
            let auth = self.auth().await;
            self.info()
                .to_api_provider(auth.as_ref().map(OdyAuth::auth_mode))
        })
    }

    /// Returns the provider base URL that will be used at request time.
    fn runtime_base_url(
        &self,
    ) -> ModelProviderFuture<'_, ody_protocol::error::Result<Option<String>>> {
        Box::pin(async { Ok(self.info().base_url.clone()) })
    }

    /// Returns the auth provider used to attach request credentials.
    fn api_auth(
        &self,
    ) -> ModelProviderFuture<'_, ody_protocol::error::Result<SharedAuthProvider>> {
        Box::pin(async move {
            let auth = self.auth().await;
            resolve_provider_auth(auth.as_ref(), self.info())
        })
    }

    /// Creates the model manager implementation appropriate for this provider.
    fn models_manager(
        &self,
        ody_home: PathBuf,
        config_model_catalog: Option<ModelsResponse>,
    ) -> SharedModelsManager;
}

pub type ModelProviderFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Shared runtime model provider handle.
pub type SharedModelProvider = Arc<dyn ModelProvider>;

/// Creates the default runtime model provider for configured provider metadata.
pub fn create_model_provider(
    provider_info: ModelProviderInfo,
    auth_manager: Option<Arc<AuthManager>>,
) -> SharedModelProvider {
    Arc::new(ConfiguredModelProvider::new(provider_info, auth_manager))
}

/// Runtime model provider backed by configured `ModelProviderInfo`.
#[derive(Clone, Debug)]
struct ConfiguredModelProvider {
    info: ModelProviderInfo,
    auth_manager: Option<Arc<AuthManager>>,
}

impl ConfiguredModelProvider {
    fn new(provider_info: ModelProviderInfo, auth_manager: Option<Arc<AuthManager>>) -> Self {
        let auth_manager = auth_manager_for_provider(auth_manager, &provider_info);
        Self {
            info: provider_info,
            auth_manager,
        }
    }
}

impl ModelProvider for ConfiguredModelProvider {
    fn info(&self) -> &ModelProviderInfo {
        &self.info
    }

    fn capabilities(&self) -> ProviderCapabilities {
        if self.info.is_chat_completions() {
            // Chat Completions providers expose plain function tools only;
            // namespaced tools, server-side image generation and web search are
            // Responses-API features. (Kimi's web search is handled separately
            // via its `builtin_function` tool.)
            ProviderCapabilities {
                namespace_tools: false,
                image_generation: false,
                web_search: false,
            }
        } else {
            ProviderCapabilities::default()
        }
    }

    fn auth_manager(&self) -> Option<Arc<AuthManager>> {
        self.auth_manager.clone()
    }

    fn auth(&self) -> ModelProviderFuture<'_, Option<OdyAuth>> {
        Box::pin(async move {
            match self.auth_manager.as_ref() {
                Some(auth_manager) => auth_manager.auth().await,
                None => None,
            }
        })
    }

    fn account_state(&self) -> ProviderAccountResult {
        let account = if self.info.requires_odysseythink_auth {
            self.auth_manager
                .as_ref()
                .and_then(|auth_manager| auth_manager.auth_cached())
                .map(|_| ProviderAccount::ApiKey)
        } else {
            None
        };

        Ok(ProviderAccountState {
            account,
            requires_odysseythink_auth: self.info.requires_odysseythink_auth,
        })
    }

    fn models_manager(
        &self,
        ody_home: PathBuf,
        config_model_catalog: Option<ModelsResponse>,
    ) -> SharedModelsManager {
        if let Some(model_catalog) = config_model_catalog {
            return Arc::new(StaticModelsManager::new(
                self.auth_manager.clone(),
                model_catalog,
            ));
        }

        // OpenAI-compatible Chat Completions providers (Kimi / DeepSeek / GLM)
        // do not expose the ody `/models` catalog; serve a bundled static list.
        if self.info.is_chat_completions()
            && let Some(catalog) = chat_provider_models(&provider_id_for_chat_catalog(&self.info))
        {
            return Arc::new(StaticModelsManager::new(self.auth_manager.clone(), catalog));
        }

        let endpoint = Arc::new(OpenAiModelsEndpoint::new(
            self.info.clone(),
            self.auth_manager.clone(),
        ));
        Arc::new(OpenAiModelsManager::new(
            ody_home,
            endpoint,
            self.auth_manager.clone(),
        ))
    }
}

/// Resolve the catalog key for a Chat Completions provider from its identity.
fn provider_id_for_chat_catalog(info: &ModelProviderInfo) -> String {
    if info.is_kimi() {
        "kimi".to_string()
    } else if info.is_deepseek() {
        "deepseek".to_string()
    } else if info.is_glm() {
        "glm".to_string()
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use ody_model_provider_info::WireApi;
    use pretty_assertions::assert_eq;

    use super::*;

    fn provider_for(base_url: String) -> ModelProviderInfo {
        ModelProviderInfo {
            name: "mock".into(),
            base_url: Some(base_url),
            env_key: None,
            env_key_instructions: None,
            experimental_bearer_token: None,
            auth: None,
            wire_api: WireApi::Responses,
            query_params: None,
            http_headers: None,
            env_http_headers: None,
            request_max_retries: Some(0),
            stream_max_retries: Some(0),
            stream_idle_timeout_ms: Some(5_000),
            websocket_connect_timeout_ms: None,
            requires_odysseythink_auth: false,
            supports_websockets: false,
        }
    }

    #[test]
    fn configured_provider_uses_default_capabilities() {
        let provider = create_model_provider(
            ModelProviderInfo::create_odysseythink_provider(/*base_url*/ None),
            /*auth_manager*/ None,
        );

        assert_eq!(provider.capabilities(), ProviderCapabilities::default());
    }

    #[test]
    fn configured_provider_uses_default_approval_review_preferred_model() {
        let provider = create_model_provider(
            ModelProviderInfo::create_odysseythink_provider(/*base_url*/ None),
            /*auth_manager*/ None,
        );

        assert_eq!(
            provider.approval_review_preferred_model(),
            DEFAULT_APPROVAL_REVIEW_PREFERRED_MODEL
        );
    }

    #[tokio::test]
    async fn configured_provider_runtime_base_url_uses_configured_base_url() {
        let provider = create_model_provider(
            provider_for("https://example.test/v1".to_string()),
            /*auth_manager*/ None,
        );

        assert_eq!(
            provider
                .runtime_base_url()
                .await
                .expect("runtime base URL should resolve"),
            Some("https://example.test/v1".to_string())
        );
    }

    #[test]
    fn odysseythink_provider_returns_unauthenticated_odysseythink_account_state() {
        let provider = create_model_provider(
            ModelProviderInfo::create_odysseythink_provider(/*base_url*/ None),
            /*auth_manager*/ None,
        );

        assert_eq!(
            provider.account_state().unwrap(),
            ProviderAccountState {
                account: None,
                requires_odysseythink_auth: true,
            }
        );
    }

    #[test]
    fn odysseythink_provider_returns_api_key_account_state() {
        let provider = create_model_provider(
            ModelProviderInfo::create_odysseythink_provider(/*base_url*/ None),
            Some(AuthManager::from_auth_for_testing(OdyAuth::from_api_key(
                "odysseythink-api-key",
            ))),
        );

        assert_eq!(
            provider.account_state().unwrap(),
            ProviderAccountState {
                account: Some(ProviderAccount::ApiKey),
                requires_odysseythink_auth: true,
            }
        );
    }

    #[test]
    fn custom_non_odysseythink_provider_returns_no_account_state() {
        let provider = create_model_provider(
            ModelProviderInfo {
                name: "Custom".to_string(),
                base_url: Some("http://localhost:1234/v1".to_string()),
                wire_api: WireApi::Responses,
                requires_odysseythink_auth: false,
                ..Default::default()
            },
            /*auth_manager*/ None,
        );

        assert_eq!(
            provider.account_state().unwrap(),
            ProviderAccountState {
                account: None,
                requires_odysseythink_auth: false,
            }
        );
    }

    #[tokio::test]
    async fn configured_provider_uses_provider_bearer_token_for_api_auth() {
        let mut provider_info = provider_for("https://example.test/v1".to_string());
        provider_info.experimental_bearer_token = Some("provider-token".to_string());
        let provider = create_model_provider(
            provider_info,
            Some(AuthManager::from_auth_for_testing(OdyAuth::from_api_key(
                "odysseythink-api-key",
            ))),
        );

        let auth = provider
            .api_auth()
            .await
            .expect("provider auth should resolve");
        let mut headers = http::HeaderMap::new();
        auth.add_auth_headers(&mut headers);

        assert_eq!(
            headers
                .get(http::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok()),
            Some("Bearer provider-token")
        );
    }
}
