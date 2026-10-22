use ody_protocol::config_types::ModeAvailability;
use ody_protocol::model_metadata::ModelPreset;
use std::convert::Infallible;

#[derive(Debug, Clone)]
pub(crate) struct ModelCatalog {
    models: Vec<ModelPreset>,
    /// Which opt-in collaboration modes this invocation may offer. Carried here
    /// because every mode-list decision in the TUI starts from the catalog, and
    /// a mode the user cannot enter must never reach the mode ring.
    mode_availability: ModeAvailability,
}

impl ModelCatalog {
    pub(crate) fn new(models: Vec<ModelPreset>) -> Self {
        Self {
            models,
            mode_availability: ModeAvailability::default(),
        }
    }

    pub(crate) fn with_mode_availability(mut self, mode_availability: ModeAvailability) -> Self {
        self.mode_availability = mode_availability;
        self
    }

    pub(crate) fn mode_availability(&self) -> ModeAvailability {
        self.mode_availability
    }

    pub(crate) fn try_list_models(&self) -> Result<Vec<ModelPreset>, Infallible> {
        Ok(self.models.clone())
    }
}
