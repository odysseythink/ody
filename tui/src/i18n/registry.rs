//! Bundled locale registry for the TUI i18n framework.
//!
//! The registry maps canonical language codes to the embedded TOML translation
//! resources. Resolution follows the `zh-CN → zh → en` fallback ladder so that
//! region-specific codes can reuse a broader translation while still defaulting to
//! English when nothing is available.

use std::collections::HashMap;

/// Embedded English translation resource.
const EN_TRANSLATIONS: &str = include_str!("translations/en.toml");
/// Embedded Chinese translation resource.
const ZH_TRANSLATIONS: &str = include_str!("translations/zh.toml");

lazy_static::lazy_static! {
    /// Canonical locale code → bundled TOML source.
    static ref BUNDLED: HashMap<&'static str, &'static str> = {
        let mut m = HashMap::new();
        m.insert("en", EN_TRANSLATIONS);
        m.insert("zh", ZH_TRANSLATIONS);
        m
    };
}

/// Resolve a requested locale code to a bundled translation source.
///
/// Falls back from the exact code to the primary language code and finally to
/// English (`en`). Returns the resolved locale code and the TOML source.
pub(super) fn resolve(requested: &str) -> (&'static str, &'static str) {
    let requested = requested.trim().to_lowercase();
    if requested.is_empty() {
        return ("en", EN_TRANSLATIONS);
    }

    if let Some(&source) = BUNDLED.get(requested.as_str()) {
        return (BUNDLED.get_key_value(requested.as_str()).unwrap().0, source);
    }

    let primary = requested
        .split(|c| c == '-' || c == '_')
        .next()
        .unwrap_or("en");
    if let Some(&source) = BUNDLED.get(primary) {
        return (BUNDLED.get_key_value(primary).unwrap().0, source);
    }

    ("en", EN_TRANSLATIONS)
}

/// List the bundled locale codes. Used by tests to assert completeness.
pub(super) fn bundled_codes() -> Vec<&'static str> {
    BUNDLED.keys().copied().collect()
}
