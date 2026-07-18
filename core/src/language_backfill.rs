//! One-time backfill of the `language` field from the system locale.
//!
//! When `config.toml` has no `language` (or it is empty), the model's
//! "Think and respond in <language>." directive is derived by detecting the
//! system locale on *every* run (see `Config`'s `effective_language`). That
//! run-time detection is a source of instability: if a launch environment
//! cannot resolve a locale, the directive is dropped entirely and the model
//! silently falls back to English.
//!
//! To make the preference stable, this backfill resolves the system locale
//! once and persists the concrete BCP-47 primary code (e.g. `zh`) into
//! `config.toml`. Subsequent runs then read an explicit value and no longer
//! depend on locale detection.
//!
//! The backfill is deliberately conservative: it only writes when the field is
//! absent or empty, so an explicit choice (including the special value `auto`,
//! which opts back in to per-run detection) is always respected.

use crate::config::edit::ConfigEditsBuilder;
use ody_config::config_toml::ConfigToml;
use std::path::Path;

/// Outcome of the system-language backfill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LanguageBackfillStatus {
    /// `language` was already set to a non-empty value; nothing written.
    SkippedExplicit,
    /// The field was missing/empty but the system locale could not be resolved
    /// to a known language, so nothing was written (run-time auto-detection
    /// still applies as before).
    SkippedUndetected,
    /// Detected the system language and persisted its BCP-47 primary code.
    Applied(String),
}

/// Persist the detected system language into `config.toml` when the `language`
/// field is missing or empty.
pub async fn backfill_language_if_needed(
    ody_home: &Path,
    config_toml: &ConfigToml,
) -> anyhow::Result<LanguageBackfillStatus> {
    let explicit = config_toml
        .language
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if explicit.is_some() {
        return Ok(LanguageBackfillStatus::SkippedExplicit);
    }

    let Some(code) = ody_config::locale::detect_system_locale_code() else {
        return Ok(LanguageBackfillStatus::SkippedUndetected);
    };

    ConfigEditsBuilder::new(ody_home)
        .set_language(Some(&code))
        .apply()
        .await?;

    Ok(LanguageBackfillStatus::Applied(code))
}

#[cfg(test)]
#[path = "language_backfill_tests.rs"]
mod tests;
