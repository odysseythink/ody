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
/// Currently carries only the web search provider configuration.
#[derive(Clone, Deserialize, Serialize, PartialEq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ServicesConfig {
    #[serde(rename = "webSearch")]
    pub web_search: Option<WebSearchConfig>,
}

impl fmt::Debug for ServicesConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServicesConfig")
            .field("web_search", &self.web_search)
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
    fn provider_names_serialize_to_lowercase() {
        assert_eq!(
            serde_json::to_string(&WebSearchProviderName::Moonshot).unwrap(),
            "\"moonshot\""
        );
        assert_eq!(WebSearchProviderName::Serpapi.to_string(), "serpapi");
    }
}
