//! Config-edit builders for the TUI `/login` flow.

use ody_app_server_protocol::ConfigEdit;
use ody_model_provider_info::LoginProvider;

use crate::config_update::replace_config_value;

/// Build config edits for a newly configured provider.
///
/// Writes individual field edits (`providers.<alias>.type`, `.api_key`, `.base_url`)
/// rather than a single JSON object so the app server produces standard TOML
/// tables instead of inline tables.
pub(crate) fn build_login_provider_edits(
    alias: &str,
    provider: LoginProvider,
    api_key: &str,
    base_url: Option<&str>,
) -> Vec<ConfigEdit> {
    let provider_id = provider.id();
    let mut edits = vec![
        replace_config_value(
            format!("providers.{alias}.type"),
            serde_json::json!(provider_id),
        ),
        replace_config_value(
            format!("providers.{alias}.api_key"),
            serde_json::json!(api_key),
        ),
    ];
    if let Some(base_url) = base_url {
        edits.push(replace_config_value(
            format!("providers.{alias}.base_url"),
            serde_json::json!(base_url),
        ));
    }
    edits
}

/// Build config edits that persist a model alias and set it as the default model.
pub(crate) fn build_login_model_edits(
    alias: &str,
    provider: LoginProvider,
    model_id: &str,
    display_name: Option<&str>,
) -> Vec<ConfigEdit> {
    let provider_id = provider.id();
    let model_alias = format!("{alias}/{model_id}");
    let mut edits = vec![
        replace_config_value(
            format!("models.\"{model_alias}\".provider"),
            serde_json::json!(provider_id),
        ),
        replace_config_value(
            format!("models.\"{model_alias}\".model"),
            serde_json::json!(model_id),
        ),
    ];
    if let Some(display_name) = display_name {
        edits.push(replace_config_value(
            format!("models.\"{model_alias}\".display_name"),
            serde_json::json!(display_name),
        ));
    }
    edits.push(replace_config_value(
        "model",
        serde_json::json!(model_alias),
    ));
    edits
}

/// Build config edits that remove a provider and any model aliases that belong
/// to it, and clear the default model if it points to the removed provider.
pub(crate) fn build_logout_provider_edits(
    aliases_to_remove: &[String],
    default_model: Option<&str>,
) -> Vec<ConfigEdit> {
    use crate::config_update::clear_config_value;

    if aliases_to_remove.is_empty() {
        return Vec::new();
    }

    let mut edits = Vec::new();
    for alias in aliases_to_remove {
        edits.push(clear_config_value(format!("providers.{alias}")));
    }

    if let Some(model) = default_model {
        if let Some((alias, _)) = model.split_once('/') {
            if aliases_to_remove
                .iter()
                .any(|a| a.eq_ignore_ascii_case(alias))
            {
                edits.push(clear_config_value("model"));
            }
        }
    }

    edits
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
