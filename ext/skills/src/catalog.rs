use ody_core_skills::model::SkillDependencies;
use ody_core_skills::model::SkillType;
use ody_protocol::config_types::ModeKind;
use ody_utils_absolute_path::AbsolutePathBuf;

/// Runtime mode used for filtering skill visibility and model invocability.
pub type RuntimeMode = ModeKind;

/// Source authority that owns a skill package and must be used to read it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SkillSourceKind {
    /// Ody-hosted skills, including bundled, user, repo, plugin-installed,
    /// and downloaded/materialized remote skills.
    Host,
    /// Skills owned by an execution environment.
    Executor,
    /// Skills owned by the orchestrator rather than an execution environment.
    Orchestrator,
    /// Extension-private source kind for future providers that do not fit an
    /// existing transport category.
    Custom(String),
}

impl SkillSourceKind {
    pub fn custom(kind: impl Into<String>) -> Self {
        Self::Custom(kind.into())
    }

    fn as_str(&self) -> &str {
        match self {
            Self::Host => "host",
            Self::Executor => "executor",
            Self::Orchestrator => "orchestrator",
            Self::Custom(kind) => kind,
        }
    }
}

impl std::fmt::Display for SkillSourceKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.as_str().fmt(formatter)
    }
}

/// Opaque authority identity for list/read routing.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SkillAuthority {
    pub kind: SkillSourceKind,
    pub id: String,
}

impl SkillAuthority {
    pub fn new(kind: SkillSourceKind, id: impl Into<String>) -> Self {
        Self {
            kind,
            id: id.into(),
        }
    }
}

/// Opaque package id. Callers should not parse local paths out of this value.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SkillPackageId(pub String);

/// Opaque resource id inside a skill package, optionally bound to the
/// environment path that owns its contents.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SkillResourceId {
    id: String,
    environment_path: Option<EnvironmentSkillResource>,
}

impl SkillResourceId {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            environment_path: None,
        }
    }

    pub fn environment(
        id: impl Into<String>,
        environment_id: impl Into<String>,
        path: AbsolutePathBuf,
    ) -> Self {
        Self {
            id: id.into(),
            environment_path: Some(EnvironmentSkillResource {
                environment_id: environment_id.into(),
                path,
            }),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.id
    }

    pub(crate) fn environment_path(&self) -> Option<(&str, &AbsolutePathBuf)> {
        self.environment_path
            .as_ref()
            .map(|resource| (resource.environment_id.as_str(), &resource.path))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct EnvironmentSkillResource {
    environment_id: String,
    path: AbsolutePathBuf,
}

/// Metadata shown in the always-visible skills catalog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillCatalogEntry {
    pub id: SkillPackageId,
    pub authority: SkillAuthority,
    pub name: String,
    pub description: String,
    pub short_description: Option<String>,
    pub main_prompt: SkillResourceId,
    pub display_path: Option<String>,
    pub dependencies: Option<SkillDependencies>,
    pub enabled: bool,
    pub prompt_visible: bool,
    pub skill_type: SkillType,
    pub triggers: Vec<String>,
    pub hidden_in_modes: Vec<ModeKind>,
    pub disable_model_invocation: bool,
}

impl SkillCatalogEntry {
    pub fn new(
        id: SkillPackageId,
        authority: SkillAuthority,
        name: impl Into<String>,
        description: impl Into<String>,
        main_prompt: SkillResourceId,
    ) -> Self {
        Self {
            id,
            authority,
            name: name.into(),
            description: description.into(),
            short_description: None,
            main_prompt,
            display_path: None,
            dependencies: None,
            enabled: true,
            prompt_visible: true,
            skill_type: SkillType::default(),
            triggers: Vec::new(),
            hidden_in_modes: Vec::new(),
            disable_model_invocation: false,
        }
    }

    pub fn with_short_description(mut self, short_description: Option<String>) -> Self {
        self.short_description = short_description;
        self
    }

    pub fn with_display_path(mut self, display_path: impl Into<String>) -> Self {
        self.display_path = Some(display_path.into());
        self
    }

    pub fn with_dependencies(mut self, dependencies: Option<SkillDependencies>) -> Self {
        self.dependencies = dependencies;
        self
    }

    pub fn with_skill_type(mut self, skill_type: SkillType) -> Self {
        self.skill_type = skill_type;
        if matches!(skill_type, SkillType::Knowledge | SkillType::Flow) {
            self.prompt_visible = false;
        }
        self
    }

    pub fn with_triggers(mut self, triggers: Vec<String>) -> Self {
        self.triggers = triggers;
        self
    }

    pub fn with_hidden_in_modes(mut self, hidden_in_modes: Vec<ModeKind>) -> Self {
        self.hidden_in_modes = hidden_in_modes;
        self
    }

    pub fn with_disable_model_invocation(mut self, disable_model_invocation: bool) -> Self {
        self.disable_model_invocation = disable_model_invocation;
        self
    }

    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }

    pub fn hidden_from_prompt(mut self) -> Self {
        self.prompt_visible = false;
        self
    }

    /// Returns whether this entry should appear in the prompt-visible catalog
    /// for the given runtime mode.
    pub fn is_visible_in_mode(&self, mode: RuntimeMode) -> bool {
        self.enabled && self.prompt_visible && !self.hidden_in_modes.contains(&mode)
    }

    /// Returns whether the model is allowed to invoke this entry in the given
    /// runtime mode.
    pub fn is_model_invocable(&self, mode: RuntimeMode) -> bool {
        self.enabled && !self.disable_model_invocation && !self.hidden_in_modes.contains(&mode)
    }

    pub(crate) fn rendered_path(&self) -> &str {
        self.display_path
            .as_deref()
            .unwrap_or_else(|| self.main_prompt.as_str())
    }
}

/// Merged catalog for one turn.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SkillCatalog {
    pub entries: Vec<SkillCatalogEntry>,
    pub warnings: Vec<String>,
}

impl SkillCatalog {
    pub fn extend(&mut self, other: SkillCatalog) {
        for entry in other.entries {
            self.push_entry(entry);
        }
        self.warnings.extend(other.warnings);
    }

    pub fn push_entry(&mut self, entry: SkillCatalogEntry) {
        if self
            .entries
            .iter()
            .any(|existing| existing.authority == entry.authority && existing.id == entry.id)
        {
            return;
        }

        self.entries.push(entry);
    }

    /// Retains only entries that are visible in the given runtime mode.
    ///
    /// Passing `None` leaves the catalog unchanged.
    pub fn filter_for_mode(&mut self, mode: Option<RuntimeMode>) {
        let Some(mode) = mode else {
            return;
        };
        self.entries.retain(|entry| entry.is_visible_in_mode(mode));
    }
}

/// Contents returned after resolving a skill resource through its owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillReadResult {
    pub resource: SkillResourceId,
    pub contents: String,
}

/// Search results for a package whose files are not readable through ordinary
/// executor filesystem access.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SkillSearchResult {
    pub matches: Vec<SkillSearchMatch>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillSearchMatch {
    pub resource: SkillResourceId,
    pub title: String,
    pub snippet: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillProviderError {
    pub message: String,
}

impl SkillProviderError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for SkillProviderError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.message.fmt(formatter)
    }
}

impl std::error::Error for SkillProviderError {}

pub type SkillProviderResult<T> = Result<T, SkillProviderError>;

#[cfg(test)]
mod catalog_tests;
