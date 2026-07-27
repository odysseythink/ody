//! Canonical identity types for model providers and model references.
//!
//! These types provide a single, structured representation for provider aliases
//! and qualified model strings (`alias/model`). They live in the lowest-level
//! provider crate so that `core`, `tui`, and `models-manager` can all share the
//! same parsing rules without repeating `split_once('/')` logic.

use crate::ModelProviderInfo;
use std::collections::HashMap;

/// Provider kind resolved from display name and base URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderKind {
    Kimi,
    Deepseek,
    Glm,
    Custom,
}

/// Provider alias paired with its resolved kind.
///
/// `alias` is the key used in `config.model_providers` (e.g. `kimi` or a
/// numeric alias like `123456`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProviderRef {
    pub alias: String,
    pub kind: ProviderKind,
}

/// A canonical model reference: provider alias + bare model id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelRef {
    pub provider_alias: String,
    pub model_id: String,
}

impl ModelRef {
    /// Parse a qualified model string such as `alias/model`.
    ///
    /// Splits on the first `/`. If there is no `/`, the whole string is treated
    /// as a bare model id and `provider_alias` is empty. Multi-segment strings
    /// like `a/b/c` keep the remainder after the first slash as the model id.
    pub fn parse(qualified: &str) -> Self {
        if let Some((alias, model)) = qualified.split_once('/') {
            Self {
                provider_alias: alias.to_string(),
                model_id: model.to_string(),
            }
        } else {
            Self {
                provider_alias: String::new(),
                model_id: qualified.to_string(),
            }
        }
    }

    /// Return the qualified string `alias/model`.
    ///
    /// When the alias is empty, this returns the bare model id unchanged.
    pub fn qualified(&self) -> String {
        if self.provider_alias.is_empty() {
            self.model_id.clone()
        } else {
            format!("{}/{}", self.provider_alias, self.model_id)
        }
    }

    /// Return the bare model id (the part after the slash, or the whole string
    /// if there was no slash).
    pub fn bare(&self) -> &str {
        &self.model_id
    }

    /// Construct a model reference from explicit parts.
    pub fn from_parts(alias: &str, model: &str) -> Self {
        Self {
            provider_alias: alias.to_string(),
            model_id: model.to_string(),
        }
    }
}

/// Resolve a provider's kind from its display name and base URL.
///
/// This is the single source of truth for provider-kind detection. The legacy
/// `is_kimi`/`is_deepseek`/`is_glm` methods on [`ModelProviderInfo`] delegate
/// to this function via [`ModelProviderInfo::provider_kind`].
pub fn resolve_kind(info: &ModelProviderInfo) -> ProviderKind {
    let name = info.name.to_ascii_lowercase();
    let base_url = info
        .base_url
        .as_deref()
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    let name_matches = |candidates: &[&str]| candidates.iter().any(|c| name == *c);
    let url_matches = |needles: &[&str]| needles.iter().any(|n| base_url.contains(n));

    if name_matches(&["kimi", "moonshot"]) || url_matches(&["moonshot", "kimi.com"]) {
        ProviderKind::Kimi
    } else if name_matches(&["deepseek"]) || url_matches(&["deepseek"]) {
        ProviderKind::Deepseek
    } else if name_matches(&["glm", "zhipu", "bigmodel"]) || url_matches(&["bigmodel"]) {
        ProviderKind::Glm
    } else {
        ProviderKind::Custom
    }
}

/// Resolve a provider alias to a canonical [`ProviderRef`].
///
/// First searches the user-configured `providers` map (`config.model_providers`).
/// If the alias is not present, falls back to the built-in provider names
/// `kimi`, `deepseek`, and `glm`. Returns `None` for unknown aliases.
pub fn resolve_provider(
    alias: &str,
    providers: &HashMap<String, ModelProviderInfo>,
) -> Option<ProviderRef> {
    if let Some(info) = providers.get(alias) {
        return Some(ProviderRef {
            alias: alias.to_string(),
            kind: info.provider_kind(),
        });
    }

    let kind = match alias.to_ascii_lowercase().as_str() {
        "kimi" => ProviderKind::Kimi,
        "deepseek" => ProviderKind::Deepseek,
        "glm" => ProviderKind::Glm,
        _ => return None,
    };

    Some(ProviderRef {
        alias: alias.to_string(),
        kind,
    })
}

/// Resolve a provider alias to its full [`ModelProviderInfo`].
///
/// First searches the user-configured `providers` map (`config.model_providers`).
/// If the alias is not present, falls back to the built-in provider definitions
/// for `kimi`, `deepseek`, and `glm`. Returns `None` for unknown aliases.
///
/// This is the single place where the `model_providers.get(alias)` lookup and
/// the built-in `create_*_provider()` fallback are merged, so callers no longer
/// need to repeat the `kimi`/`deepseek`/`glm` match.
pub fn resolve_provider_info(
    alias: &str,
    providers: &HashMap<String, ModelProviderInfo>,
) -> Option<ModelProviderInfo> {
    if let Some(info) = providers.get(alias) {
        return Some(info.clone());
    }

    match alias.to_ascii_lowercase().as_str() {
        "kimi" => Some(crate::create_kimi_provider()),
        "deepseek" => Some(crate::create_deepseek_provider()),
        "glm" => Some(crate::create_glm_provider()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_alias_and_model() {
        let m = ModelRef::parse("kimi/kimi-for-coding");
        assert_eq!(m.provider_alias, "kimi");
        assert_eq!(m.model_id, "kimi-for-coding");
    }

    #[test]
    fn parse_bare_model() {
        let m = ModelRef::parse("kimi-for-coding");
        assert_eq!(m.provider_alias, "");
        assert_eq!(m.model_id, "kimi-for-coding");
    }

    #[test]
    fn parse_numeric_alias() {
        let m = ModelRef::parse("123456/kimi-for-coding");
        assert_eq!(m.provider_alias, "123456");
        assert_eq!(m.model_id, "kimi-for-coding");
    }

    #[test]
    fn parse_multi_segment_model_id() {
        // Only the first slash separates alias from model; the rest is the model id.
        let m = ModelRef::parse("a/b/c");
        assert_eq!(m.provider_alias, "a");
        assert_eq!(m.model_id, "b/c");
    }

    #[test]
    fn qualified_roundtrip() {
        let original = ModelRef::parse("kimi/kimi-for-coding");
        assert_eq!(original.qualified(), "kimi/kimi-for-coding");

        let bare = ModelRef::parse("kimi-for-coding");
        assert_eq!(bare.qualified(), "kimi-for-coding");
    }

    #[test]
    fn from_parts() {
        let m = ModelRef::from_parts("kimi", "kimi-for-coding");
        assert_eq!(m.provider_alias, "kimi");
        assert_eq!(m.model_id, "kimi-for-coding");
        assert_eq!(m.qualified(), "kimi/kimi-for-coding");
    }

    #[test]
    fn bare_returns_model_id() {
        let m = ModelRef::parse("kimi/kimi-for-coding");
        assert_eq!(m.bare(), "kimi-for-coding");
    }

    #[test]
    fn resolve_kind_kimi() {
        let info = crate::create_kimi_provider();
        assert_eq!(resolve_kind(&info), ProviderKind::Kimi);
    }

    #[test]
    fn resolve_kind_deepseek() {
        let info = crate::create_deepseek_provider();
        assert_eq!(resolve_kind(&info), ProviderKind::Deepseek);
    }

    #[test]
    fn resolve_kind_glm() {
        let info = crate::create_glm_provider();
        assert_eq!(resolve_kind(&info), ProviderKind::Glm);
    }

    #[test]
    fn resolve_kind_custom() {
        let mut info = crate::create_kimi_provider();
        info.name = "My Custom Provider".to_string();
        info.base_url = Some("https://example.com/v1".to_string());
        assert_eq!(resolve_kind(&info), ProviderKind::Custom);
    }

    #[test]
    fn resolve_provider_configured_alias() {
        let info = crate::create_kimi_provider();
        let providers = maplit::hashmap! {
            "my-kimi".to_string() => info,
        };

        let resolved = resolve_provider("my-kimi", &providers).expect("configured alias resolves");
        assert_eq!(resolved.alias, "my-kimi");
        assert_eq!(resolved.kind, ProviderKind::Kimi);
    }

    #[test]
    fn resolve_provider_configured_alias_takes_precedence_over_builtin() {
        let mut info = crate::create_kimi_provider();
        info.name = "My Custom Provider".to_string();
        info.base_url = Some("https://example.com/v1".to_string());
        let providers = maplit::hashmap! {
            "kimi".to_string() => info,
        };

        let resolved = resolve_provider("kimi", &providers).expect("configured alias resolves");
        assert_eq!(resolved.alias, "kimi");
        assert_eq!(resolved.kind, ProviderKind::Custom);
    }

    #[test]
    fn resolve_provider_builtin_names() {
        let providers = HashMap::new();

        let resolved = resolve_provider("kimi", &providers).expect("kimi builtin resolves");
        assert_eq!(resolved.alias, "kimi");
        assert_eq!(resolved.kind, ProviderKind::Kimi);

        let resolved = resolve_provider("deepseek", &providers).expect("deepseek builtin resolves");
        assert_eq!(resolved.alias, "deepseek");
        assert_eq!(resolved.kind, ProviderKind::Deepseek);

        let resolved = resolve_provider("glm", &providers).expect("glm builtin resolves");
        assert_eq!(resolved.alias, "glm");
        assert_eq!(resolved.kind, ProviderKind::Glm);
    }

    #[test]
    fn resolve_provider_builtin_names_are_case_insensitive() {
        let providers = HashMap::new();

        assert_eq!(
            resolve_provider("Kimi", &providers).map(|r| r.kind),
            Some(ProviderKind::Kimi)
        );
        assert_eq!(
            resolve_provider("DEEPSEEK", &providers).map(|r| r.kind),
            Some(ProviderKind::Deepseek)
        );
        assert_eq!(
            resolve_provider("Glm", &providers).map(|r| r.kind),
            Some(ProviderKind::Glm)
        );
    }

    #[test]
    fn resolve_provider_unknown_alias() {
        let providers = HashMap::new();
        assert_eq!(resolve_provider("unknown", &providers), None);
    }

    #[test]
    fn resolve_provider_info_configured_alias() {
        let mut info = crate::create_kimi_provider();
        info.name = "Alias Provider".to_string();
        let providers = maplit::hashmap! {
            "my-alias".to_string() => info.clone(),
        };

        let resolved = resolve_provider_info("my-alias", &providers).expect("configured alias resolves");
        assert_eq!(resolved.name, "Alias Provider");
    }

    #[test]
    fn resolve_provider_info_builtin_names() {
        let providers = HashMap::new();

        let resolved = resolve_provider_info("kimi", &providers).expect("kimi builtin resolves");
        assert_eq!(resolved.name, crate::create_kimi_provider().name);

        let resolved = resolve_provider_info("deepseek", &providers).expect("deepseek builtin resolves");
        assert_eq!(resolved.name, crate::create_deepseek_provider().name);

        let resolved = resolve_provider_info("glm", &providers).expect("glm builtin resolves");
        assert_eq!(resolved.name, crate::create_glm_provider().name);
    }

    #[test]
    fn resolve_provider_info_unknown_alias() {
        let providers = HashMap::new();
        assert_eq!(resolve_provider_info("unknown", &providers), None);
    }
}
