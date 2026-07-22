//! Config-edit builders for the TUI `/login` flow.

use ody_app_server_protocol::ConfigEdit;
use ody_config::config_toml::OdyCodeModelConfig;
use ody_model_provider_info::LoginProvider;
use ody_protocol::model_metadata::ModelInfo;
use std::collections::HashMap;

use crate::config_update::{clear_config_value, replace_config_value};
use ody_model_provider::login::LoginModelInfo;

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
    let display_name = display_name.unwrap_or(model_id);
    let model = LoginModelInfo {
        id: model_id.to_string(),
        display_name: display_name.to_string(),
    };
    build_login_models_edits(alias, provider, &[model], model_id)
}

/// Build config edits that persist every model fetched during `/login` and set
/// the user-selected model as the default.
pub(crate) fn build_login_models_edits(
    alias: &str,
    provider: LoginProvider,
    models: &[LoginModelInfo],
    selected_model_id: &str,
) -> Vec<ConfigEdit> {
    let bundled_models = bundled_models_by_provider_slug(provider.id());
    let mut edits = Vec::new();
    for model in models {
        let model_alias = format!("{alias}/{}", model.id);
        edits.push(replace_config_value(
            format!("models.\"{model_alias}\".provider"),
            serde_json::json!(alias),
        ));
        edits.push(replace_config_value(
            format!("models.\"{model_alias}\".model"),
            serde_json::json!(model.id),
        ));
        if !model.display_name.is_empty() && model.display_name != model.id {
            edits.push(replace_config_value(
                format!("models.\"{model_alias}\".display_name"),
                serde_json::json!(model.display_name),
            ));
        }
        if let Some(bundled) = bundled_models.get(model.id.as_str()) {
            if let Some(context_window) = bundled.context_window {
                edits.push(replace_config_value(
                    format!("models.\"{model_alias}\".max_context_size"),
                    serde_json::json!(context_window),
                ));
            }
            if let Some(max_output_tokens) = bundled.capabilities.max_output_tokens {
                edits.push(replace_config_value(
                    format!("models.\"{model_alias}\".max_output_size"),
                    serde_json::json!(max_output_tokens),
                ));
            }
            let capabilities = capability_flags_from_model(bundled);
            if !capabilities.is_empty() {
                edits.push(replace_config_value(
                    format!("models.\"{model_alias}\".capabilities"),
                    serde_json::json!(capabilities),
                ));
            }
        }
    }
    edits.push(clear_config_value("model"));
    edits.push(replace_config_value(
        "default_model",
        serde_json::json!(format!("{alias}/{selected_model_id}")),
    ));
    edits
}

fn bundled_models_by_provider_slug(provider_id: &str) -> HashMap<String, ModelInfo> {
    ody_models_manager::bundled_models_response()
        .map(|response| {
            response
                .models
                .into_iter()
                .filter(|model| model.provider == provider_id)
                .map(|model| (model.slug.clone(), model))
                .collect()
        })
        .unwrap_or_default()
}

fn capability_flags_from_model(model: &ModelInfo) -> Vec<String> {
    let mut flags = Vec::new();
    if model.capabilities.supports_tools || model.supports_parallel_tool_calls {
        flags.push("tool_use".to_string());
    }
    if model.capabilities.supports_thinking || model.supports_reasoning_summaries {
        flags.push("thinking".to_string());
    }
    if model.capabilities.supports_vision {
        flags.push("image_in".to_string());
    }
    flags
}

/// Build config edits that remove a provider and any model aliases that belong
/// to it, and clear the default model if it points to the removed provider.
pub(crate) fn build_logout_provider_edits(
    aliases_to_remove: &[String],
    configured_models: &HashMap<String, OdyCodeModelConfig>,
    default_model: Option<&str>,
) -> Vec<ConfigEdit> {
    use crate::config_update::clear_config_value;

    if aliases_to_remove.is_empty() {
        return Vec::new();
    }

    let mut edits = Vec::new();
    for alias in aliases_to_remove {
        edits.push(clear_config_value(format!("providers.{alias}")));

        let mut matching_models: Vec<&str> = configured_models
            .keys()
            .filter(|key| key.starts_with(&format!("{alias}/")))
            .map(|s| s.as_str())
            .collect();
        matching_models.sort();
        for model_key in matching_models {
            edits.push(clear_config_value(format!("models.\"{model_key}\"")));
        }
    }

    if let Some(model) = default_model {
        if let Some((alias, _)) = model.split_once('/') {
            if aliases_to_remove
                .iter()
                .any(|a| a.eq_ignore_ascii_case(alias))
            {
                edits.push(clear_config_value("default_model"));
            }
        }
    }

    edits
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
