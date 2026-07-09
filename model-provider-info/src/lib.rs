//! Registry of model providers supported by Ody.
//!
//! Providers can be defined in two places:
//!   1. Built-in defaults compiled into the binary so Ody works out-of-the-box.
//!   2. User-defined entries inside `~/.ody-code/config.toml` under the `model_providers`
//!      key. These override or extend the defaults at runtime.

use ody_api::Provider as ApiProvider;
use ody_api::RetryConfig as ApiRetryConfig;
use ody_api::is_azure_responses_provider;
use ody_app_server_protocol::AuthMode;
use ody_protocol::config_types::ModelProviderAuthInfo;
use ody_protocol::error::OdyErr;
use ody_protocol::error::EnvVarError;
use ody_protocol::error::Result as OdyResult;
use http::HeaderMap;
use http::header::HeaderName;
use http::header::HeaderValue;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

const DEFAULT_STREAM_IDLE_TIMEOUT_MS: u64 = 300_000;
const DEFAULT_STREAM_MAX_RETRIES: u64 = 5;
const DEFAULT_REQUEST_MAX_RETRIES: u64 = 4;
pub const DEFAULT_WEBSOCKET_CONNECT_TIMEOUT_MS: u64 = 15_000;
/// Hard cap for user-configured `stream_max_retries`.
const MAX_STREAM_MAX_RETRIES: u64 = 100;
/// Hard cap for user-configured `request_max_retries`.
const MAX_REQUEST_MAX_RETRIES: u64 = 100;


// OpenAI-compatible third-party Chat Completions providers.
const KIMI_PROVIDER_NAME: &str = "Kimi";
pub const KIMI_PROVIDER_ID: &str = "kimi";
pub const KIMI_DEFAULT_BASE_URL: &str = "https://api.moonshot.ai/v1";
const KIMI_ENV_KEY: &str = "KIMI_API_KEY";

const DEEPSEEK_PROVIDER_NAME: &str = "DeepSeek";
pub const DEEPSEEK_PROVIDER_ID: &str = "deepseek";
pub const DEEPSEEK_DEFAULT_BASE_URL: &str = "https://api.deepseek.com/v1";
const DEEPSEEK_ENV_KEY: &str = "DEEPSEEK_API_KEY";

const GLM_PROVIDER_NAME: &str = "GLM";
pub const GLM_PROVIDER_ID: &str = "glm";
pub const GLM_DEFAULT_BASE_URL: &str = "https://open.bigmodel.cn/api/paas/v4";
const GLM_ENV_KEY: &str = "GLM_API_KEY";

/// Wire protocol that the provider speaks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WireApi {
    /// The Responses API exposed by OpenAI at `/v1/responses`.
    #[default]
    Responses,
    /// The Chat Completions API exposed at `/v1/chat/completions`. Used by
    /// OpenAI-compatible third-party providers such as Kimi, DeepSeek and GLM.
    Chat,
    /// Anthropic Messages API at `/v1/messages`.
    AnthropicMessages,
    /// Google GenAI API.
    #[schemars(rename = "google_genai")]
    GoogleGenAI,
}

impl fmt::Display for WireApi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Responses => "responses",
            Self::Chat => "chat",
            Self::AnthropicMessages => "anthropic_messages",
            Self::GoogleGenAI => "google_genai",
        };
        f.write_str(value)
    }
}

impl<'de> Deserialize<'de> for WireApi {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "responses" => Ok(Self::Responses),
            "chat" => Ok(Self::Chat),
            "anthropic_messages" => Ok(Self::AnthropicMessages),
            "google_genai" => Ok(Self::GoogleGenAI),
            _ => Err(serde::de::Error::unknown_variant(
                &value,
                &["responses", "chat", "anthropic_messages", "google_genai", "local"],
            )),
        }
    }
}

/// Provider-level feature capabilities.
///
/// These are upper-bound flags that core/TUI use to decide which features a
/// provider can expose. Defaults are conservative (all false) so unknown
/// providers do not accidentally claim advanced features.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ProviderCapabilities {
    /// Whether the provider can use WebSocket transport for streaming.
    #[serde(default)]
    pub supports_websockets: bool,

    /// Whether the provider supports remote context compaction.
    #[serde(default)]
    pub supports_remote_compaction: bool,

    /// Whether the provider exposes namespaced / server-side tools.
    #[serde(default)]
    pub namespace_tools: bool,

    /// Whether the provider supports server-side image generation.
    #[serde(default)]
    pub image_generation: bool,

    /// Whether the provider supports server-side web search.
    #[serde(default)]
    pub web_search: bool,

    /// Whether the provider uses command-backed dynamic auth.
    #[serde(default)]
    pub command_auth: bool,

    /// Whether requests should include attestation headers.
    #[serde(default)]
    pub attestation: bool,
}

/// Provider capability fallback when no user config or built-in catalog value exists.
pub fn default_provider_capabilities_for_wire_api(wire_api: WireApi) -> ProviderCapabilities {
    match wire_api {
        WireApi::Responses => ProviderCapabilities {
            supports_websockets: true,
            supports_remote_compaction: true,
            namespace_tools: true,
            image_generation: true,
            web_search: true,
            command_auth: false,
            attestation: false,
        },
        WireApi::Chat | WireApi::AnthropicMessages | WireApi::GoogleGenAI => ProviderCapabilities {
            supports_websockets: false,
            supports_remote_compaction: false,
            namespace_tools: false,
            image_generation: false,
            web_search: false,
            command_auth: false,
            attestation: false,
        },
    }
}

/// Serializable representation of a provider definition.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ModelProviderInfo {
    /// Friendly display name.
    #[serde(default)]
    pub name: String,
    /// Base URL for the provider's OpenAI-compatible API.
    pub base_url: Option<String>,
    /// Environment variable that stores the user's API key for this provider.
    pub env_key: Option<String>,

    /// Optional instructions to help the user get a valid value for the
    /// variable and set it.
    pub env_key_instructions: Option<String>,
    /// Value to use with `Authorization: Bearer <token>` header. Use of this
    /// config is discouraged in favor of `env_key` for security reasons, but
    /// this may be necessary when using this programmatically.
    pub experimental_bearer_token: Option<String>,
    /// Command-backed bearer-token configuration for this provider.
    pub auth: Option<ModelProviderAuthInfo>,
    /// Which wire protocol this provider expects.
    #[serde(default)]
    pub wire_api: WireApi,
    /// Optional query parameters to append to the base URL.
    pub query_params: Option<HashMap<String, String>>,
    /// Additional HTTP headers to include in requests to this provider where
    /// the (key, value) pairs are the header name and value.
    pub http_headers: Option<HashMap<String, String>>,
    /// Optional HTTP headers to include in requests to this provider where the
    /// (key, value) pairs are the header name and _environment variable_ whose
    /// value should be used. If the environment variable is not set, or the
    /// value is empty, the header will not be included in the request.
    pub env_http_headers: Option<HashMap<String, String>>,
    /// Maximum number of times to retry a failed HTTP request to this provider.
    pub request_max_retries: Option<u64>,
    /// Number of times to retry reconnecting a dropped streaming response before failing.
    pub stream_max_retries: Option<u64>,
    /// Idle timeout (in milliseconds) to wait for activity on a streaming response before treating
    /// the connection as lost.
    pub stream_idle_timeout_ms: Option<u64>,
    /// Maximum time (in milliseconds) to wait for a websocket connection attempt before treating
    /// it as failed.
    pub websocket_connect_timeout_ms: Option<u64>,
    /// Whether this provider supports the Responses API WebSocket transport.
    #[serde(default)]
    pub supports_websockets: bool,
    /// Provider-level feature capabilities.
    #[serde(default)]
    pub capabilities: ProviderCapabilities,
}

impl ModelProviderInfo {
    pub fn validate(&self) -> std::result::Result<(), String> {
        let Some(auth) = self.auth.as_ref() else {
            return Ok(());
        };

        if auth.command.trim().is_empty() {
            return Err("provider auth.command must not be empty".to_string());
        }

        let mut conflicts = Vec::new();
        if self.env_key.is_some() {
            conflicts.push("env_key");
        }
        if self.experimental_bearer_token.is_some() {
            conflicts.push("experimental_bearer_token");
        }

        if conflicts.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "provider auth cannot be combined with {}",
                conflicts.join(", ")
            ))
        }
    }

    fn build_header_map(&self) -> OdyResult<HeaderMap> {
        let capacity = self.http_headers.as_ref().map_or(0, HashMap::len)
            + self.env_http_headers.as_ref().map_or(0, HashMap::len);
        let mut headers = HeaderMap::with_capacity(capacity);
        if let Some(extra) = &self.http_headers {
            for (k, v) in extra {
                if let (Ok(name), Ok(value)) = (HeaderName::try_from(k), HeaderValue::try_from(v)) {
                    headers.insert(name, value);
                }
            }
        }

        if let Some(env_headers) = &self.env_http_headers {
            for (header, env_var) in env_headers {
                if let Ok(val) = std::env::var(env_var)
                    && !val.trim().is_empty()
                    && let (Ok(name), Ok(value)) =
                        (HeaderName::try_from(header), HeaderValue::try_from(val))
                {
                    headers.insert(name, value);
                }
            }
        }

        Ok(headers)
    }

    pub fn to_api_provider(&self, _auth_mode: Option<AuthMode>) -> OdyResult<ApiProvider> {
        let default_base_url = "https://api.openai.com/v1";
        let base_url = self
            .base_url
            .clone()
            .unwrap_or_else(|| default_base_url.to_string());

        let headers = self.build_header_map()?;
        let retry = ApiRetryConfig {
            max_attempts: self.request_max_retries(),
            base_delay: Duration::from_millis(200),
            retry_429: false,
            retry_5xx: true,
            retry_transport: true,
        };

        Ok(ApiProvider {
            name: self.name.clone(),
            base_url,
            query_params: self.query_params.clone(),
            headers,
            retry,
            stream_idle_timeout: self.stream_idle_timeout(),
        })
    }

    /// If `env_key` is Some, returns the API key for this provider if present
    /// (and non-empty) in the environment. If `env_key` is required but
    /// cannot be found, returns an error.
    pub fn api_key(&self) -> OdyResult<Option<String>> {
        match &self.env_key {
            Some(env_key) => {
                let api_key = std::env::var(env_key)
                    .ok()
                    .filter(|v| !v.trim().is_empty())
                    .ok_or_else(|| {
                        OdyErr::EnvVar(EnvVarError {
                            var: env_key.clone(),
                            instructions: self.env_key_instructions.clone(),
                        })
                    })?;
                Ok(Some(api_key))
            }
            None => Ok(None),
        }
    }

    /// Effective maximum number of request retries for this provider.
    pub fn request_max_retries(&self) -> u64 {
        self.request_max_retries
            .unwrap_or(DEFAULT_REQUEST_MAX_RETRIES)
            .min(MAX_REQUEST_MAX_RETRIES)
    }

    /// Effective maximum number of stream reconnection attempts for this provider.
    pub fn stream_max_retries(&self) -> u64 {
        self.stream_max_retries
            .unwrap_or(DEFAULT_STREAM_MAX_RETRIES)
            .min(MAX_STREAM_MAX_RETRIES)
    }

    /// Effective idle timeout for streaming responses.
    pub fn stream_idle_timeout(&self) -> Duration {
        self.stream_idle_timeout_ms
            .map(Duration::from_millis)
            .unwrap_or(Duration::from_millis(DEFAULT_STREAM_IDLE_TIMEOUT_MS))
    }

    /// Effective timeout for websocket connect attempts.
    pub fn websocket_connect_timeout(&self) -> Duration {
        self.websocket_connect_timeout_ms
            .map(Duration::from_millis)
            .unwrap_or(Duration::from_millis(DEFAULT_WEBSOCKET_CONNECT_TIMEOUT_MS))
    }

    /// Matches a known third-party Chat provider by name (case-insensitive, plus
    /// common aliases) or by a substring of its base URL. This mirrors the
    /// dialect detection in `ody-api`'s `ChatVendor::from_provider`, so a provider
    /// configured as `kimi`/`Moonshot`/etc. still resolves to its bundled model
    /// catalog (and therefore a real context window) instead of falling back to
    /// synthetic metadata.
    fn matches_provider(&self, names: &[&str], base_url_needles: &[&str]) -> bool {
        let name = self.name.to_ascii_lowercase();
        if names.iter().any(|candidate| name == *candidate) {
            return true;
        }
        if let Some(base_url) = self.base_url.as_deref() {
            let base_url = base_url.to_ascii_lowercase();
            if base_url_needles
                .iter()
                .any(|needle| base_url.contains(needle))
            {
                return true;
            }
        }
        false
    }

    pub fn is_kimi(&self) -> bool {
        // Kimi is reachable at two hosts: the public `api.moonshot.ai` and the
        // coding endpoint `api.kimi.com`. Match both so detection does not rely
        // solely on the display name resolving to "Kimi".
        self.matches_provider(&["kimi", "moonshot"], &["moonshot", "kimi.com"])
    }

    pub fn is_deepseek(&self) -> bool {
        self.matches_provider(&["deepseek"], &["deepseek"])
    }

    pub fn is_glm(&self) -> bool {
        self.matches_provider(&["glm", "zhipu", "bigmodel"], &["bigmodel"])
    }

    /// Whether this provider speaks the Chat Completions wire protocol.
    pub fn is_chat_completions(&self) -> bool {
        self.wire_api == WireApi::Chat
    }

    pub fn supports_remote_compaction(&self) -> bool {
        // The capability matrix is the authoritative source; legacy name/base_url
        // detection remains as a compatibility fallback for configs that predate
        // the `capabilities` field.
        self.capabilities.supports_remote_compaction
            || is_azure_responses_provider(&self.name, self.base_url.as_deref())
    }

    pub fn has_command_auth(&self) -> bool {
        self.auth.is_some()
    }

    /// Backfill capabilities from the wire-api defaults when the provider has
    /// not explicitly configured any capabilities (i.e. the value is the
    /// all-false default). This gives user-defined and converted providers
    /// sensible defaults without overriding explicit choices.
    pub fn normalize_capabilities(&mut self) {
        if self.capabilities == ProviderCapabilities::default() {
            self.capabilities = default_provider_capabilities_for_wire_api(self.wire_api);
        }
    }
}

/// Built-in default provider list.
pub fn built_in_model_providers() -> HashMap<String, ModelProviderInfo> {
    // We do not want to be in the business of adjucating which third-party
    // providers are bundled with Ody CLI, so we only include the OpenAI. 
    // Users are encouraged to add to
    // `model_providers` in config.toml to add their own providers.
    [
        // OpenAI-compatible third-party Chat Completions providers.
        (KIMI_PROVIDER_ID, create_kimi_provider()),
        (DEEPSEEK_PROVIDER_ID, create_deepseek_provider()),
        (GLM_PROVIDER_ID, create_glm_provider()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}

/// Merge configured providers into the built-in provider catalog.
///
/// Configured providers extend the built-in set. Built-in providers are not
/// overridable. After merging, all providers are normalized so their
/// capability matrices reflect their wire API and other declared fields.
pub fn merge_configured_model_providers(
    mut model_providers: HashMap<String, ModelProviderInfo>,
    configured_model_providers: HashMap<String, ModelProviderInfo>,
) -> Result<HashMap<String, ModelProviderInfo>, String> {
    for (key, provider) in configured_model_providers {
        model_providers.entry(key).or_insert(provider);
    }
    for provider in model_providers.values_mut() {
        provider.normalize_capabilities();
    }

    Ok(model_providers)
}

/// Build an OpenAI-compatible Chat Completions provider (Kimi / DeepSeek / GLM).
fn create_chat_provider(
    name: &str,
    base_url: &str,
    env_key: &str,
    http_headers: Option<HashMap<String, String>>,
) -> ModelProviderInfo {
    ModelProviderInfo {
        name: name.into(),
        base_url: Some(base_url.into()),
        env_key: Some(env_key.into()),
        env_key_instructions: None,
        experimental_bearer_token: None,
        auth: None,
        wire_api: WireApi::Chat,
        query_params: None,
        http_headers,
        env_http_headers: None,
        request_max_retries: None,
        stream_max_retries: None,
        stream_idle_timeout_ms: None,
        websocket_connect_timeout_ms: None,
        supports_websockets: false,
        capabilities: ProviderCapabilities {
            supports_websockets: false,
            supports_remote_compaction: false,
            namespace_tools: false,
            image_generation: false,
            web_search: false,
            command_auth: false,
            attestation: false,
        },
    }
}

/// Product name Kimi's backend associates with the official coding CLI. Kimi's
/// `kimi-for-coding` plan is gated to this client, and the Moonshot backend is
/// believed to apply different serving policies when the presented identity is
/// not the official CLI. We therefore present the official identity so the
/// coding plan is served on the same path the official client gets.
///
/// NOTE: this deliberately mimics the official client. Keep `KIMI_CODE_CLI_VERSION`
/// in sync with the real `kimi-code-cli` release the account is entitled to.
const KIMI_CODE_CLI_PRODUCT: &str = "kimi-code-cli";
const KIMI_CODE_CLI_VERSION: &str = "0.8.0";

/// Device-identity headers Kimi expects from its coding CLI, sent alongside the
/// API key. The OAuth device flow is not implemented; these mirror the headers
/// the official client attaches to every inference request (`User-Agent` +
/// `X-Msh-*`) so the request is served on the official path.
fn kimi_http_headers() -> HashMap<String, String> {
    let info = os_info::get();
    let arch = info.architecture().unwrap_or("unknown");
    let os_version = info.version().to_string();
    let os_label = kimi_os_label(&info);

    let mut headers = HashMap::from([
        (
            "User-Agent".to_string(),
            format!("{KIMI_CODE_CLI_PRODUCT}/{KIMI_CODE_CLI_VERSION}"),
        ),
        ("X-Msh-Platform".to_string(), "kimi_code_cli".to_string()),
        (
            "X-Msh-Version".to_string(),
            KIMI_CODE_CLI_VERSION.to_string(),
        ),
        ("X-Msh-Device-Name".to_string(), kimi_device_name()),
        (
            "X-Msh-Device-Model".to_string(),
            format!("{os_label} {os_version} {arch}"),
        ),
        ("X-Msh-Os-Version".to_string(), os_version),
    ]);
    if let Some(device_id) = kimi_device_id() {
        headers.insert("X-Msh-Device-Id".to_string(), device_id);
    }
    headers
}

/// Human-facing OS label matching the official client's device-model string
/// (`macOS`/`Windows`/`Linux`) rather than `os_info`'s `Display` ("Mac OS").
fn kimi_os_label(info: &os_info::Info) -> &'static str {
    match info.os_type() {
        os_info::Type::Macos => "macOS",
        os_info::Type::Windows => "Windows",
        _ => "Linux",
    }
}

/// Best-effort ASCII hostname for `X-Msh-Device-Name`.
fn kimi_device_name() -> String {
    let raw = gethostname::gethostname().to_string_lossy().to_string();
    let cleaned: String = raw.chars().filter(|c| (' '..='~').contains(c)).collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Process-stable device id. Resolved once and cached so that (a) every request
/// in a run presents the same id even when the on-disk store cannot be written,
/// and (b) building the provider list is side-effect-free and deterministic on
/// repeated calls.
static KIMI_DEVICE_ID: std::sync::LazyLock<Option<String>> =
    std::sync::LazyLock::new(resolve_kimi_device_id);

fn kimi_device_id() -> Option<String> {
    KIMI_DEVICE_ID.clone()
}

/// Stable per-machine device id, persisted under `$ODY_HOME` (or
/// `~/.ody-code`). Mirrors the official client's `device_id` file so the value
/// is consistent across runs. Returns `None` only if no writable/home location
/// can be resolved.
fn resolve_kimi_device_id() -> Option<String> {
    let dir = kimi_identity_dir()?;
    let path = dir.join("kimi_device_id");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let trimmed = existing.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    // Best-effort persistence: the in-memory id is still usable if writes fail.
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(&path, &id);
    Some(id)
}

fn kimi_identity_dir() -> Option<std::path::PathBuf> {
    if let Ok(home) = std::env::var("ODY_HOME") {
        if !home.trim().is_empty() {
            return Some(std::path::PathBuf::from(home));
        }
    }
    dirs::home_dir().map(|home| home.join(".ody-code"))
}

pub fn create_kimi_provider() -> ModelProviderInfo {
    create_chat_provider(
        KIMI_PROVIDER_NAME,
        KIMI_DEFAULT_BASE_URL,
        KIMI_ENV_KEY,
        Some(kimi_http_headers()),
    )
}

pub fn create_deepseek_provider() -> ModelProviderInfo {
    create_chat_provider(
        DEEPSEEK_PROVIDER_NAME,
        DEEPSEEK_DEFAULT_BASE_URL,
        DEEPSEEK_ENV_KEY,
        None,
    )
}

pub fn create_glm_provider() -> ModelProviderInfo {
    create_chat_provider(GLM_PROVIDER_NAME, GLM_DEFAULT_BASE_URL, GLM_ENV_KEY, None)
}

#[cfg(test)]
#[path = "model_provider_info_tests.rs"]
mod tests;
