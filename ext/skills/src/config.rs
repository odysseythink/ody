/// Host-supplied configuration used by the skills extension.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkillsExtensionConfig {
    /// Whether the available-skills catalog is included in model context.
    pub include_instructions: bool,
    /// Whether bundled skills are eligible for discovery.
    pub bundled_skills_enabled: bool,
    /// Whether orchestrator-owned skills are eligible for discovery.
    pub orchestrator_skills_enabled: bool,
    /// Whether knowledge microagents are enabled.
    pub knowledge_microagents_enabled: bool,
    /// Maximum number of knowledge skills to inject per turn.
    pub knowledge_max_skills_per_turn: usize,
    /// Maximum total bytes of knowledge skill contents to inject per turn.
    pub knowledge_max_contents_bytes: usize,
    /// Whether model tools are enabled on the host model.
    pub host_model_tools_enabled: bool,
    /// Whether model tools are enabled on the executor model.
    pub executor_model_tools_enabled: bool,
}

impl Default for SkillsExtensionConfig {
    fn default() -> Self {
        Self {
            include_instructions: true,
            bundled_skills_enabled: true,
            orchestrator_skills_enabled: true,
            knowledge_microagents_enabled: true,
            knowledge_max_skills_per_turn: 3,
            knowledge_max_contents_bytes: 8_000,
            host_model_tools_enabled: true,
            executor_model_tools_enabled: true,
        }
    }
}
