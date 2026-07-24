use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
pub struct SubmitPlanArgs {
    /// The plan markdown to persist and submit.
    /// If omitted, the host reads the persisted plan from the current plan artifact.
    pub plan: Option<String>,
}
