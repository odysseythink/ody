//! Plan summary for run-before guardian approval (M2.2): a compact,
//! guardian-readable projection of a validated [`FlowPlan`] — phase ids,
//! per-step kind, and bounded prompt previews — sufficient for an
//! approve/deny decision without disclosing full run state.

use ody_core_skills::FlowPlan;
use ody_core_skills::FlowStep;
use serde::Serialize;

/// Preview length cap per template, in characters.
const PROMPT_PREVIEW_CHARS: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct FlowPlanSummary {
    pub(crate) flow_name: String,
    pub(crate) phases: Vec<PhaseSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PhaseSummary {
    pub(crate) id: String,
    pub(crate) steps: Vec<StepSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum StepSummary {
    Agent { preview: String, output: Option<String> },
    Pipeline { items: String, each_preview: String, output: Option<String> },
    Parallel { children: Vec<StepSummary>, output: Option<String> },
}

impl FlowPlanSummary {
    pub(crate) fn new(flow_name: impl Into<String>, plan: &FlowPlan) -> Self {
        Self {
            flow_name: flow_name.into(),
            phases: plan
                .phases
                .iter()
                .map(|phase| PhaseSummary {
                    id: phase.id.clone(),
                    steps: phase.steps.iter().map(step_summary).collect(),
                })
                .collect(),
        }
    }
}

fn step_summary(step: &FlowStep) -> StepSummary {
    match step {
        FlowStep::Agent { agent, output } => {
            StepSummary::Agent { preview: preview(agent), output: output.clone() }
        }
        FlowStep::Pipeline { pipeline, each, output } => StepSummary::Pipeline {
            items: preview(pipeline),
            each_preview: preview(each),
            output: output.clone(),
        },
        FlowStep::Parallel { parallel, output } => StepSummary::Parallel {
            children: parallel.iter().map(step_summary).collect(),
            output: output.clone(),
        },
    }
}

fn preview(template: &str) -> String {
    let mut chars = template.chars();
    let head: String = chars.by_ref().take(PROMPT_PREVIEW_CHARS).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(source: &str) -> FlowPlanSummary {
        let plan = ody_core_skills::parse_flow_plan(source).expect("fixture plan parses");
        FlowPlanSummary::new("demo", &plan)
    }

    #[test]
    fn summary_mirrors_phase_and_step_structure() {
        let summary = summary(
            "phases:\n  - id: design\n    steps:\n      - agent: Generate\n        output: gdd\n  - id: impl\n    steps:\n      - pipeline: ${{ gdd.items }}\n        each: Do ${item}\n        output: results\n      - parallel:\n          - agent: Review\n          - agent: Test\n",
        );
        assert_eq!(summary.flow_name, "demo");
        assert_eq!(summary.phases.len(), 2);
        assert_eq!(summary.phases[0].id, "design");
        assert!(matches!(
            &summary.phases[0].steps[0],
            StepSummary::Agent { preview, output }
                if preview == "Generate" && output.as_deref() == Some("gdd")
        ));
        assert!(matches!(
            &summary.phases[1].steps[0],
            StepSummary::Pipeline { items, each_preview, .. }
                if items == "${{ gdd.items }}" && each_preview == "Do ${item}"
        ));
        match &summary.phases[1].steps[1] {
            StepSummary::Parallel { children, .. } => assert_eq!(children.len(), 2),
            other => panic!("expected parallel summary, got {other:?}"),
        }
    }

    #[test]
    fn preview_truncates_long_templates() {
        let long = "x".repeat(500);
        let summary = summary(&format!("phases:\n  - id: p\n    steps:\n      - agent: {long}\n"));
        match &summary.phases[0].steps[0] {
            StepSummary::Agent { preview, .. } => {
                assert!(preview.ends_with('…'));
                assert_eq!(preview.chars().count(), 201); // 200 + ellipsis
            }
            other => panic!("expected agent summary, got {other:?}"),
        }
    }
}
