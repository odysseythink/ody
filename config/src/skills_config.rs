//! Skill-related configuration types shared across crates.

use ody_utils_absolute_path::AbsolutePathBuf;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;

const fn default_enabled() -> bool {
    true
}

const fn default_max_skills() -> usize {
    3
}

const fn default_max_contents_bytes() -> usize {
    8_000
}

fn is_default_max_skills(value: &usize) -> bool {
    *value == default_max_skills()
}

fn is_default_max_contents_bytes(value: &usize) -> bool {
    *value == default_max_contents_bytes()
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct SkillConfig {
    /// Path-based selector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<AbsolutePathBuf>,
    /// Name-based selector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub enabled: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct SkillsConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundled: Option<BundledSkillsConfig>,

    /// Whether turns receive the automatic skills instructions block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_instructions: Option<bool>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub config: Vec<SkillConfig>,

    /// Whether knowledge microagents may be loaded from the project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub knowledge_microagents_enabled: Option<bool>,

    /// Maximum number of knowledge skills that may be loaded in a single turn.
    #[serde(default = "default_max_skills", skip_serializing_if = "is_default_max_skills")]
    pub knowledge_max_skills_per_turn: usize,

    /// Maximum total size of knowledge skill contents in bytes.
    #[serde(
        default = "default_max_contents_bytes",
        skip_serializing_if = "is_default_max_contents_bytes"
    )]
    pub knowledge_max_contents_bytes: usize,

    /// Whether model tools are enabled in the host (UI) model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_model_tools_enabled: Option<bool>,

    /// Whether model tools are enabled in the executor (subagent) model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor_model_tools_enabled: Option<bool>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct BundledSkillsConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

impl Default for BundledSkillsConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

impl TryFrom<toml::Value> for SkillsConfig {
    type Error = toml::de::Error;

    fn try_from(value: toml::Value) -> Result<Self, Self::Error> {
        SkillsConfig::deserialize(value)
    }
}
