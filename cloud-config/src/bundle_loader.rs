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
pub fn cloud_config_bundle_loader() -> CloudConfigBundleLoader {
    CloudConfigBundleLoader::default()
}

/// Returns a no-op cloud-config bundle loader for storage.
pub fn cloud_config_bundle_loader_for_storage() -> CloudConfigBundleLoader {
    CloudConfigBundleLoader::default()
}
