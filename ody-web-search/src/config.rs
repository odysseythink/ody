use std::collections::HashMap;
use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use strum_macros::{Display, EnumString};
use ts_rs::TS;

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    Display,
    EnumString,
    JsonSchema,
    TS,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum WebSearchProviderName {
    Duckduckgo,
    Bing,
    Serpapi,
    Searchapi,
    Serper,
    Baidu,
    Serply,
    Searxng,
    Tavily,
    Exa,
    Perplexity,
    Moonshot,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct WebSearchProviderConfig {
    pub provider: WebSearchProviderName,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    #[serde(skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub options: HashMap<String, Value>,
}

impl fmt::Debug for WebSearchProviderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebSearchProviderConfig")
            .field("provider", &self.provider)
            .field("api_key", &self.api_key.as_ref().map(|_| "***"))
            .field("timeout_ms", &self.timeout_ms)
            .field("options", &format!("{} entries", self.options.len()))
            .finish()
    }
}

#[derive(Clone, Serialize, PartialEq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct WebSearchConfig {
    /// The currently active primary search provider.
    pub primary: WebSearchProviderName,
    /// Per-provider presets. The active provider's config is resolved from this map,
    /// falling back to an empty default if no preset has been saved.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub providers: HashMap<WebSearchProviderName, WebSearchProviderConfig>,
    /// Optional fallback provider used when the primary fails.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secondary: Option<WebSearchProviderConfig>,
}

impl WebSearchConfig {
    /// Resolve the full configuration for the currently active primary provider.
    pub fn primary_config(&self) -> WebSearchProviderConfig {
        self.providers.get(&self.primary).cloned().unwrap_or_else(|| WebSearchProviderConfig {
            provider: self.primary,
            api_key: None,
            timeout_ms: None,
            options: HashMap::new(),
        })
    }

    /// Return the saved preset for a provider, if any.
    pub fn provider_config(&self, provider: WebSearchProviderName) -> Option<&WebSearchProviderConfig> {
        self.providers.get(&provider)
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum WebSearchConfigRepr {
    New {
        primary: WebSearchProviderName,
        #[serde(default)]
        providers: HashMap<WebSearchProviderName, WebSearchProviderConfig>,
        #[serde(default)]
        secondary: Option<WebSearchProviderConfig>,
    },
    Legacy {
        primary: WebSearchProviderConfig,
        #[serde(default)]
        secondary: Option<WebSearchProviderConfig>,
    },
}

impl<'de> Deserialize<'de> for WebSearchConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        match WebSearchConfigRepr::deserialize(deserializer)? {
            WebSearchConfigRepr::New {
                primary,
                providers,
                secondary,
            } => Ok(WebSearchConfig {
                primary,
                providers,
                secondary,
            }),
            WebSearchConfigRepr::Legacy { primary, secondary } => {
                let provider = primary.provider;
                let mut providers = HashMap::new();
                providers.insert(provider, primary);
                Ok(WebSearchConfig {
                    primary: provider,
                    providers,
                    secondary,
                })
            }
        }
    }
}

impl fmt::Debug for WebSearchConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebSearchConfig")
            .field("primary", &self.primary)
            .field("providers", &format!("{} entries", self.providers.len()))
            .field("secondary", &self.secondary)
            .finish()
    }
}

/// Top-level `[services]` table in `config.toml`.
#[derive(Clone, Deserialize, Serialize, PartialEq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ServicesConfig {
    #[serde(rename = "webSearch")]
    pub web_search: Option<WebSearchConfig>,
    #[serde(rename = "browser")]
    pub browser: Option<ody_browser_control::BrowserControlConfig>,
    #[serde(rename = "database")]
    pub database: Option<ody_database::config::DatabaseConfig>,
}

impl fmt::Debug for ServicesConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServicesConfig")
            .field("web_search", &self.web_search)
            .field("browser", &self.browser)
            .field("database", &self.database)
            .finish()
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_config_masks_api_key_in_debug() -> Result<(), Box<dyn std::error::Error>> {
        let config: WebSearchConfig = serde_json::from_str(
            r#"{
                "primary": {
                    "provider": "bing",
                    "api_key": "secret-key",
                    "timeout_ms": 15000,
                    "options": {}
                },
                "secondary": {
                    "provider": "serpapi",
                    "api_key": "another-secret"
                }
            }"#,
        )?;
        assert_eq!(config.primary, WebSearchProviderName::Bing);
        let primary = config.primary_config();
        assert_eq!(primary.api_key.as_deref(), Some("secret-key"));
        let debug = format!("{:?}", config);
        assert!(debug.contains("***"));
        assert!(!debug.contains("secret-key"));
        assert!(!debug.contains("another-secret"));
        Ok(())
    }

    #[test]
    fn services_config_with_browser_round_trips() {
        let services = ServicesConfig {
            web_search: None,
            browser: Some(ody_browser_control::BrowserControlConfig {
                headless: false,
                ..Default::default()
            }),
            database: None,
        };
        let json = serde_json::to_value(&services).expect("serialize services config");
        let back: ServicesConfig = serde_json::from_value(json).expect("deserialize services config");
        assert_eq!(back, services);
    }

    #[test]
    fn services_config_deserializes_browser_from_toml() {
        let services: ServicesConfig = toml::from_str(
            r#"
[browser]
headless = false
mode = "external"
connect_url = "ws://localhost:9222"
"#,
        )
        .expect("deserialize toml");
        let browser = services.browser.expect("browser config present");
        assert!(!browser.headless);
        assert_eq!(browser.mode, ody_browser_control::BrowserControlMode::External);
        assert_eq!(browser.connect_url.as_deref(), Some("ws://localhost:9222"));
    }

    #[test]
    fn new_format_web_search_config_deserializes_from_toml() {
        let config: WebSearchConfig = toml::from_str(
            r#"
primary = "duckduckgo"

[providers.duckduckgo]
provider = "duckduckgo"
timeout_ms = 30000

[providers.duckduckgo.options]
proxy_url = "http://127.0.0.1:12001"
"#,
        )
        .expect("deserialize new format");
        assert_eq!(config.primary, WebSearchProviderName::Duckduckgo);
        let duck = config
            .provider_config(WebSearchProviderName::Duckduckgo)
            .expect("duckduckgo preset");
        assert_eq!(duck.timeout_ms, Some(30000));
        assert_eq!(
            duck.options.get("proxy_url").and_then(|v| v.as_str()),
            Some("http://127.0.0.1:12001")
        );
    }

    #[test]
    fn services_config_deserializes_database_from_toml() {
        let services: ServicesConfig = toml::from_str(
            r#"
[database]
primary = "db_connection_1"

[database.connections.db_connection_1]
connection = "db_connection_1"
provider = "postgres"
host = "127.0.0.1"
port = 5432
database = "project_a"
username = "ranwei"
password = "secret"

[database.connections.db_connection_1.options]
sslmode = "disable"
"#,
        )
        .expect("deserialize toml");
        let database = services.database.expect("database config present");
        assert_eq!(database.primary, "db_connection_1");
        let conn = database
            .connection_config("db_connection_1")
            .expect("db_connection_1 preset");
        assert_eq!(conn.provider, ody_database::config::DatabaseProviderName::Postgres);
        assert_eq!(conn.port, 5432);
        assert_eq!(conn.password.as_deref(), Some("secret"));
    }
}
