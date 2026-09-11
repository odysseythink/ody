//! Declarative flow plan types for `SkillType::Flow` skills.
//!
//! A flow skill carries a `flow.yaml` artifact next to its SKILL.md. The
//! loader validates the artifact at load time; the host-side runtime
//! (M1.1, `core/src/flow/`) consumes the validated plan. Host-function
//! semantics (`agent` / `pipeline` / `parallel`) are defined here and are
//! the single source of truth that future Starlark/V8 runtimes must match.

use serde::Deserialize;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowPlan {
    /// Informational metadata mirroring SKILL.md frontmatter for readability of the
    /// artifact alone; the loader never reads these — authoritative name/description
    /// always live in the skill's SKILL.md.
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    pub phases: Vec<FlowPhase>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowPhase {
    pub id: String,
    #[serde(default)]
    pub steps: Vec<FlowStep>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum FlowStep {
    /// Run one agent with the given prompt; optionally bind its result to
    /// `output` for use by later steps via `${{ name }}` interpolation.
    Agent {
        agent: String,
        #[serde(default)]
        output: Option<String>,
    },
    /// Fan out one agent per item produced by the `pipeline` expression.
    Pipeline {
        pipeline: String,
        each: String,
        #[serde(default)]
        output: Option<String>,
    },
    /// Run the given steps concurrently and wait for all of them.
    Parallel {
        parallel: Vec<FlowStep>,
        #[serde(default)]
        output: Option<String>,
    },
}

#[derive(Debug)]
pub struct FlowParseError(String);

impl fmt::Display for FlowParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid flow.yaml: {}", self.0)
    }
}

impl std::error::Error for FlowParseError {}

pub fn parse_flow_plan(yaml: &str) -> Result<FlowPlan, FlowParseError> {
    let plan: FlowPlan = serde_yaml::from_str(yaml).map_err(|e| FlowParseError(e.to_string()))?;
    if plan.phases.is_empty() {
        return Err(FlowParseError("phases must not be empty".to_string()));
    }
    Ok(plan)
}

#[cfg(test)]
#[path = "flow_tests.rs"]
mod flow_tests;
