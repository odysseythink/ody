//! Provider-name and alias validation for the TUI `/login` flow.

use ody_model_provider_info::BuiltInApiKeyProvider;
use std::str::FromStr;

/// Validate a user-supplied provider name against the supported login providers.
pub(crate) fn validate_login_provider_name(name: &str) -> Result<BuiltInApiKeyProvider, String> {
    let normalized = name.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Err("Provider name is required".to_string());
    }
    BuiltInApiKeyProvider::from_str(&normalized).map_err(|_| {
        format!("Unknown provider '{name}'. Supported providers: kimi, deepseek, glm.")
    })
}

/// Validate a custom provider alias.
///
/// Built-in provider IDs (`kimi`, `deepseek`, `glm`) are reserved because Ody
/// already ships compiled-in defaults for them; the login flow stores user
/// providers under a user-chosen alias.
pub(crate) fn validate_custom_alias(alias: &str) -> Result<(), String> {
    let trimmed = alias.trim();
    if trimmed.is_empty() {
        return Err("Alias cannot be empty".to_string());
    }
    if trimmed.len() > 64 {
        return Err("Alias must be 64 characters or fewer".to_string());
    }
    for reserved in [
        BuiltInApiKeyProvider::Kimi.id(),
        BuiltInApiKeyProvider::Deepseek.id(),
        BuiltInApiKeyProvider::Glm.id(),
    ] {
        if trimmed.eq_ignore_ascii_case(reserved) {
            return Err(format!("'{trimmed}' is a reserved provider alias"));
        }
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(
            "Alias may only contain letters, numbers, hyphens, and underscores".to_string(),
        );
    }
    let first = trimmed.chars().next().expect("non-empty alias");
    if !first.is_ascii_alphabetic() {
        return Err("Alias must start with a letter".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_known_providers() {
        assert_eq!(
            validate_login_provider_name("kimi").unwrap(),
            BuiltInApiKeyProvider::Kimi
        );
        assert_eq!(
            validate_login_provider_name("DeepSeek").unwrap(),
            BuiltInApiKeyProvider::Deepseek
        );
        assert_eq!(
            validate_login_provider_name("GLM").unwrap(),
            BuiltInApiKeyProvider::Glm
        );
    }

    #[test]
    fn rejects_unknown_provider() {
        assert!(validate_login_provider_name("openai").is_err());
        assert!(validate_login_provider_name("").is_err());
    }

    #[test]
    fn rejects_reserved_aliases() {
        assert!(validate_custom_alias("kimi").is_err());
        assert!(validate_custom_alias("DEEPSEEK").is_err());
        assert!(validate_custom_alias("glm").is_err());
    }

    #[test]
    fn rejects_invalid_alias_characters() {
        assert!(validate_custom_alias("my alias").is_err());
        assert!(validate_custom_alias("my@alias").is_err());
        assert!(validate_custom_alias("1alias").is_err());
        assert!(validate_custom_alias("").is_err());
    }

    #[test]
    fn accepts_valid_aliases() {
        assert!(validate_custom_alias("my-kimi").is_ok());
        assert!(validate_custom_alias("work_kimi").is_ok());
        assert!(validate_custom_alias("kimiCorp").is_ok());
    }
}
