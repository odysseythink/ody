use std::path::PathBuf;

use ody_protocol::user_input::UserInput;

/// Host-owned turn environment summary visible to turn-input contributors.
#[derive(Debug, Clone)]
pub struct TurnInputEnvironment {
    /// Stable host environment id used to route executor-scoped capabilities.
    pub environment_id: String,
    /// Effective working directory for this turn in the environment.
    pub cwd: PathBuf,
    /// Whether this is the primary environment for the turn.
    pub is_primary: bool,
}

/// Turn facts supplied before the host records turn-local model input items.
#[derive(Debug, Clone)]
pub struct TurnInputContext {
    /// Stable host-owned turn identifier.
    pub turn_id: String,
    /// User input submitted for this turn.
    pub user_input: Vec<UserInput>,
    /// Resolved turn environments, in host priority order.
    pub environments: Vec<TurnInputEnvironment>,
    /// Effective model context window for this turn, when the host knows it.
    ///
    /// Contributors that spend prompt space on their own content use this to size
    /// their budget against the real window instead of guessing at a constant.
    pub model_context_window: Option<i64>,
}
