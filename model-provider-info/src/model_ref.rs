//! Canonical identity types for model providers and model references.
//!
//! These types provide a single, structured representation for provider aliases
//! and qualified model strings (`alias/model`). They live in the lowest-level
//! provider crate so that `core`, `tui`, and `models-manager` can all share the
//! same parsing rules without repeating `split_once('/')` logic.

use crate::ModelProviderInfo;

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
/// This currently delegates to the existing heuristic `is_kimi`/`is_deepseek`/
/// `is_glm` methods on [`ModelProviderInfo`]. Later stages of the refactor can
/// reverse the dependency if desired.
pub fn resolve_kind(info: &ModelProviderInfo) -> ProviderKind {
    if info.is_kimi() {
        ProviderKind::Kimi
    } else if info.is_deepseek() {
        ProviderKind::Deepseek
    } else if info.is_glm() {
        ProviderKind::Glm
    } else {
        ProviderKind::Custom
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
}
