use ody_config::CloudConfigBundleLoader;
use ody_config::types::AuthCredentialsStoreMode;
use ody_login::AuthKeyringBackendKind;
use ody_login::AuthManager;
use ody_login::AuthRouteConfig;
use std::path::PathBuf;
use std::sync::Arc;

/// Returns a no-op cloud-config bundle loader.
///
/// The OpenAI/Codex backend-delivered enterprise config bundle was removed in
/// M1.2; this stub preserves the public surface while always yielding no bundle.
pub fn cloud_config_bundle_loader(
    _auth_manager: Arc<AuthManager>,
    _chatgpt_base_url: String,
    _ody_home: PathBuf,
) -> CloudConfigBundleLoader {
    CloudConfigBundleLoader::default()
}

pub async fn cloud_config_bundle_loader_for_storage(
    _ody_home: PathBuf,
    _enable_ody_api_key_env: bool,
    _credentials_store_mode: AuthCredentialsStoreMode,
    _keyring_backend_kind: AuthKeyringBackendKind,
    _chatgpt_base_url: String,
    _auth_route_config: Option<AuthRouteConfig>,
) -> CloudConfigBundleLoader {
    CloudConfigBundleLoader::default()
}
