//! Plan summary for run-before guardian approval (M2.2): a compact,
//! guardian-readable projection of a validated [`FlowPlan`] — phase ids,
//! per-step kind, and bounded prompt previews — sufficient for an
//! approve/deny decision without disclosing full run state.

use ody_core_skills::FlowPlan;
use ody_core_skills::FlowStep;
use serde::Serialize;

use super::FlowPlanSource;

/// Preview length cap per template, in characters.
const PROMPT_PREVIEW_CHARS: usize = 200;
/// Script-carrier source preview cap, in lines (M3).
const SOURCE_PREVIEW_LINES: usize = 40;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct FlowPlanSummary {
    pub(crate) flow_name: String,
    /// Carrier artifact (`flow.yaml` / `flow.star` / `workflow.js`).
    pub(crate) carrier: &'static str,
    /// Structured phase/step projection; empty for script carriers.
    pub(crate) phases: Vec<PhaseSummary>,
    /// Head of the script source for script carriers; `None` for yaml.
    pub(crate) source_preview: Option<String>,
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
            carrier: "flow.yaml",
            phases: plan
                .phases
                .iter()
                .map(|phase| PhaseSummary {
                    id: phase.id.clone(),
                    steps: phase.steps.iter().map(step_summary).collect(),
                })
                .collect(),
            source_preview: None,
        }
    }

    /// Project any validated plan for the run-before approval: structured
    /// phases for yaml, a bounded source preview for script carriers (M3).
    pub(crate) fn for_plan(flow_name: impl Into<String>, plan: &FlowPlanSource) -> Self {
        match plan {
            FlowPlanSource::Yaml(plan) => Self::new(flow_name, plan),
            #[cfg(feature = "flow-starlark")]
            FlowPlanSource::Starlark(source) => {
                Self::for_script(flow_name, "flow.star", source)
            }
            #[cfg(feature = "flow-v8")]
            FlowPlanSource::V8(source) => Self::for_script(flow_name, "workflow.js", source),
        }
    }

    fn for_script(
        flow_name: impl Into<String>,
        carrier: &'static str,
        source: &str,
    ) -> Self {
        let head: Vec<&str> = source.lines().take(SOURCE_PREVIEW_LINES).collect();
        let preview = if head.len() < source.lines().count() {
            format!("{}\n…", head.join("\n"))
        } else {
            head.join("\n")
        };
        Self {
            flow_name: flow_name.into(),
            carrier,
            phases: Vec::new(),
            source_preview: Some(preview),
        }
    }
}

fn step_summary(step: &FlowStep) -> StepSummary {
    match step {
        FlowStep::Agent { agent, output, schema: _ } => {
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

    #[test]
    fn script_summary_previews_first_forty_lines() {
        let source: String =
            (1..=45).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        let summary = FlowPlanSummary::for_script("demo", "workflow.js", &source);
        assert_eq!(summary.flow_name, "demo");
        assert_eq!(summary.carrier, "workflow.js");
        assert!(summary.phases.is_empty());
        let preview = summary.source_preview.expect("script source preview");
        let lines: Vec<&str> = preview.lines().collect();
        assert_eq!(lines.len(), 41); // 40 head lines + ellipsis marker
        assert_eq!(lines[0], "line 1");
        assert_eq!(lines[39], "line 40");
        assert_eq!(lines[40], "…");
    }

    #[test]
    fn short_script_summary_omits_ellipsis() {
        let source = "phase('x')\nresult = 1\n";
        let summary = FlowPlanSummary::for_script("demo", "flow.star", source);
        assert_eq!(
            summary.source_preview.as_deref(),
            Some("phase('x')\nresult = 1")
        );
    }

    /// Decision 8: `for_plan` dispatches per carrier; the starlark arm is
    /// the default-build anchor for the script-carrier summary path used
    /// by the run-before guardian approval.
    #[cfg(feature = "flow-starlark")]
    #[test]
    fn for_plan_marks_starlark_carrier() {
        let plan = FlowPlanSource::Starlark("phase('x')\n".to_string());
        let summary = FlowPlanSummary::for_plan("demo", &plan);
        assert_eq!(summary.carrier, "flow.star");
        assert!(summary.phases.is_empty());
        assert_eq!(summary.source_preview.as_deref(), Some("phase('x')"));
    }

    /// V8 arm of the decision-8 dispatch (M3.3 anchor): a validated
    /// `workflow.js` plan projects to a script summary, never structured
    /// phases.
    #[cfg(feature = "flow-v8")]
    #[test]
    fn for_plan_marks_v8_carrier() {
        let plan = FlowPlanSource::V8("var result = 1;\n".to_string());
        let summary = FlowPlanSummary::for_plan("demo", &plan);
        assert_eq!(summary.carrier, "workflow.js");
        assert!(summary.phases.is_empty());
        assert_eq!(summary.source_preview.as_deref(), Some("var result = 1;"));
    }
}
