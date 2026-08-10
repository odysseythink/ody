//! Metadata describing the per-provider configuration fields that the TUI
//! (and other callers) can render as an editable form.

use crate::config::WebSearchProviderName;

/// Kind of value a configuration field accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderConfigFieldKind {
    /// An API key or other secret that should be masked while typing.
    ApiKey,
    /// A free-form string value.
    String,
    /// An unsigned 32-bit integer in the inclusive range `[min, max]`.
    U32 { min: u32, max: u32 },
    /// One of a fixed set of string choices.
    Choice { options: &'static [&'static str] },
}

/// One editable field for a web-search provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderConfigField {
    /// Dotted key used in `WebSearchProviderConfig` / `options`.
    pub key: &'static str,
    /// Human-readable label shown in the form.
    pub label: &'static str,
    /// Short hint describing what the field does and its allowed values.
    pub description: &'static str,
    /// What kind of input the field expects.
    pub kind: ProviderConfigFieldKind,
    /// Whether the field must have a non-empty value before saving.
    pub required: bool,
}

impl ProviderConfigField {
    const fn api_key() -> Self {
        Self {
            key: "api_key",
            label: "API key",
            description: "Provider API key. Leave blank to read from the <PROVIDER>_API_KEY environment variable.",
            kind: ProviderConfigFieldKind::ApiKey,
            required: false,
        }
    }

    const fn timeout_ms() -> Self {
        Self {
            key: "timeout_ms",
            label: "Timeout (ms)",
            description: "Request timeout in milliseconds (1000–300000). Leave blank for the provider default.",
            kind: ProviderConfigFieldKind::U32 {
                min: 1000,
                max: 300_000,
            },
            required: false,
        }
    }

    const fn base_url() -> Self {
        Self {
            key: "base_url",
            label: "Base URL",
            description: "Custom API endpoint. Leave blank to use the provider default.",
            kind: ProviderConfigFieldKind::String,
            required: false,
        }
    }

    const fn string(key: &'static str, label: &'static str, description: &'static str) -> Self {
        Self {
            key,
            label,
            description,
            kind: ProviderConfigFieldKind::String,
            required: false,
        }
    }

    const fn choice(
        key: &'static str,
        label: &'static str,
        description: &'static str,
        options: &'static [&'static str],
    ) -> Self {
        Self {
            key,
            label,
            description,
            kind: ProviderConfigFieldKind::Choice { options },
            required: false,
        }
    }

    const fn u32(
        key: &'static str,
        label: &'static str,
        description: &'static str,
        min: u32,
        max: u32,
    ) -> Self {
        Self {
            key,
            label,
            description,
            kind: ProviderConfigFieldKind::U32 { min, max },
            required: false,
        }
    }
}

/// Return the ordered list of editable fields for a web-search provider.
pub fn provider_config_fields(name: WebSearchProviderName) -> Vec<ProviderConfigField> {
    match name {
        WebSearchProviderName::Duckduckgo => vec![
            ProviderConfigField::timeout_ms(),
            ProviderConfigField::string(
                "proxy_url",
                "Proxy URL",
                "Optional HTTP proxy that forwards DuckDuckGo HTML requests.",
            ),
        ],
        WebSearchProviderName::Searxng => vec![
            ProviderConfigField::timeout_ms(),
            ProviderConfigField {
                key: "base_url",
                label: "Base URL",
                description: "SearXNG instance search endpoint (e.g. https://searx.example/search).",
                kind: ProviderConfigFieldKind::String,
                required: true,
            },
        ],
        WebSearchProviderName::Bing => vec![
            ProviderConfigField::api_key(),
            ProviderConfigField::timeout_ms(),
            ProviderConfigField::base_url(),
        ],
        WebSearchProviderName::Serpapi => vec![
            ProviderConfigField::api_key(),
            ProviderConfigField::timeout_ms(),
            ProviderConfigField::base_url(),
        ],
        WebSearchProviderName::Searchapi => vec![
            ProviderConfigField::api_key(),
            ProviderConfigField::timeout_ms(),
            ProviderConfigField::base_url(),
        ],
        WebSearchProviderName::Serper => vec![
            ProviderConfigField::api_key(),
            ProviderConfigField::timeout_ms(),
            ProviderConfigField::base_url(),
        ],
        WebSearchProviderName::Baidu => vec![
            ProviderConfigField::api_key(),
            ProviderConfigField::timeout_ms(),
            ProviderConfigField::base_url(),
            ProviderConfigField::u32(
                "top_k",
                "Top K",
                "Maximum number of results to return (1–50).",
                1,
                50,
            ),
        ],
        WebSearchProviderName::Serply => vec![
            ProviderConfigField::api_key(),
            ProviderConfigField::timeout_ms(),
            ProviderConfigField::base_url(),
            ProviderConfigField::string(
                "language",
                "Language",
                "Search language code such as 'en' or 'zh'.",
            ),
            ProviderConfigField::string(
                "hl",
                "Host language (hl)",
                "Google UI language code such as 'en' or 'zh-CN'.",
            ),
            ProviderConfigField::string(
                "gl",
                "Geolocation (gl)",
                "Country code for results such as 'US' or 'CN'.",
            ),
            ProviderConfigField::choice(
                "device",
                "Device",
                "Device type used for the search.",
                &["desktop", "mobile", "tablet"],
            ),
        ],
        WebSearchProviderName::Tavily => vec![
            ProviderConfigField::api_key(),
            ProviderConfigField::timeout_ms(),
            ProviderConfigField::base_url(),
            ProviderConfigField::choice(
                "search_depth",
                "Search depth",
                "How deeply Tavily should search.",
                &["basic", "advanced"],
            ),
        ],
        WebSearchProviderName::Exa => vec![
            ProviderConfigField::api_key(),
            ProviderConfigField::timeout_ms(),
            ProviderConfigField::base_url(),
            ProviderConfigField::choice(
                "type",
                "Result type",
                "Exa query type.",
                &["auto", "neural", "keyword"],
            ),
            ProviderConfigField::choice(
                "livecrawl",
                "Livecrawl",
                "When to crawl result pages for fresh content.",
                &["always", "fallback", "never"],
            ),
        ],
        WebSearchProviderName::Perplexity => vec![
            ProviderConfigField::api_key(),
            ProviderConfigField::timeout_ms(),
            ProviderConfigField::base_url(),
            ProviderConfigField::u32(
                "max_results",
                "Max results",
                "Maximum number of search results (1–20).",
                1,
                20,
            ),
            ProviderConfigField::u32(
                "max_tokens_per_page",
                "Max tokens per page",
                "Maximum tokens returned per result page (1–10000).",
                1,
                10_000,
            ),
        ],
        WebSearchProviderName::Moonshot => vec![
            ProviderConfigField::api_key(),
            ProviderConfigField::timeout_ms(),
            ProviderConfigField::base_url(),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WebSearchProviderName;

    #[test]
    fn every_provider_has_fields() {
        for name in [
            WebSearchProviderName::Duckduckgo,
            WebSearchProviderName::Bing,
            WebSearchProviderName::Serpapi,
            WebSearchProviderName::Searchapi,
            WebSearchProviderName::Serper,
            WebSearchProviderName::Baidu,
            WebSearchProviderName::Serply,
            WebSearchProviderName::Searxng,
            WebSearchProviderName::Tavily,
            WebSearchProviderName::Exa,
            WebSearchProviderName::Perplexity,
            WebSearchProviderName::Moonshot,
        ] {
            let fields = provider_config_fields(name);
            assert!(
                !fields.is_empty(),
                "{name} should expose at least one config field"
            );
            // Every provider has a timeout field so users can tune it.
            assert!(
                fields.iter().any(|f| f.key == "timeout_ms"),
                "{name} should expose a timeout_ms field"
            );
            // The dotted key must be unique within a provider.
            let mut keys: Vec<_> = fields.iter().map(|f| f.key).collect();
            keys.sort();
            let deduped: Vec<_> = keys.iter().copied().collect();
            assert_eq!(
                keys, deduped,
                "{name} config field keys must be unique"
            );
        }
    }

    #[test]
    fn searxng_requires_base_url() {
        let fields = provider_config_fields(WebSearchProviderName::Searxng);
        let base_url = fields.iter().find(|f| f.key == "base_url").unwrap();
        assert!(base_url.required);
    }

    #[test]
    fn api_key_is_absent_for_free_providers() {
        for name in [WebSearchProviderName::Duckduckgo, WebSearchProviderName::Searxng] {
            assert!(
                !provider_config_fields(name).iter().any(|f| f.key == "api_key"),
                "{name} should not expose an API key field"
            );
        }
    }
}
