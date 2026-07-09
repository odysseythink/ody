use std::sync::Arc;

use http::HeaderMap;
use ody_api::AuthProvider;
use ody_api::SharedAuthProvider;
use ody_model_provider_info::ModelProviderInfo;

use crate::bearer_auth_provider::BearerAuthProvider;

// Some providers are meant to send no auth headers. Examples include local OSS
// providers and custom test providers with `requires_openai_auth = false`.
#[derive(Clone, Debug)]
struct UnauthenticatedAuthProvider;

impl AuthProvider for UnauthenticatedAuthProvider {
    fn add_auth_headers(&self, _headers: &mut HeaderMap) {}
}

pub fn unauthenticated_auth_provider() -> SharedAuthProvider {
    Arc::new(UnauthenticatedAuthProvider)
}

pub(crate) fn resolve_provider_auth(
    provider: &ModelProviderInfo,
) -> ody_protocol::error::Result<SharedAuthProvider> {
    if let Some(auth) = bearer_auth_for_provider(provider)? {
        return Ok(Arc::new(auth));
    }

    Ok(unauthenticated_auth_provider())
}

fn bearer_auth_for_provider(
    provider: &ModelProviderInfo,
) -> ody_protocol::error::Result<Option<BearerAuthProvider>> {
    if let Some(api_key) = provider.api_key()? {
        return Ok(Some(BearerAuthProvider::new(api_key)));
    }

    if let Some(token) = provider.experimental_bearer_token.clone() {
        return Ok(Some(BearerAuthProvider::new(token)));
    }

    Ok(None)
}
