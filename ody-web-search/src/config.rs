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
    pub api_key: Option<String>,
    pub timeout_ms: Option<u64>,
    #[serde(default)]
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

#[derive(Clone, Deserialize, Serialize, PartialEq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct WebSearchConfig {
    pub primary: WebSearchProviderConfig,
    pub secondary: Option<WebSearchProviderConfig>,
}

impl fmt::Debug for WebSearchConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebSearchConfig")
            .field("primary", &self.primary)
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
}

impl fmt::Debug for ServicesConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServicesConfig")
            .field("web_search", &self.web_search)
            .field("browser", &self.browser)
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
        assert_eq!(config.primary.provider, WebSearchProviderName::Bing);
        assert_eq!(config.primary.api_key.as_deref(), Some("secret-key"));
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
}
