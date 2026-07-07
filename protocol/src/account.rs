use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use ts_rs::TS;

use crate::auth::KnownPlan;
use crate::auth::PlanType as AuthPlanType;

/// Account state returned by a model provider before it is adapted to an app-facing wire type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderAccount {
    ApiKey,
}