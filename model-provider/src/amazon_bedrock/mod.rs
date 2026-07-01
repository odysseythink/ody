mod auth;
mod catalog;
mod error;
mod mantle;

use std::path::PathBuf;
use std::sync::Arc;

use ody_api::ApiError;
use ody_api::Provider;
use ody_api::SharedAuthProvider;
use ody_login::AuthManager;
use ody_login::OdyAuth;
use ody_model_provider_info::AMAZON_BEDROCK_GPT_5_4_MODEL_ID;
use ody_model_provider_info::ModelProviderAwsAuthInfo;
use ody_model_provider_info::ModelProviderInfo;
use ody_models_manager::manager::SharedModelsManager;
use ody_models_manager::manager::StaticModelsManager;
use ody_protocol::account::AmazonBedrockCredentialSource;
use ody_protocol::account::ProviderAccount;
use ody_protocol::error::OdyErr;
use ody_protocol::error::Result;
use ody_protocol::odysseythink_models::ModelsResponse;

use crate::provider::ModelProvider;
use crate::provider::ModelProviderFuture;
use crate::provider::ProviderAccountResult;
use crate::provider::ProviderAccountState;
use crate::provider::ProviderCapabilities;
use auth::resolve_provider_auth;
pub(crate) use catalog::static_model_catalog;
use catalog::with_default_only_service_tier;
use mantle::runtime_base_url;

/// Runtime provider for Amazon Bedrock's OpenAI-compatible Mantle endpoint.
#[derive(Clone, Debug)]
pub(crate) struct AmazonBedrockModelProvider {
    pub(crate) info: ModelProviderInfo,
    pub(crate) aws: ModelProviderAwsAuthInfo,
}

impl AmazonBedrockModelProvider {
    pub(crate) fn new(
        provider_info: ModelProviderInfo,
        _auth_manager: Option<Arc<AuthManager>>,
    ) -> Self {
        let aws = provider_info
            .aws
            .clone()
            .unwrap_or(ModelProviderAwsAuthInfo {
                profile: None,
                region: None,
            });
        Self {
            info: provider_info,
            aws,
        }
    }

    async fn auth(&self) -> Option<OdyAuth> {
        None
    }

    async fn api_provider(&self) -> Result<Provider> {
        let mut api_provider_info = self.info.clone();
        api_provider_info.base_url =
            Some(runtime_base_url(&self.aws).await?);
        api_provider_info.to_api_provider(/*auth_mode*/ None)
    }

    async fn runtime_base_url(&self) -> Result<Option<String>> {
        Ok(Some(
            runtime_base_url(&self.aws).await?,
        ))
    }

    async fn api_auth(&self) -> Result<SharedAuthProvider> {
        resolve_provider_auth(&self.aws).await
    }
}

impl ModelProvider for AmazonBedrockModelProvider {
    fn info(&self) -> &ModelProviderInfo {
        &self.info
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            namespace_tools: true,
            image_generation: false,
            web_search: false,
        }
    }

    fn approval_review_preferred_model(&self) -> &'static str {
        AMAZON_BEDROCK_GPT_5_4_MODEL_ID
    }

    fn memory_extraction_preferred_model(&self) -> &'static str {
        AMAZON_BEDROCK_GPT_5_4_MODEL_ID
    }

    fn memory_consolidation_preferred_model(&self) -> &'static str {
        AMAZON_BEDROCK_GPT_5_4_MODEL_ID
    }

    fn auth_manager(&self) -> Option<Arc<AuthManager>> {
        None
    }

    fn auth(&self) -> ModelProviderFuture<'_, Option<OdyAuth>> {
        Box::pin(AmazonBedrockModelProvider::auth(self))
    }

    fn account_state(&self) -> ProviderAccountResult {
        Ok(ProviderAccountState {
            account: Some(ProviderAccount::AmazonBedrock {
                credential_source: AmazonBedrockCredentialSource::AwsManaged,
            }),
            requires_odysseythink_auth: false,
        })
    }

    fn map_api_error(&self, error: ApiError) -> OdyErr {
        error::map_api_error(error)
    }

    fn api_provider(&self) -> ModelProviderFuture<'_, Result<Provider>> {
        Box::pin(AmazonBedrockModelProvider::api_provider(self))
    }

    fn runtime_base_url(&self) -> ModelProviderFuture<'_, Result<Option<String>>> {
        Box::pin(AmazonBedrockModelProvider::runtime_base_url(self))
    }

    fn api_auth(&self) -> ModelProviderFuture<'_, Result<SharedAuthProvider>> {
        Box::pin(AmazonBedrockModelProvider::api_auth(self))
    }

    fn models_manager(
        &self,
        _ody_home: PathBuf,
        config_model_catalog: Option<ModelsResponse>,
    ) -> SharedModelsManager {
        Arc::new(StaticModelsManager::new(
            /*auth_manager*/ None,
            config_model_catalog.map_or_else(static_model_catalog, with_default_only_service_tier),
        ))
    }
}

#[cfg(test)]
#[path = "error_tests.rs"]
mod error_tests;

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn api_provider_for_bedrock_bearer_token_uses_configured_region_endpoint() {
        let region = "eu-central-1";
        let mut api_provider_info =
            ModelProviderInfo::create_amazon_bedrock_provider(/*aws*/ None);
        api_provider_info.base_url = Some(mantle::base_url(region).expect("supported region"));
        let api_provider = api_provider_info
            .to_api_provider(/*auth_mode*/ None)
            .expect("api provider should build");

        assert_eq!(
            api_provider.base_url,
            "https://bedrock-mantle.eu-central-1.api.aws/odysseythink/v1"
        );
    }

    #[tokio::test]
    async fn odysseythink_auth_is_not_exposed_to_bedrock() {
        let provider = AmazonBedrockModelProvider::new(
            ModelProviderInfo::create_amazon_bedrock_provider(/*aws*/ None),
            Some(AuthManager::from_auth_for_testing(OdyAuth::from_api_key(
                "odysseythink-api-key",
            ))),
        );

        assert!(provider.auth_manager().is_none());
        assert_eq!(provider.auth().await, None);
        assert_eq!(
            provider.account_state().unwrap(),
            ProviderAccountState {
                account: Some(ProviderAccount::AmazonBedrock {
                    credential_source: AmazonBedrockCredentialSource::AwsManaged,
                }),
                requires_odysseythink_auth: false,
            }
        );
    }

    #[test]
    fn capabilities_disable_unsupported_hosted_tools() {
        let provider = AmazonBedrockModelProvider::new(
            ModelProviderInfo::create_amazon_bedrock_provider(/*aws*/ None),
            /*auth_manager*/ None,
        );

        assert_eq!(
            provider.capabilities(),
            ProviderCapabilities {
                namespace_tools: true,
                image_generation: false,
                web_search: false,
            }
        );
    }

    #[test]
    fn approval_review_preferred_model_uses_bedrock_gpt_5_4() {
        let provider = AmazonBedrockModelProvider::new(
            ModelProviderInfo::create_amazon_bedrock_provider(/*aws*/ None),
            /*auth_manager*/ None,
        );

        assert_eq!(
            provider.approval_review_preferred_model(),
            AMAZON_BEDROCK_GPT_5_4_MODEL_ID
        );
    }
}
