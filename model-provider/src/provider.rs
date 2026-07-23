use std::fmt;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use ody_api::ApiError;
use ody_api::Provider;
use ody_api::SharedAuthProvider;
use ody_model_provider_info::ModelProviderInfo;
use ody_model_provider_info::ProviderCapabilities as ModelProviderInfoCapabilities;
use ody_models_manager::manager::OpenAiModelsManager;
use ody_models_manager::manager::SharedModelsManager;
use ody_models_manager::manager::StaticModelsManager;
use ody_protocol::error::OdyErr;
use ody_protocol::model_metadata::ModelsResponse;

use crate::adapters::chat::ChatAdapter;
use crate::adapters::responses::ResponsesAdapter;
use crate::auth::resolve_provider_auth;
use crate::chat_provider::ChatProvider;
use crate::models_endpoint::OpenAiModelsEndpoint;

/// Optional app-visible features that Ody may expose at runtime.
///
/// These are *feature-level* capabilities (e.g. whether the provider supports
/// namespaced tools or server-side web search). They are intentionally
/// separate from `chat_provider::ProviderCapabilities`, which describes raw
/// chat-model capabilities such as streaming, vision, or thinking.
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

impl From<&ModelProviderInfoCapabilities> for ProviderCapabilities {
    fn from(capabilities: &ModelProviderInfoCapabilities) -> Self {
        Self {
            namespace_tools: capabilities.namespace_tools,
            image_generation: capabilities.image_generation,
            web_search: capabilities.web_search,
        }
    }
}

/// Current app-visible account state for a model provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderAccountState;

pub type ProviderAccountResult = ody_protocol::error::Result<ProviderAccountState>;

/// Default model used for automatic approval review when a provider does not
/// require a backend-specific model ID.
pub const DEFAULT_APPROVAL_REVIEW_PREFERRED_MODEL: &str = "ody-auto-review";

/// Default model used for memory extraction when a provider does not require a
/// backend-specific model ID.
pub const DEFAULT_MEMORY_EXTRACTION_PREFERRED_MODEL: &str = "glm-4.5";

/// Default model used for memory consolidation when a provider does not require
/// a backend-specific model ID.
pub const DEFAULT_MEMORY_CONSOLIDATION_PREFERRED_MODEL: &str = "k3";

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

    /// Returns the current app-visible account state for this provider.
    fn account_state(&self) -> ProviderAccountResult;

    /// Maps an API client error into the provider's user-facing error representation.
    fn map_api_error(&self, error: ApiError) -> OdyErr {
        ody_api::map_api_error(error)
    }

    /// Returns provider configuration adapted for the API client.
    fn api_provider(&self) -> ModelProviderFuture<'_, ody_protocol::error::Result<Provider>> {
        Box::pin(async move { self.info().to_api_provider(None) })
    }

    /// Returns the provider base URL that will be used at request time.
    fn runtime_base_url(
        &self,
    ) -> ModelProviderFuture<'_, ody_protocol::error::Result<Option<String>>> {
        Box::pin(async { Ok(self.info().base_url.clone()) })
    }

    /// Returns the auth provider used to attach request credentials.
    fn api_auth(&self) -> ModelProviderFuture<'_, ody_protocol::error::Result<SharedAuthProvider>> {
        Box::pin(async move { resolve_provider_auth(self.info()) })
    }

    /// Returns a provider-neutral chat adapter for this provider.
    fn chat_provider(
        &self,
    ) -> ModelProviderFuture<'_, ody_protocol::error::Result<Box<dyn ChatProvider>>>;

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

/// Creates the default runtime model provider for configured provider metadata,
/// deriving the provider id from the wire API identity.
pub fn create_model_provider(provider_info: ModelProviderInfo) -> SharedModelProvider {
    create_model_provider_with_id(provider_id_for_wire_api(&provider_info), provider_info)
}

/// Creates the default runtime model provider for configured provider metadata
/// using the given provider id.
pub fn create_model_provider_with_id(
    provider_id: impl Into<String>,
    provider_info: ModelProviderInfo,
) -> SharedModelProvider {
    Arc::new(ConfiguredModelProvider::new(
        provider_id.into(),
        provider_info,
    ))
}

/// Runtime model provider backed by configured `ModelProviderInfo`.
#[derive(Clone, Debug)]
struct ConfiguredModelProvider {
    provider_id: String,
    info: ModelProviderInfo,
}

impl ConfiguredModelProvider {
    fn new(provider_id: String, provider_info: ModelProviderInfo) -> Self {
        Self {
            provider_id,
            info: provider_info,
        }
    }
}

impl ModelProvider for ConfiguredModelProvider {
    fn info(&self) -> &ModelProviderInfo {
        &self.info
    }

    fn chat_provider(
        &self,
    ) -> ModelProviderFuture<'_, ody_protocol::error::Result<Box<dyn ChatProvider>>> {
        Box::pin(async move {
            let api_provider = self.api_provider().await?;
            let auth = self.api_auth().await?;
            let transport =
                ody_api::ReqwestTransport::new(ody_client::default_client::build_reqwest_client());
            let adapter: Box<dyn ChatProvider> = match self.info.wire_api {
                ody_model_provider_info::WireApi::Responses => Box::new(
                    ResponsesAdapter::new(transport, api_provider, auth)
                        .with_provider_id(provider_id_for_wire_api(&self.info)),
                ),
                ody_model_provider_info::WireApi::Chat => {
                    let vendor = ody_api::chat::ChatVendor::from_provider(
                        &self.info.name,
                        self.info.base_url.as_deref(),
                    );
                    Box::new(
                        ChatAdapter::new(transport, api_provider, auth, vendor)
                            .with_provider_id(provider_id_for_wire_api(&self.info)),
                    )
                }
                _ => {
                    return Err(OdyErr::InvalidRequest(format!(
                        "wire api {:?} is not yet supported by chat_provider",
                        self.info.wire_api
                    )));
                }
            };
            Ok(adapter)
        })
    }

    fn capabilities(&self) -> ProviderCapabilities {
        (&self.info.capabilities).into()
    }

    fn account_state(&self) -> ProviderAccountResult {
        Ok(ProviderAccountState)
    }

    fn models_manager(
        &self,
        ody_home: PathBuf,
        config_model_catalog: Option<ModelsResponse>,
    ) -> SharedModelsManager {
        if let Some(model_catalog) = config_model_catalog {
            return Arc::new(StaticModelsManager::new(model_catalog));
        }

        let endpoint = Arc::new(OpenAiModelsEndpoint::new(
            self.provider_id.clone(),
            self.info.clone(),
        ));
        Arc::new(OpenAiModelsManager::new(ody_home, endpoint))
    }
}

/// Resolve the provider id used by the chat adapter.
fn provider_id_for_wire_api(info: &ModelProviderInfo) -> &'static str {
    if info.is_kimi() {
        "kimi"
    } else if info.is_deepseek() {
        "deepseek"
    } else if info.is_glm() {
        "glm"
    } else if info.wire_api == ody_model_provider_info::WireApi::Responses {
        "openai-responses"
    } else {
        "chat"
    }
}

#[cfg(test)]
mod tests {
    use ody_model_provider_info::ProviderCapabilities as ModelProviderCapabilities;
    use ody_model_provider_info::WireApi;
    use ody_models_manager::manager::RefreshStrategy;
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
            supports_websockets: false,
            capabilities: ModelProviderCapabilities::default(),
        }
    }

    #[tokio::test]
    async fn configured_provider_runtime_base_url_uses_configured_base_url() {
        let provider = create_model_provider(provider_for("https://example.test/v1".to_string()));

        assert_eq!(
            provider
                .runtime_base_url()
                .await
                .expect("runtime base URL should resolve"),
            Some("https://example.test/v1".to_string())
        );
    }

    #[test]
    fn custom_non_odysseythink_provider_returns_no_account_state() {
        let provider = create_model_provider(ModelProviderInfo {
            name: "Custom".to_string(),
            base_url: Some("http://localhost:1234/v1".to_string()),
            wire_api: WireApi::Responses,
            ..Default::default()
        });

        assert_eq!(provider.account_state().unwrap(), ProviderAccountState);
    }

    #[tokio::test]
    async fn configured_provider_uses_provider_bearer_token_for_api_auth() {
        let mut provider_info = provider_for("https://example.test/v1".to_string());
        provider_info.experimental_bearer_token = Some("provider-token".to_string());
        let provider = create_model_provider(provider_info);

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

    #[test]
    fn configured_provider_capabilities_match_info() {
        let info = ModelProviderInfo {
            name: "custom-chat".into(),
            base_url: Some("http://localhost:1234/v1".into()),
            wire_api: WireApi::Chat,
            capabilities: ModelProviderCapabilities {
                web_search: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let provider = create_model_provider_with_id("custom-chat", info);
        let caps = provider.capabilities();

        assert!(caps.web_search);
        assert!(!caps.namespace_tools);
    }
}
