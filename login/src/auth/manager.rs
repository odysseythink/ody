use std::env;
use std::fmt::Debug;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::RwLock;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use tokio::sync::watch;
use tracing::instrument;

use ody_app_server_protocol::AuthMode;
use ody_app_server_protocol::AuthMode as ApiAuthMode;
use ody_protocol::config_types::ForcedLoginMethod;

pub use crate::auth::storage::AuthDotJson;
pub use crate::auth::storage::AuthKeyringBackendKind;
use crate::auth::storage::create_auth_storage;
use crate::outbound_proxy::AuthRouteConfig;
use ody_config::types::AuthCredentialsStoreMode;

/// Authentication mechanism used by the current user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OdyAuth {
    ApiKey(ApiKeyAuth),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKeyAuth {
    api_key: String,
}

static NEXT_DUMMY_AUTH_ID: AtomicU64 = AtomicU64::new(1);

impl OdyAuth {
    async fn from_auth_dot_json(
        auth_dot_json: AuthDotJson,
    ) -> std::io::Result<Self> {
        let auth_mode = auth_dot_json.resolved_mode();
        if auth_mode == ApiAuthMode::ApiKey {
            let Some(api_key) = auth_dot_json.odysseythink_api_key.as_deref() else {
                return Err(std::io::Error::other("API key auth is missing a key."));
            };
            return Ok(Self::from_api_key(api_key));
        }

        Err(std::io::Error::other(
            "Stored auth uses a login method that is no longer supported. Please log in again with an API key.",
        ))
    }

    pub async fn from_auth_storage(
        ody_home: &Path,
        auth_credentials_store_mode: AuthCredentialsStoreMode,
        keyring_backend_kind: AuthKeyringBackendKind,
        auth_route_config: Option<&AuthRouteConfig>,
    ) -> std::io::Result<Option<Self>> {
        load_auth(
            ody_home,
            /*enable_ody_api_key_env*/ false,
            auth_credentials_store_mode,
            keyring_backend_kind,
            auth_route_config,
        )
        .await
    }

    pub fn auth_mode(&self) -> AuthMode {
        AuthMode::ApiKey
    }

    pub fn api_auth_mode(&self) -> ApiAuthMode {
        ApiAuthMode::ApiKey
    }

    pub fn is_api_key_auth(&self) -> bool {
        true
    }

    /// Returns the API key for API-key auth.
    pub fn api_key(&self) -> Option<&str> {
        match self {
            Self::ApiKey(auth) => Some(auth.api_key.as_str()),
        }
    }

    /// Returns the token string used for bearer authentication.
    pub fn get_token(&self) -> Result<String, std::io::Error> {
        match self {
            Self::ApiKey(auth) => Ok(auth.api_key.clone()),
        }
    }

    /// Consider this private to integration tests.
    pub fn create_dummy_api_key_auth_for_testing() -> Self {
        let dummy_auth_id = NEXT_DUMMY_AUTH_ID.fetch_add(1, Ordering::Relaxed);
        Self::from_api_key(&format!("sk-dummy-api-key-auth-{dummy_auth_id}"))
    }

    pub fn from_api_key(api_key: &str) -> Self {
        Self::ApiKey(ApiKeyAuth {
            api_key: api_key.to_owned(),
        })
    }
}

pub const OPENAI_API_KEY_ENV_VAR: &str = "OPENAI_API_KEY";
pub const ODY_API_KEY_ENV_VAR: &str = "ODY_API_KEY";

pub fn read_odysseythink_api_key_from_env() -> Option<String> {
    env::var(OPENAI_API_KEY_ENV_VAR)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub fn read_ody_api_key_from_env() -> Option<String> {
    read_non_empty_env_var(ODY_API_KEY_ENV_VAR)
}

fn read_non_empty_env_var(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Delete the auth.json file inside `ody_home` if it exists. Returns `Ok(true)`
/// if a file was removed, `Ok(false)` if no auth file was present.
pub fn logout(
    ody_home: &Path,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    keyring_backend_kind: AuthKeyringBackendKind,
) -> std::io::Result<bool> {
    let storage = create_auth_storage(
        ody_home.to_path_buf(),
        auth_credentials_store_mode,
        keyring_backend_kind,
    );
    storage.delete()
}

/// Writes an `auth.json` that contains only the API key.
pub fn login_with_api_key(
    ody_home: &Path,
    api_key: &str,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    keyring_backend_kind: AuthKeyringBackendKind,
) -> std::io::Result<()> {
    let auth_dot_json = AuthDotJson {
        auth_mode: Some(ApiAuthMode::ApiKey),
        odysseythink_api_key: Some(api_key.to_string()),
    };
    save_auth(
        ody_home,
        &auth_dot_json,
        auth_credentials_store_mode,
        keyring_backend_kind,
    )
}

/// Persist the provided auth payload using the specified backend.
pub fn save_auth(
    ody_home: &Path,
    auth: &AuthDotJson,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    keyring_backend_kind: AuthKeyringBackendKind,
) -> std::io::Result<()> {
    let storage = create_auth_storage(
        ody_home.to_path_buf(),
        auth_credentials_store_mode,
        keyring_backend_kind,
    );
    storage.save(auth)
}

/// Load the raw stored auth payload without applying environment overrides.
///
/// Returns `None` when no credentials are stored. Prefer `AuthManager` for
/// ordinary production reads; this helper is for tests and write-side
/// maintenance that must inspect the exact payload in storage.
pub fn load_auth_dot_json(
    ody_home: &Path,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    keyring_backend_kind: AuthKeyringBackendKind,
) -> std::io::Result<Option<AuthDotJson>> {
    let storage = create_auth_storage(
        ody_home.to_path_buf(),
        auth_credentials_store_mode,
        keyring_backend_kind,
    );
    storage.load()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthConfig {
    pub ody_home: PathBuf,
    pub auth_credentials_store_mode: AuthCredentialsStoreMode,
    pub keyring_backend_kind: AuthKeyringBackendKind,
    pub forced_login_method: Option<ForcedLoginMethod>,
    pub auth_route_config: Option<AuthRouteConfig>,
}

/// Enforces configured login restrictions using auth-owned HTTP settings.
pub async fn enforce_login_restrictions(config: &AuthConfig) -> std::io::Result<()> {
    let Some(auth) = load_auth(
        &config.ody_home,
        /*enable_ody_api_key_env*/ true,
        config.auth_credentials_store_mode,
        config.keyring_backend_kind,
        config.auth_route_config.as_ref(),
    )
    .await?
    else {
        return Ok(());
    };

    if let Some(required_method) = config.forced_login_method {
        let method_violation = match (required_method, auth.auth_mode()) {
            (ForcedLoginMethod::Api, AuthMode::ApiKey) => None,
            (ForcedLoginMethod::Chatgpt, _) => Some(
                "ChatGPT login is no longer supported. Logging out.".to_string(),
            ),
            (ForcedLoginMethod::Api, _) => Some(
                "API key login is required, but the stored credentials use an unsupported login method. Logging out."
                    .to_string(),
            ),
        };

        if let Some(message) = method_violation {
            return logout_with_message(
                &config.ody_home,
                message,
                config.auth_credentials_store_mode,
                config.keyring_backend_kind,
            );
        }
    }

    Ok(())
}

fn logout_with_message(
    ody_home: &Path,
    message: String,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    keyring_backend_kind: AuthKeyringBackendKind,
) -> std::io::Result<()> {
    // Persistent auth may still exist from earlier logins. Clear it so a forced
    // logout truly removes all active auth.
    let removal_result = logout_all_stores(
        ody_home,
        auth_credentials_store_mode,
        keyring_backend_kind,
    );
    let error_message = match removal_result {
        Ok(_) => message,
        Err(err) => format!("{message}. Failed to remove auth.json: {err}"),
    };
    Err(std::io::Error::other(error_message))
}

fn logout_all_stores(
    ody_home: &Path,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    keyring_backend_kind: AuthKeyringBackendKind,
) -> std::io::Result<bool> {
    if auth_credentials_store_mode == AuthCredentialsStoreMode::Ephemeral {
        return logout(
            ody_home,
            AuthCredentialsStoreMode::Ephemeral,
            AuthKeyringBackendKind::default(),
        );
    }
    let removed_ephemeral = logout(
        ody_home,
        AuthCredentialsStoreMode::Ephemeral,
        AuthKeyringBackendKind::default(),
    )?;
    let removed_managed = logout(
        ody_home,
        auth_credentials_store_mode,
        keyring_backend_kind,
    )?;
    Ok(removed_ephemeral || removed_managed)
}

async fn load_auth(
    ody_home: &Path,
    enable_ody_api_key_env: bool,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    keyring_backend_kind: AuthKeyringBackendKind,
    _auth_route_config: Option<&AuthRouteConfig>,
) -> std::io::Result<Option<OdyAuth>> {
    // API key via env var takes precedence over any other auth method.
    if enable_ody_api_key_env && let Some(api_key) = read_ody_api_key_from_env() {
        return Ok(Some(OdyAuth::from_api_key(api_key.as_str())));
    }

    // External auth tokens used to live in the in-memory (ephemeral) store. With
    // only API-key auth supported now, just fall through to persistent storage.
    if auth_credentials_store_mode == AuthCredentialsStoreMode::Ephemeral {
        return Ok(None);
    }

    // Fall back to the configured persistent store (file/keyring/auto) for managed auth.
    let storage = create_auth_storage(
        ody_home.to_path_buf(),
        auth_credentials_store_mode,
        keyring_backend_kind,
    );
    let auth_dot_json = match storage.load()? {
        Some(auth) => auth,
        None => return Ok(None),
    };

    let auth = OdyAuth::from_auth_dot_json(auth_dot_json).await?;
    Ok(Some(auth))
}

/// Internal cached auth state.
#[derive(Clone)]
struct CachedAuth {
    auth: Option<OdyAuth>,
}

impl Debug for CachedAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachedAuth")
            .field("auth_mode", &self.auth.as_ref().map(OdyAuth::api_auth_mode))
            .finish()
    }
}

/// Central manager providing a single source of truth for auth.json derived
/// authentication data. It loads once (or on preference change) and then
/// hands out cloned `OdyAuth` values so the rest of the program has a
/// consistent snapshot.
///
/// External modifications to `auth.json` will NOT be observed until
/// `reload()` is called explicitly. This matches the design goal of avoiding
/// different parts of the program seeing inconsistent auth data mid‑run.
pub struct AuthManager {
    ody_home: PathBuf,
    inner: RwLock<CachedAuth>,
    auth_change_tx: watch::Sender<u64>,
    enable_ody_api_key_env: bool,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    keyring_backend_kind: AuthKeyringBackendKind,
    auth_route_config: Option<AuthRouteConfig>,
}

/// Configuration view required to construct a shared [`AuthManager`].
///
/// Implementations should return the auth-related config values for the
/// already-resolved runtime configuration. The primary implementation is
/// `ody_core::config::Config`, but this trait keeps `ody-login` independent
/// from `ody-core`.
pub trait AuthManagerConfig {
    /// Returns the Ody home directory used for auth storage.
    fn ody_home(&self) -> PathBuf;

    /// Returns the CLI auth credential storage mode for auth loading.
    fn cli_auth_credentials_store_mode(&self) -> AuthCredentialsStoreMode;

    /// Returns the backend to use when CLI auth keyring storage is selected.
    fn auth_keyring_backend_kind(&self) -> AuthKeyringBackendKind;

    /// Returns route-selection settings for auth-owned clients.
    fn auth_route_config(&self) -> Option<AuthRouteConfig>;
}

impl Debug for AuthManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthManager")
            .field("ody_home", &self.ody_home)
            .field("inner", &self.inner)
            .field("enable_ody_api_key_env", &self.enable_ody_api_key_env)
            .field(
                "auth_credentials_store_mode",
                &self.auth_credentials_store_mode,
            )
            .field("keyring_backend_kind", &self.keyring_backend_kind)
            .field("auth_route_config", &self.auth_route_config)
            .finish_non_exhaustive()
    }
}

impl AuthManager {
    /// Create a new manager loading the initial auth using the provided
    /// preferred auth method. Errors loading auth are swallowed; `auth()` will
    /// simply return `None` in that case so callers can treat it as an
    /// unauthenticated state.
    pub async fn new(
        ody_home: PathBuf,
        enable_ody_api_key_env: bool,
        auth_credentials_store_mode: AuthCredentialsStoreMode,
        keyring_backend_kind: AuthKeyringBackendKind,
        auth_route_config: Option<AuthRouteConfig>,
    ) -> Self {
        let managed_auth = load_auth(
            &ody_home,
            enable_ody_api_key_env,
            auth_credentials_store_mode,
            keyring_backend_kind,
            auth_route_config.as_ref(),
        )
        .await
        .ok()
        .flatten();
        let (auth_change_tx, _auth_change_rx) = watch::channel(0);
        Self {
            ody_home,
            inner: RwLock::new(CachedAuth {
                auth: managed_auth,
            }),
            auth_change_tx,
            enable_ody_api_key_env,
            auth_credentials_store_mode,
            keyring_backend_kind,
            auth_route_config,
        }
    }

    /// Create an AuthManager with a specific OdyAuth, for testing only.
    pub fn from_auth_for_testing(auth: OdyAuth) -> Arc<Self> {
        let cached = CachedAuth { auth: Some(auth) };
        let (auth_change_tx, _auth_change_rx) = watch::channel(0);

        Arc::new(Self {
            ody_home: PathBuf::from("non-existent"),
            inner: RwLock::new(cached),
            auth_change_tx,
            enable_ody_api_key_env: false,
            auth_credentials_store_mode: AuthCredentialsStoreMode::File,
            keyring_backend_kind: AuthKeyringBackendKind::default(),
            auth_route_config: None,
        })
    }

    /// Create an AuthManager with a specific OdyAuth and ody home, for testing only.
    pub fn from_auth_for_testing_with_home(auth: OdyAuth, ody_home: PathBuf) -> Arc<Self> {
        let cached = CachedAuth { auth: Some(auth) };
        let (auth_change_tx, _auth_change_rx) = watch::channel(0);
        Arc::new(Self {
            ody_home,
            inner: RwLock::new(cached),
            auth_change_tx,
            enable_ody_api_key_env: false,
            auth_credentials_store_mode: AuthCredentialsStoreMode::File,
            keyring_backend_kind: AuthKeyringBackendKind::default(),
            auth_route_config: None,
        })
    }

    /// Current cached auth (clone) without attempting a refresh.
    pub fn auth_cached(&self) -> Option<OdyAuth> {
        self.inner.read().ok().and_then(|c| c.auth.clone())
    }

    /// Current cached auth (clone). May be `None` if not logged in or load failed.
    #[instrument(level = "trace", skip_all)]
    pub async fn auth(&self) -> Option<OdyAuth> {
        self.auth_cached()
    }

    /// Force a reload of the auth information from auth.json. Returns
    /// whether the auth value changed.
    pub async fn reload(&self) -> bool {
        tracing::info!("Reloading auth");
        let new_auth = self.load_auth_from_storage().await;
        self.set_cached_auth(new_auth)
    }

    async fn load_auth_from_storage(&self) -> Option<OdyAuth> {
        load_auth(
            &self.ody_home,
            self.enable_ody_api_key_env,
            self.auth_credentials_store_mode,
            self.keyring_backend_kind,
            self.auth_route_config.as_ref(),
        )
        .await
        .ok()
        .flatten()
    }

    fn set_cached_auth(&self, new_auth: Option<OdyAuth>) -> bool {
        if let Ok(mut guard) = self.inner.write() {
            let previous = guard.auth.as_ref();
            let changed = !AuthManager::auths_equal(previous, new_auth.as_ref());
            tracing::info!("Reloaded auth, changed: {changed}");
            guard.auth = new_auth;
            if changed {
                self.auth_change_tx.send_modify(|revision| *revision += 1);
            }
            changed
        } else {
            false
        }
    }

    fn auths_equal(a: Option<&OdyAuth>, b: Option<&OdyAuth>) -> bool {
        match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => a == b,
            _ => false,
        }
    }

    /// Subscribes to cached auth changes that can affect request recovery.
    pub fn auth_change_receiver(&self) -> watch::Receiver<u64> {
        self.auth_change_tx.subscribe()
    }

    pub fn ody_api_key_env_enabled(&self) -> bool {
        self.enable_ody_api_key_env
    }

    /// Convenience constructor returning an `Arc` wrapper.
    pub async fn shared(
        ody_home: PathBuf,
        enable_ody_api_key_env: bool,
        auth_credentials_store_mode: AuthCredentialsStoreMode,
        keyring_backend_kind: AuthKeyringBackendKind,
        auth_route_config: Option<AuthRouteConfig>,
    ) -> Arc<Self> {
        Arc::new(
            Self::new(
                ody_home,
                enable_ody_api_key_env,
                auth_credentials_store_mode,
                keyring_backend_kind,
                auth_route_config,
            )
            .await,
        )
    }

    /// Convenience constructor returning an `Arc` wrapper from resolved config.
    pub async fn shared_from_config(
        config: &impl AuthManagerConfig,
        enable_ody_api_key_env: bool,
    ) -> Arc<Self> {
        Self::shared(
            config.ody_home(),
            enable_ody_api_key_env,
            config.cli_auth_credentials_store_mode(),
            config.auth_keyring_backend_kind(),
            config.auth_route_config(),
        )
        .await
    }

    /// Log out by deleting the on‑disk auth.json (if present). Returns Ok(true)
    /// if a file was removed, Ok(false) if no auth file existed. On success,
    /// reloads the in‑memory auth cache so callers immediately observe the
    /// unauthenticated state.
    pub async fn logout(&self) -> std::io::Result<bool> {
        let removed = logout_all_stores(
            &self.ody_home,
            self.auth_credentials_store_mode,
            self.keyring_backend_kind,
        )?;
        // Always reload to clear any cached auth (even if file absent).
        self.reload().await;
        Ok(removed)
    }

    pub fn get_api_auth_mode(&self) -> Option<ApiAuthMode> {
        self.auth_cached().as_ref().map(OdyAuth::api_auth_mode)
    }

    pub fn auth_mode(&self) -> Option<AuthMode> {
        self.auth_cached().as_ref().map(OdyAuth::auth_mode)
    }

    pub fn current_auth_uses_ody_backend(&self) -> bool {
        false
    }
}
