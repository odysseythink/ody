//! Minimal TUI i18n runtime backed by bundled TOML translations.
//!
//! The runtime supports a `zh-CN → zh → en` fallback ladder and surfaces a
//! missing key as the key itself while emitting a `tracing::warn!`. No runtime
//! loading of custom translations is supported; all resources are embedded at
//! compile time via `include_str!`.

mod registry;

use std::collections::HashMap;

/// Runtime i18n handle holding the resolved translation map.
#[derive(Debug, Clone)]
pub struct I18n {
    translations: HashMap<String, String>,
    resolved_locale: String,
}

impl I18n {
    /// Build a new `I18n` handle for the requested locale code.
    ///
    /// The code is resolved against the bundled registry using the standard
    /// fallback ladder. Parse failures are treated as a programming error
    /// because the bundled resources are validated at test time.
    pub fn new(requested: &str) -> Self {
        let (resolved_locale, source) = registry::resolve(requested);
        let translations: HashMap<String, String> =
            toml::from_str(source).expect("bundled TOML translation resources should be valid");
        Self {
            translations,
            resolved_locale: resolved_locale.to_string(),
        }
    }

    /// Return the locale code that was actually resolved after fallback.
    pub fn resolved_locale(&self) -> &str {
        &self.resolved_locale
    }

    /// Look up a translation key.
    ///
    /// Returns the translated value if present; otherwise returns the key
    /// itself and logs a `tracing::warn!` so that missing keys are visible
    /// during development.
    pub fn get<'a>(&'a self, key: &'a str) -> &'a str {
        match self.translations.get(key) {
            Some(value) => value.as_str(),
            None => {
                tracing::warn!(i18n_key = key, locale = %self.resolved_locale, "missing i18n key");
                key
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    const EXPECTED_EN_KEYS: &[&str] = &[
        "design_audit_title",
        "design_audit_subtitle",
        "design_audit_basic",
        "design_audit_basic_description",
        "design_audit_standard",
        "design_audit_standard_description",
        "design_audit_deep",
        "design_audit_deep_description",
    ];

    #[test]
    fn english_bundle_contains_all_expected_keys() {
        let i18n = I18n::new("en");
        assert_eq!(i18n.resolved_locale(), "en");
        for key in EXPECTED_EN_KEYS {
            assert!(
                i18n.translations.contains_key(*key),
                "missing key {key} in en bundle"
            );
        }
    }

    #[test]
    fn zh_cn_resolves_to_zh() {
        let i18n = I18n::new("zh-CN");
        assert_eq!(i18n.resolved_locale(), "zh");
        assert_eq!(i18n.get("design_audit_standard"), "标准");
    }

    #[test]
    fn unknown_locale_falls_back_to_english() {
        let i18n = I18n::new("de");
        assert_eq!(i18n.resolved_locale(), "en");
        assert_eq!(i18n.get("design_audit_title"), "Select Design Audit Level");
    }

    #[test]
    fn missing_key_returns_key_and_warns() {
        let i18n = I18n::new("en");
        assert_eq!(i18n.get("nonexistent_key"), "nonexistent_key");
    }
}
