use std::env;
use std::fmt::Debug;
use std::future::Future;
use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::RwLock;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use tokio::sync::Semaphore;
use tokio::sync::watch;
use tracing::instrument;

use ody_app_server_protocol::AuthMode;
use ody_app_server_protocol::AuthMode as ApiAuthMode;
use ody_protocol::config_types::ForcedLoginMethod;
use ody_protocol::config_types::ModelProviderAuthInfo;

use super::external_bearer::BearerTokenRefresher;
use super::revoke::revoke_auth_tokens;
pub use crate::auth::bedrock_api_key::BedrockApiKeyAuth;
pub use crate::auth::storage::AuthDotJson;
pub use crate::auth::storage::AuthKeyringBackendKind;
use crate::auth::storage::create_auth_storage;
use crate::outbound_proxy::AuthRouteConfig;
use crate::token_data::TokenData;
use ody_config::types::AuthCredentialsStoreMode;
use ody_protocol::account::PlanType as AccountPlanType;
use ody_protocol::auth::RefreshTokenFailedError;
use ody_protocol::auth::RefreshTokenFailedReason;
use thiserror::Error;

/// Authentication mechanism used by the current user.
#[derive(Debug, Clone)]
pub enum OdyAuth {
    ApiKey(ApiKeyAuth),
    BedrockApiKey(BedrockApiKeyAuth),
}

impl PartialEq for OdyAuth {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::ApiKey(a), Self::ApiKey(b)) => a == b,
            (Self::BedrockApiKey(a), Self::BedrockApiKey(b)) => a == b,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKeyAuth {
    api_key: String,
}

const REFRESH_TOKEN_UNKNOWN_MESSAGE: &str =
    "Your access token could not be refreshed. Please log out and sign in again.";
const REFRESH_TOKEN_ACCOUNT_MISMATCH_MESSAGE: &str = "Your access token could not be refreshed because you have since logged out or signed in to another account. Please sign in again.";
pub(super) const REVOKE_TOKEN_URL: &str = "https://auth.odysseythink.com/oauth/revoke";
pub const REFRESH_TOKEN_URL_OVERRIDE_ENV_VAR: &str = "ODY_REFRESH_TOKEN_URL_OVERRIDE";
pub const REVOKE_TOKEN_URL_OVERRIDE_ENV_VAR: &str = "ODY_REVOKE_TOKEN_URL_OVERRIDE";
pub const CLIENT_ID_OVERRIDE_ENV_VAR: &str = "ODY_APP_SERVER_LOGIN_CLIENT_ID";
static NEXT_DUMMY_AUTH_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Error)]
pub enum RefreshTokenError {
    #[error("{0}")]
    Permanent(#[from] RefreshTokenFailedError),
    #[error(transparent)]
    Transient(#[from] std::io::Error),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalAuthTokens {
    pub access_token: String,
    pub chatgpt_metadata: Option<ExternalAuthChatgptMetadata>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalAuthChatgptMetadata {
    pub account_id: String,
    pub plan_type: Option<String>,
}

impl ExternalAuthTokens {
    pub fn access_token_only(access_token: impl Into<String>) -> Self {
        Self {
            access_token: access_token.into(),
            chatgpt_metadata: None,
        }
    }

    pub fn chatgpt(
        access_token: impl Into<String>,
        chatgpt_account_id: impl Into<String>,
        chatgpt_plan_type: Option<String>,
    ) -> Self {
        Self {
            access_token: access_token.into(),
            chatgpt_metadata: Some(ExternalAuthChatgptMetadata {
                account_id: chatgpt_account_id.into(),
                plan_type: chatgpt_plan_type,
            }),
        }
    }

    pub fn chatgpt_metadata(&self) -> Option<&ExternalAuthChatgptMetadata> {
        self.chatgpt_metadata.as_ref()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalAuthRefreshReason {
    Unauthorized,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalAuthRefreshContext {
    pub reason: ExternalAuthRefreshReason,
    pub previous_account_id: Option<String>,
}

/// Pluggable auth provider used by `AuthManager` for externally managed auth flows.
///
/// Implementations may either resolve auth eagerly via `resolve()` or provide refreshed
/// credentials on demand via `refresh()`.
pub trait ExternalAuth: Send + Sync {
    /// Indicates which top-level auth mode this external provider supplies.
    fn auth_mode(&self) -> AuthMode;

    /// Returns cached or immediately available auth, if this provider can resolve it synchronously
    /// from the caller's perspective.
    fn resolve(&self) -> ExternalAuthFuture<'_, Option<ExternalAuthTokens>> {
        Box::pin(async { Ok(None) })
    }

    /// Refreshes auth in response to a manager-driven refresh attempt.
    fn refresh(
        &self,
        context: ExternalAuthRefreshContext,
    ) -> ExternalAuthFuture<'_, ExternalAuthTokens>;
}

pub type ExternalAuthFuture<'a, T> = Pin<Box<dyn Future<Output = std::io::Result<T>> + Send + 'a>>;

impl RefreshTokenError {
    pub fn failed_reason(&self) -> Option<RefreshTokenFailedReason> {
        match self {
            Self::Permanent(error) => Some(error.reason),
            Self::Transient(_) => None,
        }
    }
}

impl From<RefreshTokenError> for std::io::Error {
    fn from(err: RefreshTokenError) -> Self {
        match err {
            RefreshTokenError::Permanent(failed) => std::io::Error::other(failed),
            RefreshTokenError::Transient(inner) => inner,
        }
    }
}

impl OdyAuth {
    async fn from_auth_dot_json(
        _ody_home: &Path,
        auth_dot_json: AuthDotJson,
        _auth_credentials_store_mode: AuthCredentialsStoreMode,
    ) -> std::io::Result<Self> {
        if let Some(auth) = auth_dot_json.bedrock_api_key {
            return Ok(Self::BedrockApiKey(auth));
        }

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
        _chatgpt_base_url: Option<&str>,
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
        match self {
            Self::ApiKey(_) | Self::BedrockApiKey(_) => AuthMode::ApiKey,
        }
    }

    pub fn api_auth_mode(&self) -> ApiAuthMode {
        match self {
            Self::ApiKey(_) | Self::BedrockApiKey(_) => ApiAuthMode::ApiKey,
        }
    }

    pub fn is_api_key_auth(&self) -> bool {
        self.auth_mode() == AuthMode::ApiKey
    }

    pub fn is_chatgpt_auth(&self) -> bool {
        false
    }

    pub fn uses_ody_backend(&self) -> bool {
        false
    }

    /// Returns `None` if `auth_mode() != AuthMode::ApiKey`.
    pub fn api_key(&self) -> Option<&str> {
        match self {
            Self::ApiKey(auth) => Some(auth.api_key.as_str()),
            Self::BedrockApiKey(_) => None,
        }
    }

    /// Returns `Err` if token-backed auth is unavailable.
    ///
    /// Neither `ApiKey` nor `BedrockApiKey` auth carries token data; this always fails.
    pub fn get_token_data(&self) -> Result<TokenData, std::io::Error> {
        let auth_dot_json: Option<AuthDotJson> = self.get_current_auth_json();
        match auth_dot_json {
            Some(AuthDotJson {
                tokens: Some(tokens),
                last_refresh: Some(_),
                ..
            }) => Ok(tokens),
            _ => Err(std::io::Error::other("Token data is not available.")),
        }
    }

    /// Returns the token string used for bearer authentication.
    pub fn get_token(&self) -> Result<String, std::io::Error> {
        match self {
            Self::ApiKey(auth) => Ok(auth.api_key.clone()),
            Self::BedrockApiKey(_) => Err(std::io::Error::other(
                "Bedrock API key auth does not expose a Ody bearer token",
            )),
        }
    }

    /// Returns `None` if Ody backend auth does not expose an account id.
    pub fn get_account_id(&self) -> Option<String> {
        self.get_current_token_data().and_then(|t| t.account_id)
    }

    /// Returns false if Ody backend auth omits the FedRAMP claim.
    pub fn is_fedramp_account(&self) -> bool {
        self.get_current_token_data()
            .is_some_and(|t| t.id_token.is_fedramp_account())
    }

    /// Returns `None` if Ody backend auth does not expose an account email.
    pub fn get_account_email(&self) -> Option<String> {
        self.get_current_token_data().and_then(|t| t.id_token.email)
    }

    /// Returns `None` if Ody backend auth does not expose a ChatGPT user id.
    pub fn get_chatgpt_user_id(&self) -> Option<String> {
        self.get_current_token_data()
            .and_then(|t| t.id_token.chatgpt_user_id)
    }

    /// Account-facing plan classification derived from the current auth.
    /// Returns a high-level `AccountPlanType` (e.g., Free/Plus/Pro/Team/…)
    /// for UI or product decisions based on the user's subscription.
    pub fn account_plan_type(&self) -> Option<AccountPlanType> {
        self.get_current_token_data().map(|t| {
            t.id_token
                .chatgpt_plan_type
                .map(AccountPlanType::from)
                .unwrap_or(AccountPlanType::Unknown)
        })
    }

    pub fn is_workspace_account(&self) -> bool {
        self.account_plan_type()
            .is_some_and(AccountPlanType::is_workspace_account)
    }

    /// Returns `None`. Neither `ApiKey` nor `BedrockApiKey` auth carries a cached
    /// token-backed `auth.json` snapshot.
    fn get_current_auth_json(&self) -> Option<AuthDotJson> {
        None
    }

    /// Returns `None`. Neither `ApiKey` nor `BedrockApiKey` auth carries a cached
    /// token-backed `auth.json` snapshot.
    fn get_current_token_data(&self) -> Option<TokenData> {
        self.get_current_auth_json().and_then(|t| t.tokens)
    }

    /// Consider this private to integration tests.
    pub fn create_dummy_api_key_auth_for_testing() -> Self {
        let dummy_auth_id = NEXT_DUMMY_AUTH_ID.fetch_add(1, Ordering::Relaxed);
        Self::from_api_key(&format!("sk-dummy-chatgpt-auth-{dummy_auth_id}"))
    }

    pub fn from_api_key(api_key: &str) -> Self {
        Self::ApiKey(ApiKeyAuth {
            api_key: api_key.to_owned(),
        })
    }
}

pub const OPENAI_API_KEY_ENV_VAR: &str = "OPENAI_API_KEY";
pub const ODY_API_KEY_ENV_VAR: &str = "ODY_API_KEY";
pub const ODY_ACCESS_TOKEN_ENV_VAR: &str = "ODY_ACCESS_TOKEN";

pub fn read_odysseythink_api_key_from_env() -> Option<String> {
    env::var(OPENAI_API_KEY_ENV_VAR)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub fn read_ody_api_key_from_env() -> Option<String> {
    read_non_empty_env_var(ODY_API_KEY_ENV_VAR)
}

pub fn read_ody_access_token_from_env() -> Option<String> {
    read_non_empty_env_var(ODY_ACCESS_TOKEN_ENV_VAR)
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

pub async fn logout_with_revoke(
    ody_home: &Path,
    auth_credentials_store_mode: AuthCredentialsStoreMode,
    keyring_backend_kind: AuthKeyringBackendKind,
    auth_route_config: Option<&AuthRouteConfig>,
) -> std::io::Result<bool> {
    let auth_dot_json = match load_auth_dot_json(
        ody_home,
        auth_credentials_store_mode,
        keyring_backend_kind,
    ) {
        Ok(auth_dot_json) => auth_dot_json,
        Err(err) => {
            tracing::warn!("failed to load stored auth during logout: {err}");
            None
        }
    };
    if let Err(err) = revoke_auth_tokens(auth_dot_json.as_ref(), auth_route_config).await {
        tracing::warn!("failed to revoke auth tokens during logout: {err}");
    }
    logout_all_stores(
        ody_home,
        auth_credentials_store_mode,
        keyring_backend_kind,
    )
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
        tokens: None,
        last_refresh: None,
        bedrock_api_key: None,
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
    pub chatgpt_base_url: Option<String>,
    pub forced_chatgpt_workspace_id: Option<Vec<String>>,
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
    // External auth tokens live in the ephemeral store, but persistent auth may still exist
    // from earlier logins. Clear both so a forced logout truly removes all active auth.
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

    // External auth tokens live in the in-memory (ephemeral) store. Always check this
    // first so external auth takes precedence over any persisted credentials.
    let ephemeral_storage = create_auth_storage(
        ody_home.to_path_buf(),
        AuthCredentialsStoreMode::Ephemeral,
        AuthKeyringBackendKind::default(),
    );
    if let Some(auth_dot_json) = ephemeral_storage.load()? {
        let auth = OdyAuth::from_auth_dot_json(
            ody_home,
            auth_dot_json,
            AuthCredentialsStoreMode::Ephemeral,
        )
        .await?;
        return Ok(Some(auth));
    }

    // If the caller explicitly requested ephemeral auth, there is no persisted fallback.
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

    let auth =
        OdyAuth::from_auth_dot_json(ody_home, auth_dot_json, auth_credentials_store_mode).await?;
    Ok(Some(auth))
}

// Shared constant used by the (best-effort) OAuth token revocation flow in `revoke.rs`.
// Stale `auth.json` files written by older Ody builds may still carry ChatGPT OAuth
// tokens; revocation reads them directly rather than through `OdyAuth`.
pub const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

pub fn oauth_client_id() -> String {
    std::env::var(CLIENT_ID_OVERRIDE_ENV_VAR)
        .ok()
        .filter(|client_id| !client_id.trim().is_empty())
        .unwrap_or_else(|| CLIENT_ID.to_string())
}

/// Internal cached auth state.
#[derive(Clone)]
struct CachedAuth {
    auth: Option<OdyAuth>,
    /// Permanent refresh failure cached for the current auth snapshot so
    /// later refresh attempts for the same credentials fail fast without network.
    permanent_refresh_failure: Option<AuthScopedRefreshFailure>,
}

#[derive(Clone)]
struct AuthScopedRefreshFailure {
    auth: OdyAuth,
    error: RefreshTokenFailedError,
}

impl Debug for CachedAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachedAuth")
            .field(
                "auth_mode",
                &self.auth.as_ref().map(OdyAuth::api_auth_mode),
            )
            .field(
                "permanent_refresh_failure",
                &self
                    .permanent_refresh_failure
                    .as_ref()
                    .map(|failure| failure.error.reason),
            )
            .finish()
    }
}

enum UnauthorizedRecoveryStep {
    Reload,
    RefreshToken,
    ExternalRefresh,
    Done,
}

enum ReloadOutcome {
    /// Reload was performed and the cached auth changed
    ReloadedChanged,
    /// Reload was performed and the cached auth remained the same
    ReloadedNoChange,
    /// Reload was skipped (missing or mismatched account id)
    Skipped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UnauthorizedRecoveryMode {
    Managed,
    External,
}

// UnauthorizedRecovery is a state machine that handles an attempt to refresh the authentication when requests
// to API fail with 401 status code.
// The client calls next() every time it encounters a 401 error, one time per retry.
//
// Neither `ApiKey` nor `BedrockApiKey` managed auth supports refresh, so the managed
// recovery path is effectively inert; it exists so future managed auth methods can plug
// in without changing this state machine's shape.
//
// For external auth sources, UnauthorizedRecovery retries once by asking the configured
// `ExternalAuth` provider to refresh (e.g. rerunning a provider auth command without
// touching disk).
pub struct UnauthorizedRecovery {
    manager: Arc<AuthManager>,
    step: UnauthorizedRecoveryStep,
    expected_account_id: Option<String>,
    mode: UnauthorizedRecoveryMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnauthorizedRecoveryStepResult {
    auth_state_changed: Option<bool>,
}

impl UnauthorizedRecoveryStepResult {
    pub fn auth_state_changed(&self) -> Option<bool> {
        self.auth_state_changed
    }
}

impl UnauthorizedRecovery {
    fn new(manager: Arc<AuthManager>) -> Self {
        let cached_auth = manager.auth_cached();
        let expected_account_id = cached_auth.as_ref().and_then(OdyAuth::get_account_id);
        let mode = if manager.has_external_api_key_auth() {
            UnauthorizedRecoveryMode::External
        } else {
            UnauthorizedRecoveryMode::Managed
        };
        let step = match mode {
            UnauthorizedRecoveryMode::Managed => UnauthorizedRecoveryStep::Reload,
            UnauthorizedRecoveryMode::External => UnauthorizedRecoveryStep::ExternalRefresh,
        };
        Self {
            manager,
            step,
            expected_account_id,
            mode,
        }
    }

    pub fn has_next(&self) -> bool {
        if self.manager.has_external_api_key_auth() {
            return !matches!(self.step, UnauthorizedRecoveryStep::Done);
        }

        // Neither ApiKey nor BedrockApiKey managed auth supports unauthorized recovery.
        false
    }

    pub fn unavailable_reason(&self) -> &'static str {
        if self.manager.has_external_api_key_auth() {
            return if matches!(self.step, UnauthorizedRecoveryStep::Done) {
                "recovery_exhausted"
            } else {
                "ready"
            };
        }

        "not_refreshable_auth"
    }

    pub fn mode_name(&self) -> &'static str {
        match self.mode {
            UnauthorizedRecoveryMode::Managed => "managed",
            UnauthorizedRecoveryMode::External => "external",
        }
    }

    pub fn step_name(&self) -> &'static str {
        match self.step {
            UnauthorizedRecoveryStep::Reload => "reload",
            UnauthorizedRecoveryStep::RefreshToken => "refresh_token",
            UnauthorizedRecoveryStep::ExternalRefresh => "external_refresh",
            UnauthorizedRecoveryStep::Done => "done",
        }
    }

    pub async fn next(&mut self) -> Result<UnauthorizedRecoveryStepResult, RefreshTokenError> {
        if !self.has_next() {
            return Err(RefreshTokenError::Permanent(RefreshTokenFailedError::new(
                RefreshTokenFailedReason::Other,
                "No more recovery steps available.",
            )));
        }

        match self.step {
            UnauthorizedRecoveryStep::Reload => {
                match self
                    .manager
                    .reload_if_account_id_matches(self.expected_account_id.as_deref())
                    .await
                {
                    ReloadOutcome::ReloadedChanged => {
                        self.step = UnauthorizedRecoveryStep::RefreshToken;
                        return Ok(UnauthorizedRecoveryStepResult {
                            auth_state_changed: Some(true),
                        });
                    }
                    ReloadOutcome::ReloadedNoChange => {
                        self.step = UnauthorizedRecoveryStep::RefreshToken;
                        return Ok(UnauthorizedRecoveryStepResult {
                            auth_state_changed: Some(false),
                        });
                    }
                    ReloadOutcome::Skipped => {
                        self.step = UnauthorizedRecoveryStep::Done;
                        return Err(RefreshTokenError::Permanent(RefreshTokenFailedError::new(
                            RefreshTokenFailedReason::Other,
                            REFRESH_TOKEN_ACCOUNT_MISMATCH_MESSAGE.to_string(),
                        )));
                    }
                }
            }
            UnauthorizedRecoveryStep::RefreshToken => {
                self.manager.refresh_token_from_authority().await?;
                self.step = UnauthorizedRecoveryStep::Done;
                return Ok(UnauthorizedRecoveryStepResult {
                    auth_state_changed: Some(true),
                });
            }
            UnauthorizedRecoveryStep::ExternalRefresh => {
                self.manager
                    .refresh_external_auth(ExternalAuthRefreshReason::Unauthorized)
                    .await?;
                self.step = UnauthorizedRecoveryStep::Done;
                return Ok(UnauthorizedRecoveryStepResult {
                    auth_state_changed: Some(true),
                });
            }
            UnauthorizedRecoveryStep::Done => {}
        }
        Ok(UnauthorizedRecoveryStepResult {
            auth_state_changed: None,
        })
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
    forced_chatgpt_workspace_id: RwLock<Option<Vec<String>>>,
    chatgpt_base_url: Option<String>,
    refresh_lock: Semaphore,
    external_auth: RwLock<Option<Arc<dyn ExternalAuth>>>,
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

    /// Returns the workspace IDs that ChatGPT auth should be restricted to, if any.
    fn forced_chatgpt_workspace_id(&self) -> Option<Vec<String>>;

    /// Returns the ChatGPT backend base URL used for first-party backend authorization.
    fn chatgpt_base_url(&self) -> String;

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
            .field(
                "forced_chatgpt_workspace_id",
                &self.forced_chatgpt_workspace_id,
            )
            .field("chatgpt_base_url", &self.chatgpt_base_url)
            .field("auth_route_config", &self.auth_route_config)
            .field("has_external_auth", &self.has_external_auth())
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
        forced_chatgpt_workspace_id: Option<Vec<String>>,
        chatgpt_base_url: Option<String>,
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
                permanent_refresh_failure: None,
            }),
            auth_change_tx,
            enable_ody_api_key_env,
            auth_credentials_store_mode,
            keyring_backend_kind,
            forced_chatgpt_workspace_id: RwLock::new(forced_chatgpt_workspace_id),
            chatgpt_base_url,
            refresh_lock: Semaphore::new(/*permits*/ 1),
            external_auth: RwLock::new(None),
            auth_route_config,
        }
    }

    /// Create an AuthManager with a specific OdyAuth, for testing only.
    pub fn from_auth_for_testing(auth: OdyAuth) -> Arc<Self> {
        let cached = CachedAuth {
            auth: Some(auth),
            permanent_refresh_failure: None,
        };
        let (auth_change_tx, _auth_change_rx) = watch::channel(0);

        Arc::new(Self {
            ody_home: PathBuf::from("non-existent"),
            inner: RwLock::new(cached),
            auth_change_tx,
            enable_ody_api_key_env: false,
            auth_credentials_store_mode: AuthCredentialsStoreMode::File,
            keyring_backend_kind: AuthKeyringBackendKind::default(),
            forced_chatgpt_workspace_id: RwLock::new(None),
            chatgpt_base_url: None,
            refresh_lock: Semaphore::new(/*permits*/ 1),
            external_auth: RwLock::new(None),
            auth_route_config: None,
        })
    }

    /// Create an AuthManager with a specific OdyAuth and ody home, for testing only.
    pub fn from_auth_for_testing_with_home(auth: OdyAuth, ody_home: PathBuf) -> Arc<Self> {
        let cached = CachedAuth {
            auth: Some(auth),
            permanent_refresh_failure: None,
        };
        let (auth_change_tx, _auth_change_rx) = watch::channel(0);
        Arc::new(Self {
            ody_home,
            inner: RwLock::new(cached),
            auth_change_tx,
            enable_ody_api_key_env: false,
            auth_credentials_store_mode: AuthCredentialsStoreMode::File,
            keyring_backend_kind: AuthKeyringBackendKind::default(),
            forced_chatgpt_workspace_id: RwLock::new(None),
            chatgpt_base_url: None,
            refresh_lock: Semaphore::new(/*permits*/ 1),
            external_auth: RwLock::new(None),
            auth_route_config: None,
        })
    }

    pub fn external_bearer_only(config: ModelProviderAuthInfo) -> Arc<Self> {
        let (auth_change_tx, _auth_change_rx) = watch::channel(0);
        Arc::new(Self {
            ody_home: PathBuf::from("non-existent"),
            inner: RwLock::new(CachedAuth {
                auth: None,
                permanent_refresh_failure: None,
            }),
            auth_change_tx,
            enable_ody_api_key_env: false,
            auth_credentials_store_mode: AuthCredentialsStoreMode::File,
            keyring_backend_kind: AuthKeyringBackendKind::default(),
            forced_chatgpt_workspace_id: RwLock::new(None),
            chatgpt_base_url: None,
            refresh_lock: Semaphore::new(/*permits*/ 1),
            external_auth: RwLock::new(Some(
                Arc::new(BearerTokenRefresher::new(config)) as Arc<dyn ExternalAuth>
            )),
            auth_route_config: None,
        })
    }

    /// Current cached auth (clone) without attempting a refresh.
    pub fn auth_cached(&self) -> Option<OdyAuth> {
        self.inner.read().ok().and_then(|c| c.auth.clone())
    }

    /// Subscribes to cached auth changes that can affect request recovery.
    pub fn auth_change_receiver(&self) -> watch::Receiver<u64> {
        self.auth_change_tx.subscribe()
    }

    pub fn refresh_failure_for_auth(&self, auth: &OdyAuth) -> Option<RefreshTokenFailedError> {
        self.inner.read().ok().and_then(|cached| {
            cached
                .permanent_refresh_failure
                .as_ref()
                .filter(|failure| Self::auths_equal_for_refresh(Some(auth), Some(&failure.auth)))
                .map(|failure| failure.error.clone())
        })
    }

    /// Current cached auth (clone). May be `None` if not logged in or load failed.
    #[instrument(level = "trace", skip_all)]
    pub async fn auth(&self) -> Option<OdyAuth> {
        if let Some(auth) = self.resolve_external_api_key_auth().await {
            return Some(auth);
        }
        self.auth_cached()
    }

    /// Force a reload of the auth information from auth.json. Returns
    /// whether the auth value changed.
    pub async fn reload(&self) -> bool {
        tracing::info!("Reloading auth");
        let new_auth = self.load_auth_from_storage().await;
        self.set_cached_auth(new_auth)
    }

    async fn reload_if_account_id_matches(
        &self,
        expected_account_id: Option<&str>,
    ) -> ReloadOutcome {
        let expected_account_id = match expected_account_id {
            Some(account_id) => account_id,
            None => {
                tracing::info!("Skipping auth reload because no account id is available.");
                return ReloadOutcome::Skipped;
            }
        };

        let new_auth = self.load_auth_from_storage().await;
        let new_account_id = new_auth.as_ref().and_then(OdyAuth::get_account_id);

        if new_account_id.as_deref() != Some(expected_account_id) {
            let found_account_id = new_account_id.as_deref().unwrap_or("unknown");
            tracing::info!(
                "Skipping auth reload due to account id mismatch (expected: {expected_account_id}, found: {found_account_id})"
            );
            return ReloadOutcome::Skipped;
        }

        tracing::info!("Reloading auth for account {expected_account_id}");
        let cached_before_reload = self.auth_cached();
        let auth_changed =
            !Self::auths_equal_for_refresh(cached_before_reload.as_ref(), new_auth.as_ref());
        self.set_cached_auth(new_auth);
        if auth_changed {
            ReloadOutcome::ReloadedChanged
        } else {
            ReloadOutcome::ReloadedNoChange
        }
    }

    fn auths_equal_for_refresh(a: Option<&OdyAuth>, b: Option<&OdyAuth>) -> bool {
        match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => match (a, b) {
                (OdyAuth::ApiKey(_), OdyAuth::ApiKey(_)) => a.api_key() == b.api_key(),
                (OdyAuth::BedrockApiKey(a), OdyAuth::BedrockApiKey(b)) => a == b,
                _ => false,
            },
            _ => false,
        }
    }

    fn auths_equal(a: Option<&OdyAuth>, b: Option<&OdyAuth>) -> bool {
        match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => a == b,
            _ => false,
        }
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
            let auth_changed_for_refresh =
                !Self::auths_equal_for_refresh(previous, new_auth.as_ref());
            if auth_changed_for_refresh {
                guard.permanent_refresh_failure = None;
            }
            tracing::info!("Reloaded auth, changed: {changed}");
            guard.auth = new_auth;
            if auth_changed_for_refresh {
                self.auth_change_tx.send_modify(|revision| *revision += 1);
            }
            changed
        } else {
            false
        }
    }

    pub fn set_external_auth(&self, external_auth: Arc<dyn ExternalAuth>) {
        if let Ok(mut guard) = self.external_auth.write() {
            *guard = Some(external_auth);
        }
    }

    pub fn clear_external_auth(&self) {
        if let Ok(mut guard) = self.external_auth.write() {
            *guard = None;
        }
    }

    pub fn set_forced_chatgpt_workspace_id(&self, workspace_id: Option<Vec<String>>) {
        if let Ok(mut guard) = self.forced_chatgpt_workspace_id.write()
            && *guard != workspace_id
        {
            *guard = workspace_id;
        }
    }

    pub fn forced_chatgpt_workspace_id(&self) -> Option<Vec<String>> {
        self.forced_chatgpt_workspace_id
            .read()
            .ok()
            .and_then(|guard| guard.clone())
    }

    pub fn has_external_auth(&self) -> bool {
        self.external_auth().is_some()
    }

    pub fn is_external_chatgpt_auth_active(&self) -> bool {
        false
    }

    pub fn ody_api_key_env_enabled(&self) -> bool {
        self.enable_ody_api_key_env
    }

    /// Convenience constructor returning an `Arc` wrapper.
    pub async fn shared(
        ody_home: PathBuf,
        enable_ody_api_key_env: bool,
        auth_credentials_store_mode: AuthCredentialsStoreMode,
        forced_chatgpt_workspace_id: Option<Vec<String>>,
        chatgpt_base_url: Option<String>,
        keyring_backend_kind: AuthKeyringBackendKind,
        auth_route_config: Option<AuthRouteConfig>,
    ) -> Arc<Self> {
        Arc::new(
            Self::new(
                ody_home,
                enable_ody_api_key_env,
                auth_credentials_store_mode,
                forced_chatgpt_workspace_id,
                chatgpt_base_url,
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
            config.forced_chatgpt_workspace_id(),
            Some(config.chatgpt_base_url()),
            config.auth_keyring_backend_kind(),
            config.auth_route_config(),
        )
        .await
    }

    pub fn unauthorized_recovery(self: &Arc<Self>) -> UnauthorizedRecovery {
        UnauthorizedRecovery::new(Arc::clone(self))
    }

    fn external_auth(&self) -> Option<Arc<dyn ExternalAuth>> {
        self.external_auth
            .read()
            .ok()
            .and_then(|guard| guard.as_ref().cloned())
    }

    fn external_auth_mode(&self) -> Option<AuthMode> {
        self.external_auth()
            .as_ref()
            .map(|external_auth| external_auth.auth_mode())
    }

    fn has_external_api_key_auth(&self) -> bool {
        self.external_auth_mode() == Some(AuthMode::ApiKey)
    }

    async fn resolve_external_api_key_auth(&self) -> Option<OdyAuth> {
        if !self.has_external_api_key_auth() {
            return None;
        }

        let external_auth = self.external_auth()?;

        match external_auth.resolve().await {
            Ok(Some(tokens)) => Some(OdyAuth::from_api_key(&tokens.access_token)),
            Ok(None) => None,
            Err(err) => {
                tracing::error!("Failed to resolve external API key auth: {err}");
                None
            }
        }
    }

    /// Attempt to refresh the token. Neither `ApiKey` nor `BedrockApiKey` managed
    /// auth supports refresh, so this is a no-op for managed auth; it exists for
    /// API symmetry with the unauthorized-recovery state machine.
    pub async fn refresh_token(&self) -> Result<(), RefreshTokenError> {
        let _refresh_guard = self.refresh_lock.acquire().await.map_err(|_| {
            RefreshTokenError::Permanent(RefreshTokenFailedError::new(
                RefreshTokenFailedReason::Other,
                REFRESH_TOKEN_UNKNOWN_MESSAGE.to_string(),
            ))
        })?;
        Ok(())
    }

    /// Attempt to refresh the current auth token from the authority that issued
    /// the token. Neither `ApiKey` nor `BedrockApiKey` managed auth supports this.
    pub async fn refresh_token_from_authority(&self) -> Result<(), RefreshTokenError> {
        let _refresh_guard = self.refresh_lock.acquire().await.map_err(|_| {
            RefreshTokenError::Permanent(RefreshTokenFailedError::new(
                RefreshTokenFailedReason::Other,
                REFRESH_TOKEN_UNKNOWN_MESSAGE.to_string(),
            ))
        })?;
        Ok(())
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

    pub async fn logout_with_revoke(&self) -> std::io::Result<bool> {
        let auth_dot_json = self
            .auth_cached()
            .and_then(|auth| auth.get_current_auth_json());
        if let Err(err) =
            revoke_auth_tokens(auth_dot_json.as_ref(), self.auth_route_config.as_ref()).await
        {
            tracing::warn!("failed to revoke auth tokens during logout: {err}");
        }
        let result = logout_all_stores(
            &self.ody_home,
            self.auth_credentials_store_mode,
            self.keyring_backend_kind,
        )?;
        // Always reload to clear any cached auth (even if file absent).
        self.reload().await;
        Ok(result)
    }

    pub fn get_api_auth_mode(&self) -> Option<ApiAuthMode> {
        if self.has_external_api_key_auth() {
            return Some(ApiAuthMode::ApiKey);
        }
        self.auth_cached().as_ref().map(OdyAuth::api_auth_mode)
    }

    pub fn auth_mode(&self) -> Option<AuthMode> {
        if self.has_external_api_key_auth() {
            return Some(AuthMode::ApiKey);
        }
        self.auth_cached().as_ref().map(OdyAuth::auth_mode)
    }

    pub fn current_auth_uses_ody_backend(&self) -> bool {
        false
    }

    async fn refresh_external_auth(
        &self,
        reason: ExternalAuthRefreshReason,
    ) -> Result<(), RefreshTokenError> {
        let Some(external_auth) = self.external_auth() else {
            return Err(RefreshTokenError::Transient(std::io::Error::other(
                "external auth is not configured",
            )));
        };
        let previous_account_id = self
            .auth_cached()
            .as_ref()
            .and_then(OdyAuth::get_account_id);
        let context = ExternalAuthRefreshContext {
            reason,
            previous_account_id,
        };

        external_auth
            .refresh(context)
            .await
            .map_err(RefreshTokenError::Transient)?;

        if external_auth.auth_mode() != AuthMode::ApiKey {
            return Err(RefreshTokenError::Transient(std::io::Error::other(
                "external auth refresh is only supported for API key auth",
            )));
        }
        Ok(())
    }
}


