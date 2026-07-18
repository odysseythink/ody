use std::collections::HashMap;

use ody_utils_absolute_path::AbsolutePathBuf;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;

use crate::config_toml::OdyCodeProviderConfig;
use crate::config_toml::ToolsToml;
use crate::types::AnalyticsConfigToml;
use crate::types::ApprovalsReviewer;
use crate::types::Personality;
use crate::types::SessionPickerViewMode;
use crate::types::WindowsToml;
use ody_features::FeaturesToml;
use ody_protocol::config_types::ReasoningSummary;
use ody_protocol::config_types::SandboxMode;
use ody_protocol::config_types::Verbosity;
use ody_protocol::config_types::WebSearchMode;
use ody_protocol::model_metadata::ReasoningEffort;
use ody_protocol::protocol::AskForApproval;

/// Collection of common configuration options that a user can define as a unit
/// in `config.toml`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ConfigProfile {
    pub model: Option<String>,
    /// Optional explicit service tier request id for new turns (for example
    /// `default`, `priority`, or `flex`; legacy `fast` also works).
    pub service_tier: Option<String>,
    /// The key in the `model_providers` map identifying the
    /// [`ModelProviderInfo`] to use.
    pub model_provider: Option<String>,

    /// Ody-code compatible default provider id.
    #[serde(default)]
    pub default_provider: Option<String>,

    /// Ody-code compatible default model in the form `provider_id/model_name`.
    #[serde(default)]
    pub default_model: Option<String>,

    /// Ody-code compatible provider entries keyed by provider alias.
    #[serde(default)]
    pub providers: Option<HashMap<String, OdyCodeProviderConfig>>,
    pub approval_policy: Option<AskForApproval>,
    pub approvals_reviewer: Option<ApprovalsReviewer>,
    pub sandbox_mode: Option<SandboxMode>,
    pub model_reasoning_effort: Option<ReasoningEffort>,
    pub plan_mode_reasoning_effort: Option<ReasoningEffort>,
    pub model_reasoning_summary: Option<ReasoningSummary>,
    pub model_verbosity: Option<Verbosity>,
    /// Optional path to a JSON model catalog (applied on startup only).
    pub model_catalog_json: Option<AbsolutePathBuf>,
    pub personality: Option<Personality>,
    /// Optional path to a file containing model instructions.
    pub model_instructions_file: Option<AbsolutePathBuf>,
    /// Deprecated: ignored.
    #[schemars(skip)]
    pub js_repl_node_path: Option<AbsolutePathBuf>,
    /// Deprecated: ignored.
    #[schemars(skip)]
    pub js_repl_node_module_dirs: Option<Vec<AbsolutePathBuf>>,
    pub experimental_compact_prompt_file: Option<AbsolutePathBuf>,
    pub include_permissions_instructions: Option<bool>,
    pub include_apps_instructions: Option<bool>,
    pub include_collaboration_mode_instructions: Option<bool>,
    pub include_environment_context: Option<bool>,
    pub experimental_use_unified_exec_tool: Option<bool>,
    pub tools: Option<ToolsToml>,
    pub web_search: Option<WebSearchMode>,
    pub analytics: Option<AnalyticsConfigToml>,
    /// TUI settings scoped to this profile.
    #[serde(default)]
    pub tui: Option<ProfileTui>,
    #[serde(default)]
    pub windows: Option<WindowsToml>,
    /// Optional feature toggles scoped to this profile.
    #[serde(default)]
    // Injects known feature keys into the schema and forbids unknown keys.
    #[schemars(schema_with = "crate::schema::features_schema")]
    pub features: Option<FeaturesToml>,
}

/// TUI settings supported inside a named profile.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(deny_unknown_fields)]
pub struct ProfileTui {
    /// Preferred layout for resume/fork session picker results.
    #[serde(default)]
    pub session_picker_view: Option<SessionPickerViewMode>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_profile_deserializes_ody_code_providers() {
        let profile: ConfigProfile = toml::from_str(
            r#"
default_provider = "kimi_gyy"
default_model = "kimi_gyy/kimi-for-coding"

[providers.kimi_gyy]
type = "kimi"
api_key = "sk-test"
"#,
        )
        .expect("profile with providers should deserialize");

        assert_eq!(profile.default_provider, Some("kimi_gyy".to_string()));
        assert_eq!(
            profile.default_model,
            Some("kimi_gyy/kimi-for-coding".to_string())
        );
        assert!(profile.providers.as_ref().unwrap().contains_key("kimi_gyy"));
        assert_eq!(profile.providers.unwrap()["kimi_gyy"].r#type, "kimi");
    }
}
