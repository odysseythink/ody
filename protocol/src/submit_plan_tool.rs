use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct SubmitPlanArgs {
    /// The initial or complete non-frozen replacement plan markdown to persist.
    /// For a frozen split manifest, use `task_id`/`status` instead of resending it.
    /// If all fields are omitted, the host reads the persisted plan.
    #[ts(optional)]
    pub plan: Option<String>,
    /// Frozen-manifest task to advance without resending or regenerating the
    /// complete index markdown.
    #[ts(optional)]
    pub task_id: Option<String>,
    /// New task status. Currently `done` is the normal checkpoint transition.
    #[ts(optional)]
    pub status: Option<String>,
}
