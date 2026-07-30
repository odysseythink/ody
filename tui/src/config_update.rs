//! App-server-backed config update helpers for the TUI.
//!
//! This module centralizes the small typed update helpers the TUI uses
//! when a config mutation must be owned by the app server rather than written
//! to the local `config.toml` directly.

use color_eyre::eyre::Result;
use color_eyre::eyre::WrapErr;
use ody_app_server_client::AppServerRequestHandle;
use ody_app_server_protocol::ClientRequest;
use ody_app_server_protocol::ConfigBatchWriteParams;
use ody_app_server_protocol::ConfigEdit;
use ody_app_server_protocol::ConfigReadParams;
use ody_app_server_protocol::ConfigReadResponse;
use ody_app_server_protocol::ConfigWriteResponse;
use ody_app_server_protocol::MergeStrategy;
use ody_app_server_protocol::RequestId;
use ody_app_server_protocol::SkillsConfigWriteParams;
use ody_app_server_protocol::SkillsConfigWriteResponse;
use ody_config::loader::project_trust_key;
use ody_features::FEATURES;
use ody_protocol::config_types::SERVICE_TIER_DEFAULT_REQUEST_VALUE;
use ody_protocol::config_types::TrustLevel;
use ody_utils_absolute_path::AbsolutePathBuf;
use serde_json::Value as JsonValue;
use std::fmt::Display;
use std::path::Path;
use uuid::Uuid;

use crate::legacy_core::config::Config;
use ody_config::config_toml::DesignReviewToml;
use ody_config::config_toml::UsabilityLensToml;

pub(crate) fn replace_config_value(key_path: impl Into<String>, value: JsonValue) -> ConfigEdit {
    ConfigEdit {
        key_path: key_path.into(),
        value,
        merge_strategy: MergeStrategy::Replace,
    }
}

pub(crate) fn clear_config_value(key_path: impl Into<String>) -> ConfigEdit {
    replace_config_value(key_path, JsonValue::Null)
}

pub(crate) fn app_scoped_key_path(app_id: &str, key_path: &str) -> String {
    let app_id = serde_json::Value::String(app_id.to_string()).to_string();
    format!("apps.{app_id}.{key_path}")
}

pub(crate) fn format_config_error(err: &impl Display) -> String {
    format!("{err:#}")
}

fn trusted_project_edit(project_path: &Path) -> ConfigEdit {
    let project_key = project_trust_key(project_path)
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    replace_config_value(
        format!("projects.\"{project_key}\".trust_level"),
        serde_json::json!(TrustLevel::Trusted.to_string()),
    )
}

pub(crate) fn build_model_selection_edits(
    provider_id: &str,
    model: &str,
    effort: Option<impl ToString>,
) -> Vec<ConfigEdit> {
    let effort_edit = effort.map_or_else(
        || clear_config_value("model_reasoning_effort"),
        |effort| {
            replace_config_value(
                "model_reasoning_effort",
                serde_json::json!(effort.to_string()),
            )
        },
    );
    vec![
        clear_config_value("model"),
        replace_config_value(
            "default_model",
            serde_json::json!(format!("{provider_id}/{model}")),
        ),
        effort_edit,
    ]
}

pub(crate) fn build_service_tier_selection_edits(service_tier: Option<&str>) -> Vec<ConfigEdit> {
    let service_tier_edit = service_tier.map_or_else(
        || clear_config_value("service_tier"),
        |service_tier| {
            let config_value = if service_tier == SERVICE_TIER_DEFAULT_REQUEST_VALUE {
                SERVICE_TIER_DEFAULT_REQUEST_VALUE
            } else {
                match ody_protocol::config_types::ServiceTier::from_request_value(service_tier) {
                    Some(ody_protocol::config_types::ServiceTier::Fast) => "fast",
                    Some(ody_protocol::config_types::ServiceTier::Flex) => "flex",
                    None => service_tier,
                }
            };
            replace_config_value("service_tier", serde_json::json!(config_value))
        },
    );
    vec![service_tier_edit]
}

#[cfg(target_os = "windows")]
pub(crate) fn build_windows_sandbox_mode_edits(elevated_enabled: bool) -> Vec<ConfigEdit> {
    let feature_key_path = |feature: &str| format!("features.{feature}");
    vec![
        replace_config_value(
            "windows.sandbox",
            serde_json::json!(if elevated_enabled {
                "elevated"
            } else {
                "unelevated"
            }),
        ),
        clear_config_value(feature_key_path("experimental_windows_sandbox")),
        clear_config_value(feature_key_path("elevated_windows_sandbox")),
        clear_config_value(feature_key_path("enable_experimental_windows_sandbox")),
    ]
}

pub(crate) fn build_feature_enabled_edit(feature_key: &str, enabled: bool) -> ConfigEdit {
    let key_path = format!("features.{feature_key}");
    let is_default_false_feature = FEATURES
        .iter()
        .find(|spec| spec.key == feature_key)
        .is_some_and(|spec| !spec.default_enabled);
    if enabled || !is_default_false_feature {
        replace_config_value(key_path, serde_json::json!(enabled))
    } else {
        clear_config_value(key_path)
    }
}

pub(crate) fn build_memory_settings_edits(
    use_memories: bool,
    generate_memories: bool,
) -> Vec<ConfigEdit> {
    vec![
        replace_config_value("memories.use_memories", serde_json::json!(use_memories)),
        replace_config_value(
            "memories.generate_memories",
            serde_json::json!(generate_memories),
        ),
    ]
}

/// Editable snapshot of the raw `[design_review]` config keys.
///
/// Populated from the *resolved* fields on [`Config`] (which already apply the legacy
/// `design_review_model` / `review_model` fallback chain) so the form seeds with the
/// effective values, but edits are written back to the raw `design_review` table.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct DesignReviewEditState {
    pub(crate) enable: bool,
    pub(crate) review_model: Option<String>,
    pub(crate) debate_enable: bool,
    pub(crate) rounds: Option<u8>,
    pub(crate) advocate_model: Option<String>,
    pub(crate) skeptic_model: Option<String>,
    pub(crate) judge_model: Option<String>,
    pub(crate) contest_critic: bool,
    pub(crate) usability_lens: UsabilityLensToml,
}

impl DesignReviewEditState {
    pub(crate) fn from_config(config: &Config) -> Self {
        let mut state = Self {
            enable: config.design_review_enabled,
            review_model: config.design_review_model.clone(),
            ..Default::default()
        };
        if let Some(debate) = &config.design_review_debate {
            state.debate_enable = debate.enable;
            state.rounds = debate.rounds;
            state.advocate_model = debate.advocate_model.clone();
            state.skeptic_model = debate.skeptic_model.clone();
            state.judge_model = debate.judge_model.clone();
            state.contest_critic = debate.contest_critic;
            state.usability_lens = debate.usability_lens;
        }
        state
    }

    pub(crate) fn apply_from_toml(&mut self, toml: &DesignReviewToml) {
        self.enable = toml.enable;
        self.review_model = toml.review_model.clone();
        if let Some(debate) = &toml.debate {
            self.debate_enable = debate.enable;
            self.rounds = debate.rounds;
            self.advocate_model = debate.advocate_model.clone();
            self.skeptic_model = debate.skeptic_model.clone();
            self.judge_model = debate.judge_model.clone();
            self.contest_critic = debate.contest_critic;
            self.usability_lens = debate.usability_lens;
        } else {
            self.debate_enable = false;
            self.rounds = None;
            self.advocate_model = None;
            self.skeptic_model = None;
            self.judge_model = None;
            self.contest_critic = false;
            self.usability_lens = UsabilityLensToml::default();
        }
    }
}

/// Build a batch of [`ConfigEdit`]s that rewrite the raw `[design_review]` table to match
/// `state`. Every key is emitted so the on-disk table reflects the UI exactly; `None`
/// string values are cleared rather than left stale.
pub(crate) fn build_design_review_edits(state: &DesignReviewEditState) -> Vec<ConfigEdit> {
    vec![
        replace_config_value("design_review.enable", serde_json::json!(state.enable)),
        state.review_model.as_ref().map_or_else(
            || clear_config_value("design_review.review_model"),
            |model| replace_config_value("design_review.review_model", serde_json::json!(model)),
        ),
        replace_config_value(
            "design_review.debate.enable",
            serde_json::json!(state.debate_enable),
        ),
        state.rounds.map_or_else(
            || clear_config_value("design_review.debate.rounds"),
            |rounds| replace_config_value("design_review.debate.rounds", serde_json::json!(rounds)),
        ),
        state.advocate_model.as_ref().map_or_else(
            || clear_config_value("design_review.debate.advocate_model"),
            |model| replace_config_value(
                "design_review.debate.advocate_model",
                serde_json::json!(model),
            ),
        ),
        state.skeptic_model.as_ref().map_or_else(
            || clear_config_value("design_review.debate.skeptic_model"),
            |model| replace_config_value(
                "design_review.debate.skeptic_model",
                serde_json::json!(model),
            ),
        ),
        state.judge_model.as_ref().map_or_else(
            || clear_config_value("design_review.debate.judge_model"),
            |model| replace_config_value(
                "design_review.debate.judge_model",
                serde_json::json!(model),
            ),
        ),
        replace_config_value(
            "design_review.debate.contest_critic",
            serde_json::json!(state.contest_critic),
        ),
        replace_config_value(
            "design_review.debate.usability_lens",
            serde_json::json!(state.usability_lens),
        ),
    ]
}

pub(crate) async fn write_config_batch(
    request_handle: AppServerRequestHandle,
    edits: Vec<ConfigEdit>,
) -> Result<ConfigWriteResponse> {
    let request_id = RequestId::String(format!("tui-config-write-{}", Uuid::new_v4()));
    request_handle
        .request_typed(ClientRequest::ConfigBatchWrite {
            request_id,
            params: ConfigBatchWriteParams {
                edits,
                file_path: None,
                expected_version: None,
                reload_user_config: true,
            },
        })
        .await
        .wrap_err("config/batchWrite failed in TUI")
}

pub(crate) async fn write_trusted_project(
    request_handle: AppServerRequestHandle,
    project_path: &Path,
) -> Result<ConfigWriteResponse> {
    write_config_batch(request_handle, vec![trusted_project_edit(project_path)]).await
}

pub(crate) async fn read_effective_config(
    request_handle: AppServerRequestHandle,
    cwd: String,
) -> Result<ConfigReadResponse> {
    let request_id = RequestId::String(format!("tui-config-read-{}", Uuid::new_v4()));
    request_handle
        .request_typed(ClientRequest::ConfigRead {
            request_id,
            params: ConfigReadParams {
                include_layers: false,
                cwd: Some(cwd),
            },
        })
        .await
        .wrap_err("config/read failed in TUI")
}

pub(crate) async fn write_skill_enabled(
    request_handle: AppServerRequestHandle,
    path: AbsolutePathBuf,
    enabled: bool,
) -> Result<()> {
    let request_id = RequestId::String(format!("tui-skill-config-write-{}", Uuid::new_v4()));
    let _: SkillsConfigWriteResponse = request_handle
        .request_typed(ClientRequest::SkillsConfigWrite {
            request_id,
            params: SkillsConfigWriteParams {
                path: Some(path),
                name: None,
                enabled,
            },
        })
        .await
        .wrap_err("skills/config/write failed in TUI")?;
    Ok(())
}

#[cfg(test)]
#[path = "config_update_tests.rs"]
mod tests;
