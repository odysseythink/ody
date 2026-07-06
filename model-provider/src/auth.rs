use std::sync::Arc;

use http::HeaderMap;
use ody_api::AuthProvider;
use ody_api::SharedAuthProvider;
use ody_login::AuthManager;
use ody_login::OdyAuth;
use ody_model_provider_info::ModelProviderInfo;

use crate::bearer_auth_provider::BearerAuthProvider;

// Some providers are meant to send no auth headers. Examples include local OSS
// providers and custom test providers with `requires_odysseythink_auth = false`.
#[derive(Clone, Debug)]
struct UnauthenticatedAuthProvider;

impl AuthProvider for UnauthenticatedAuthProvider {
    fn add_auth_headers(&self, _headers: &mut HeaderMap) {}
}

pub fn unauthenticated_auth_provider() -> SharedAuthProvider {
    Arc::new(UnauthenticatedAuthProvider)
}

/// Returns the provider-scoped auth manager when this provider uses custom auth.
///
/// External bearer auth has been removed, so custom command-backed auth is no
/// longer supported through `AuthManager`. Providers continue using the
/// caller-supplied base manager, when present.
pub(crate) fn auth_manager_for_provider(
    auth_manager: Option<Arc<AuthManager>>,
    _provider: &ModelProviderInfo,
) -> Option<Arc<AuthManager>> {
    auth_manager
}

pub(crate) fn resolve_provider_auth(
    auth: Option<&OdyAuth>,
    provider: &ModelProviderInfo,
) -> ody_protocol::error::Result<SharedAuthProvider> {
    if let Some(auth) = bearer_auth_for_provider(provider)? {
        return Ok(Arc::new(auth));
    }

    Ok(match auth {
        Some(auth) => auth_provider_from_auth(auth),
        None => unauthenticated_auth_provider(),
    })
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

/// Builds request-header auth for a first-party Ody auth snapshot.
pub fn auth_provider_from_auth(auth: &OdyAuth) -> SharedAuthProvider {
    match auth {
        OdyAuth::ApiKey(_) => Arc::new(BearerAuthProvider {
            token: auth.get_token().ok(),
        }),
    }
}
