//! Locale parsing and mapping helpers shared across Ody crates.
//!
//! This module centralises BCP-47 / POSIX locale string handling so that both
//! `ody-core` (model instruction generation) and `ody-tui` (TUI i18n) resolve
//! the same language code from `ConfigToml::language` and from the system locale.

use icu_locale_core::Locale;

/// Best-effort reverse mapping from common human-readable language names to
/// BCP-47 primary language codes. This keeps old config values such as
/// `language = "Chinese"` working after the field semantics switched to codes.
fn human_readable_to_code(name: &str) -> Option<&'static str> {
    let code = match name.to_lowercase().as_str() {
        "arabic" => "ar",
        "czech" => "cs",
        "german" => "de",
        "greek" => "el",
        "english" => "en",
        "spanish" => "es",
        "french" => "fr",
        "hebrew" => "he",
        "hindi" => "hi",
        "indonesian" => "id",
        "italian" => "it",
        "japanese" => "ja",
        "korean" => "ko",
        "malay" => "ms",
        "dutch" => "nl",
        "polish" => "pl",
        "portuguese" => "pt",
        "russian" => "ru",
        "swedish" => "sv",
        "thai" => "th",
        "turkish" => "tr",
        "ukrainian" => "uk",
        "vietnamese" => "vi",
        "chinese" => "zh",
        _ => return None,
    };
    Some(code)
}

/// Parse a locale string into a normalised BCP-47 primary language code.
///
/// * Accepts BCP-47 tags such as `en`, `en-US`, `zh-CN`, `zh-Hans-CN`.
/// * Accepts POSIX-style tags with underscores such as `zh_CN` or `zh_Hans_CN`.
/// * Accepts common human-readable names such as `Chinese` or `English`.
/// * Returns `None` for empty strings, the special value `auto`, and for
///   strings that cannot be resolved to a known language.
pub fn parse_locale_code(input: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("auto") {
        return None;
    }

    if let Some(code) = human_readable_to_code(trimmed) {
        return Some(code.to_string());
    }

    // Normalise POSIX separators to BCP-47 separators before parsing.
    let normalised = trimmed.replace('_', "-");
    let locale = normalised.parse::<Locale>().ok()?;
    let code = locale.id.language.as_str().to_lowercase();
    // Only expose codes that we can map to a human-readable model language;
    // this keeps unknown / placeholder tags (e.g. `xx-XX`) from propagating.
    map_locale_code_to_model_language(&code).map(|_| code)
}

/// Detect the user's preferred language from the system locale.
///
/// Returns a normalised BCP-47 primary language code, or `None` if the system
/// locale cannot be resolved to a known language.
pub fn detect_system_locale_code() -> Option<String> {
    sys_locale::get_locale().and_then(|locale| parse_locale_code(&locale))
}

/// Map a BCP-47 primary language code to a human-readable language name
/// suitable for a model instruction.
pub fn map_locale_code_to_model_language(code: &str) -> Option<String> {
    let primary = code.split(|c| c == '-' || c == '_').next()?.to_lowercase();
    let name = match primary.as_str() {
        "ar" => "Arabic",
        "cs" => "Czech",
        "de" => "German",
        "el" => "Greek",
        "en" => "English",
        "es" => "Spanish",
        "fr" => "French",
        "he" => "Hebrew",
        "hi" => "Hindi",
        "id" => "Indonesian",
        "it" => "Italian",
        "ja" => "Japanese",
        "ko" => "Korean",
        "ms" => "Malay",
        "nl" => "Dutch",
        "pl" => "Polish",
        "pt" => "Portuguese",
        "ru" => "Russian",
        "sv" => "Swedish",
        "th" => "Thai",
        "tr" => "Turkish",
        "uk" => "Ukrainian",
        "vi" => "Vietnamese",
        "zh" => "Chinese",
        _ => return None,
    };
    Some(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn parse_locale_code_parses_bcp47_tags() {
        assert_eq!(parse_locale_code("en"), Some("en".to_string()));
        assert_eq!(parse_locale_code("en-US"), Some("en".to_string()));
        assert_eq!(parse_locale_code("zh-CN"), Some("zh".to_string()));
        assert_eq!(parse_locale_code("zh-Hans-CN"), Some("zh".to_string()));
        assert_eq!(parse_locale_code("ja_JP"), Some("ja".to_string()));
        assert_eq!(parse_locale_code("ko-KR"), Some("ko".to_string()));
        assert_eq!(parse_locale_code("fr_FR"), Some("fr".to_string()));
    }

    #[test]
    fn parse_locale_code_parses_human_readable_names() {
        assert_eq!(parse_locale_code("Chinese"), Some("zh".to_string()));
        assert_eq!(parse_locale_code("english"), Some("en".to_string()));
        assert_eq!(parse_locale_code("Spanish"), Some("es".to_string()));
        assert_eq!(parse_locale_code("French"), Some("fr".to_string()));
    }

    #[test]
    fn parse_locale_code_returns_none_for_auto_empty_and_unknown() {
        assert_eq!(parse_locale_code("auto"), None);
        assert_eq!(parse_locale_code("Auto"), None);
        assert_eq!(parse_locale_code("  "), None);
        assert_eq!(parse_locale_code("xx-XX"), None);
    }

    #[test]
    fn map_locale_code_to_model_language_maps_common_codes() {
        assert_eq!(
            map_locale_code_to_model_language("zh").as_deref(),
            Some("Chinese")
        );
        assert_eq!(
            map_locale_code_to_model_language("zh-CN").as_deref(),
            Some("Chinese")
        );
        assert_eq!(
            map_locale_code_to_model_language("en").as_deref(),
            Some("English")
        );
        assert_eq!(
            map_locale_code_to_model_language("es").as_deref(),
            Some("Spanish")
        );
        assert_eq!(
            map_locale_code_to_model_language("fr").as_deref(),
            Some("French")
        );
    }

    #[test]
    fn map_locale_code_to_model_language_returns_none_for_unknown() {
        assert_eq!(map_locale_code_to_model_language("xx"), None);
    }

    #[test]
    fn detect_system_locale_code_does_not_panic() {
        // The actual value depends on the build environment, so just make sure
        // the call completes without panicking and returns a valid code if any.
        if let Some(code) = detect_system_locale_code() {
            assert!(!code.is_empty());
        }
    }
}
