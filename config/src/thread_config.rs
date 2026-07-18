use std::collections::BTreeMap;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use ody_app_server_protocol::ConfigLayerSource;
use ody_model_provider_info::ModelProviderInfo;
use ody_utils_absolute_path::AbsolutePathBuf;
use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;
use toml::Value as TomlValue;

use crate::ConfigLayerEntry;
use crate::config_toml::OdyCodeProviderConfig;

mod remote;

pub use remote::RemoteThreadConfigLoader;

/// Context available to implementations when loading thread-scoped config.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ThreadConfigContext {
    pub thread_id: Option<String>,
    pub cwd: Option<AbsolutePathBuf>,
}

/// Config values owned by the service that starts or manages the session.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionThreadConfig {
    /// Legacy: use `default_provider` / `default_model` instead.
    pub model_provider: Option<String>,

    /// Legacy provider entries kept for compatibility.
    #[serde(default)]
    pub model_providers: HashMap<String, ModelProviderInfo>,

    /// Ody-code compatible default provider id.
    #[serde(default)]
    pub default_provider: Option<String>,

    /// Ody-code compatible default model in the form `provider_id/model_name`.
    #[serde(default)]
    pub default_model: Option<String>,

    /// Ody-code compatible provider entries keyed by provider alias.
    #[serde(default)]
    pub providers: HashMap<String, OdyCodeProviderConfig>,

    #[serde(default)]
    pub features: BTreeMap<String, bool>,
}

/// Config values owned by the authenticated user.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UserThreadConfig {}

/// A typed config payload paired with the authority that produced it.
#[derive(Clone, Debug, PartialEq)]
pub enum ThreadConfigSource {
    Session(SessionThreadConfig),
    User(UserThreadConfig),
}

/// Stable category for failures returned while loading thread config.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThreadConfigLoadErrorCode {
    Auth,
    Timeout,
    Parse,
    RequestFailed,
    Internal,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error("{message}")]
pub struct ThreadConfigLoadError {
    code: ThreadConfigLoadErrorCode,
    message: String,
    status_code: Option<u16>,
}

impl ThreadConfigLoadError {
    pub fn new(
        code: ThreadConfigLoadErrorCode,
        status_code: Option<u16>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            status_code,
        }
    }

    pub fn code(&self) -> ThreadConfigLoadErrorCode {
        self.code
    }

    pub fn status_code(&self) -> Option<u16> {
        self.status_code
    }
}

/// Loads typed config sources for a new thread.
///
/// Implementations should fetch only the source-specific config they own and
/// return typed payloads without applying precedence or merge rules. Callers
/// are responsible for resolving the returned sources into the effective
/// runtime config.
pub trait ThreadConfigLoader: Send + Sync {
    /// Load source-specific typed config.
    ///
    /// Implementations should keep this method focused on fetching and parsing
    /// their owned sources. Most callers should use [`Self::load_config_layers`]
    /// so precedence and merging continue through the ordinary config layer
    /// stack.
    fn load(
        &self,
        context: ThreadConfigContext,
    ) -> ThreadConfigLoaderFuture<'_, Vec<ThreadConfigSource>>;

    fn load_config_layers(
        &self,
        context: ThreadConfigContext,
    ) -> ThreadConfigLoaderFuture<'_, Vec<ConfigLayerEntry>> {
        Box::pin(async move {
            let sources = self.load(context).await?;
            sources
                .into_iter()
                .map(thread_config_source_to_layer)
                .collect::<Result<Vec<_>, _>>()
                .map(|layers| layers.into_iter().flatten().collect())
        })
    }
}

pub type ThreadConfigLoaderFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ThreadConfigLoadError>> + Send + 'a>>;

/// Loader backed by a static set of typed thread config sources.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StaticThreadConfigLoader {
    sources: Vec<ThreadConfigSource>,
}

impl StaticThreadConfigLoader {
    pub fn new(sources: Vec<ThreadConfigSource>) -> Self {
        Self { sources }
    }
}

impl ThreadConfigLoader for StaticThreadConfigLoader {
    fn load(
        &self,
        _context: ThreadConfigContext,
    ) -> ThreadConfigLoaderFuture<'_, Vec<ThreadConfigSource>> {
        Box::pin(async { Ok(self.sources.clone()) })
    }
}

/// Loader used when no external thread config source is configured.
#[derive(Clone, Debug, Default)]
pub struct NoopThreadConfigLoader;

impl ThreadConfigLoader for NoopThreadConfigLoader {
    fn load(
        &self,
        _context: ThreadConfigContext,
    ) -> ThreadConfigLoaderFuture<'_, Vec<ThreadConfigSource>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

fn thread_config_source_to_layer(
    source: ThreadConfigSource,
) -> Result<Option<ConfigLayerEntry>, ThreadConfigLoadError> {
    match source {
        ThreadConfigSource::Session(config) => {
            let config = session_thread_config_to_toml(config)?;
            if is_empty_table(&config) {
                Ok(None)
            } else {
                Ok(Some(ConfigLayerEntry::new(
                    ConfigLayerSource::SessionFlags,
                    config,
                )))
            }
        }
        // UserThreadConfig has no TOML-backed fields yet. When it grows one,
        // fold it into the existing user layer instead of adding another
        // ConfigLayerSource variant.
        ThreadConfigSource::User(_config) => Ok(None),
    }
}

fn is_empty_table(config: &TomlValue) -> bool {
    config.as_table().is_some_and(toml::map::Map::is_empty)
}

/// Normalize a `SessionThreadConfig` after deserialization.
///
/// If canonical fields are missing but legacy fields are present, the legacy
/// values are copied into canonical fields without attempting a lossy reverse
/// conversion of `model_providers` into `OdyCodeProviderConfig`.
pub fn normalize_session_thread_config(config: SessionThreadConfig) -> SessionThreadConfig {
    let mut normalized = config;
    if normalized.default_provider.is_none() {
        normalized.default_provider = normalized.model_provider.clone();
    }
    normalized
}

fn session_thread_config_from_toml(
    value: TomlValue,
) -> Result<SessionThreadConfig, ThreadConfigLoadError> {
    let config: SessionThreadConfig = SessionThreadConfig::deserialize(value).map_err(|err| {
        ThreadConfigLoadError::new(
            ThreadConfigLoadErrorCode::Parse,
            /*status_code*/ None,
            format!("failed to parse session thread config TOML: {err}"),
        )
    })?;
    Ok(normalize_session_thread_config(config))
}

fn session_thread_config_to_toml(
    config: SessionThreadConfig,
) -> Result<TomlValue, ThreadConfigLoadError> {
    let mut table = toml::map::Map::new();

    // If canonical ody-code fields are present, write only those. Otherwise fall
    // back to legacy fields so that lossy provider features (auth, env_http_headers)
    // are not dropped.
    let use_canonical = config.default_provider.is_some()
        || config.default_model.is_some()
        || !config.providers.is_empty();

    if use_canonical {
        if let Some(default_provider) = config.default_provider {
            table.insert(
                "default_provider".to_string(),
                TomlValue::String(default_provider),
            );
        }
        if let Some(default_model) = config.default_model {
            table.insert(
                "default_model".to_string(),
                TomlValue::String(default_model),
            );
        }
        if !config.providers.is_empty() {
            let providers = TomlValue::try_from(config.providers).map_err(|err| {
                ThreadConfigLoadError::new(
                    ThreadConfigLoadErrorCode::Parse,
                    /*status_code*/ None,
                    format!("failed to convert session providers to config TOML: {err}"),
                )
            })?;
            table.insert("providers".to_string(), providers);
        }
    } else {
        if let Some(model_provider) = config.model_provider {
            table.insert(
                "model_provider".to_string(),
                TomlValue::String(model_provider),
            );
        }
        if !config.model_providers.is_empty() {
            let model_providers = TomlValue::try_from(config.model_providers).map_err(|err| {
                ThreadConfigLoadError::new(
                    ThreadConfigLoadErrorCode::Parse,
                    /*status_code*/ None,
                    format!("failed to convert session model providers to config TOML: {err}"),
                )
            })?;
            table.insert("model_providers".to_string(), model_providers);
        }
    }

    if !config.features.is_empty() {
        let features = config
            .features
            .into_iter()
            .map(|(feature, enabled)| (feature, TomlValue::Boolean(enabled)))
            .collect();
        table.insert("features".to_string(), TomlValue::Table(features));
    }

    Ok(TomlValue::Table(table))
}

#[cfg(test)]
mod tests {
    use ody_model_provider_info::ModelProviderInfo;
    use ody_model_provider_info::ProviderCapabilities;
    use ody_model_provider_info::WireApi;
    use pretty_assertions::assert_eq;

    use super::*;

    #[tokio::test]
    async fn loader_returns_session_and_user_sources() {
        let loader = StaticThreadConfigLoader::new(vec![
            ThreadConfigSource::Session(SessionThreadConfig {
                model_provider: Some("local".to_string()),
                model_providers: HashMap::from([("local".to_string(), test_provider("local"))]),
                features: BTreeMap::from([("plugins".to_string(), false)]),
                ..Default::default()
            }),
            ThreadConfigSource::User(UserThreadConfig::default()),
        ]);

        let sources = loader
            .load(ThreadConfigContext {
                thread_id: Some("thread-1".to_string()),
                ..Default::default()
            })
            .await
            .expect("thread config loads");

        assert_eq!(
            sources,
            vec![
                ThreadConfigSource::Session(SessionThreadConfig {
                    model_provider: Some("local".to_string()),
                    model_providers: HashMap::from([("local".to_string(), test_provider("local"))]),
                    features: BTreeMap::from([("plugins".to_string(), false)]),
                    ..Default::default()
                }),
                ThreadConfigSource::User(UserThreadConfig::default()),
            ]
        );
    }

    #[tokio::test]
    async fn loader_translates_sources_to_config_layers() {
        let provider = test_provider("local");
        let provider_toml =
            toml::Value::try_from(&provider).expect("test_provider serializes to toml");
        let loader = StaticThreadConfigLoader::new(vec![
            ThreadConfigSource::User(UserThreadConfig::default()),
            ThreadConfigSource::Session(SessionThreadConfig {
                model_provider: Some("local".to_string()),
                model_providers: HashMap::from([("local".to_string(), provider)]),
                features: BTreeMap::from([("plugins".to_string(), false)]),
                ..Default::default()
            }),
        ]);
        let layers = loader
            .load_config_layers(ThreadConfigContext {
                cwd: Some(
                    AbsolutePathBuf::from_absolute_path_checked(
                        std::env::temp_dir().join("project"),
                    )
                    .expect("absolute cwd"),
                ),
                ..Default::default()
            })
            .await
            .expect("thread config layers load");

        // Build expected TOML programmatically so it matches the loader's
        // serialization, including any new fields like `capabilities`.
        let mut expected_toml = toml::map::Map::new();
        expected_toml.insert(
            "model_provider".to_string(),
            toml::Value::String("local".to_string()),
        );
        let mut providers = toml::map::Map::new();
        providers.insert("local".to_string(), provider_toml);
        expected_toml.insert("model_providers".to_string(), toml::Value::Table(providers));

        // features
        let mut features = toml::map::Map::new();
        features.insert("plugins".to_string(), toml::Value::Boolean(false));
        expected_toml.insert("features".to_string(), toml::Value::Table(features));

        assert_eq!(
            layers,
            vec![ConfigLayerEntry::new(
                ConfigLayerSource::SessionFlags,
                toml::Value::Table(expected_toml).into()
            )]
        );
    }

    fn test_provider(name: &str) -> ModelProviderInfo {
        ModelProviderInfo {
            name: name.to_string(),
            base_url: Some("http://127.0.0.1:8061/api/ody".to_string()),
            env_key: None,
            env_key_instructions: None,
            experimental_bearer_token: None,
            auth: None,
            wire_api: WireApi::Responses,
            query_params: None,
            http_headers: None,
            env_http_headers: None,
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            websocket_connect_timeout_ms: None,
            supports_websockets: true,
            capabilities: ProviderCapabilities::default(),
        }
    }

    #[test]
    fn session_thread_config_roundtrip_with_providers() {
        let config = SessionThreadConfig {
            default_provider: Some("local".to_string()),
            default_model: Some("local/gpt-4o".to_string()),
            providers: HashMap::from([(
                "local".to_string(),
                OdyCodeProviderConfig {
                    r#type: "openai".to_string(),
                    api_key: Some("sk-test".to_string()),
                    base_url: Some("http://localhost:8061/v1".to_string()),
                    ..Default::default()
                },
            )]),
            features: BTreeMap::from([("plugins".to_string(), false)]),
            ..Default::default()
        };

        let toml =
            session_thread_config_to_toml(config.clone()).expect("serialize session thread config");
        let parsed =
            session_thread_config_from_toml(toml).expect("deserialize session thread config");

        assert_eq!(parsed, config);
    }

    #[test]
    fn session_thread_config_reads_legacy_model_providers() {
        let toml_str = r#"
model_provider = "local"

[model_providers.local]
name = "Local"
wire_api = "responses"
base_url = "http://127.0.0.1:8061/api/ody"
"#;
        let toml_value: TomlValue = toml::from_str(toml_str).expect("parse TOML");
        let config = session_thread_config_from_toml(toml_value)
            .expect("deserialize legacy session thread config");

        assert_eq!(config.model_provider, Some("local".to_string()));
        assert_eq!(config.default_provider, Some("local".to_string()));
        assert!(config.model_providers.contains_key("local"));
        assert_eq!(
            config.model_providers["local"].base_url,
            Some("http://127.0.0.1:8061/api/ody".to_string())
        );
    }
}
