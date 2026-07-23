pub(crate) mod cache;
pub mod collaboration_mode_presets;
pub(crate) mod config;
pub mod manager;
pub mod model_info;
pub mod test_support;

pub use config::ModelsManagerConfig;
pub use ody_app_server_protocol::AuthMode;

/// Load the bundled model catalog shipped with `ody-models-manager`.
///
/// Personality metadata is synthesized here rather than stored in `models.json`:
/// the personality template is just each model's `base_instructions` with a
/// `{{ personality }}` placeholder, so baking it into the JSON would duplicate
/// the full (~22 KB) prompt for every model. See
/// [`model_info::personality_messages_from_base_instructions`].
pub fn bundled_models_response()
-> std::result::Result<ody_protocol::model_metadata::ModelsResponse, serde_json::Error> {
    let mut response: ody_protocol::model_metadata::ModelsResponse =
        serde_json::from_str(include_str!("../models.json"))?;
    for model in &mut response.models {
        if model.model_messages.is_none()
            && model_info::provider_supports_personality(&model.provider)
        {
            model.model_messages = Some(model_info::personality_messages_from_base_instructions(
                &model.base_instructions,
            ));
        }
    }
    Ok(response)
}

/// Convert the client version string to a whole version string (e.g. "1.2.3-alpha.4" -> "1.2.3").
pub fn client_version_to_whole() -> String {
    format!(
        "{}.{}.{}",
        env!("CARGO_PKG_VERSION_MAJOR"),
        env!("CARGO_PKG_VERSION_MINOR"),
        env!("CARGO_PKG_VERSION_PATCH")
    )
}
