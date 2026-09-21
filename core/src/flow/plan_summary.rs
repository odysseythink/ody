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

/// Extract static `phase("...")` / `phase('...')` title literals from a
/// script carrier source, in order of first appearance, deduplicated.
///
/// Deliberately a hand-rolled scanner (no regex): only statically knowable
/// calls are extracted — `phase(title)` with a variable or an f-string is
/// skipped (the call still shows up in the source preview). Matches require
/// an identifier boundary before `phase(`, so `myphase("x")` is not a hit.
fn extract_phase_literals(source: &str) -> Vec<String> {
    let mut phases: Vec<String> = Vec::new();
    for line in source.lines() {
        let mut rest = line;
        while let Some(pos) = rest.find("phase(") {
            // Identifier boundary: the char before `phase` must not be an
            // identifier or member-access character (covers `myphase(` and
            // `x.phase(`).
            let boundary_ok = pos == 0
                || !rest[..pos]
                    .chars()
                    .last()
                    .map(|c| c.is_alphanumeric() || c == '_' || c == '.')
                    .unwrap_or(false);
            let after = &rest[pos + "phase(".len()..];
            if boundary_ok {
                if let Some(literal) = parse_string_literal(after.trim_start()) {
                    if !phases.contains(&literal) {
                        phases.push(literal);
                    }
                }
            }
            rest = &rest[pos + 1..];
        }
    }
    phases
}

/// Parse a leading `"..."` / `'...'` string literal (with backslash escape
/// handling); `None` for anything else (variables, f-strings, numbers).
fn parse_string_literal(text: &str) -> Option<String> {
    let mut chars = text.chars();
    let quote = match chars.next() {
        Some('"') | Some('\'') => text.chars().next().unwrap(),
        _ => return None,
    };
    let mut literal = String::new();
    let mut escaped = false;
    for c in chars {
        if escaped {
            literal.push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == quote {
            return Some(literal);
        } else {
            literal.push(c);
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct FlowPlanSummary {
    pub(crate) flow_name: String,
    /// Carrier artifact (`flow.yaml` / `flow.star` / `workflow.js`).
    pub(crate) carrier: &'static str,
    /// Structured phase/step projection. For script carriers this is the
    /// extracted `phase("...")` title checklist (no steps).
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
        // Guardian preview upgrade: static `phase("...")` literals become a
        // phase title checklist on top of the bounded source preview.
        let phases = extract_phase_literals(source)
            .into_iter()
            .map(|id| PhaseSummary { id, steps: Vec::new() })
            .collect();
        Self {
            flow_name: flow_name.into(),
            carrier,
            phases,
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
        // Static phase literals surface as a title checklist (2026-09-21).
        assert_eq!(summary.phases.len(), 1);
        assert_eq!(summary.phases[0].id, "x");
        assert!(summary.phases[0].steps.is_empty());
        assert_eq!(summary.source_preview.as_deref(), Some("phase('x')"));
    }

    /// V8 arm of the decision-8 dispatch (M3.3 anchor): a validated
    /// `workflow.js` plan projects to a script summary; static `phase()`
    /// literals become the title checklist here too.
    #[cfg(feature = "flow-v8")]
    #[test]
    fn for_plan_marks_v8_carrier() {
        let plan = FlowPlanSource::V8("var result = 1;\n".to_string());
        let summary = FlowPlanSummary::for_plan("demo", &plan);
        assert_eq!(summary.carrier, "workflow.js");
        assert!(summary.phases.is_empty());
        assert_eq!(summary.source_preview.as_deref(), Some("var result = 1;"));
    }

    // ---- Static phase literal extraction (2026-09-21) ----

    #[test]
    fn extract_phase_literals_keeps_order_and_dedupes() {
        let source = r#"
phase("first")
phase('second')
phase("first")
phase(title)
phase(f"dynamic {title}")
myphase("not a phase call")
x.phase("method call is not a hit")
"#;
        assert_eq!(
            extract_phase_literals(source),
            vec!["first".to_string(), "second".to_string()]
        );
    }

    #[test]
    fn extract_phase_literals_handles_escapes_and_empty() {
        assert_eq!(
            extract_phase_literals(r#"phase("say \"hi\"")"#),
            vec!["say \"hi\"".to_string()]
        );
        // Unterminated / non-string arguments are skipped.
        assert!(extract_phase_literals("phase(\n").is_empty());
        assert!(extract_phase_literals("phase(42)").is_empty());
    }

    #[test]
    fn script_summary_without_literals_keeps_preview_only() {
        let source = "result = agent('plain')\n";
        let summary = FlowPlanSummary::for_script("demo", "flow.star", source);
        assert!(summary.phases.is_empty());
        assert_eq!(summary.source_preview.as_deref(), Some(source.trim_end()));
    }

    /// The shipped `/game` flow.star must project its full phase title
    /// checklist for the run-before guardian approval.
    #[test]
    fn game_flow_script_summary_lists_all_phase_titles() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../skills/src/assets/embedded/game/flow.star");
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("embedded game flow.star should be readable: {err}"));
        let summary = FlowPlanSummary::for_script("game", "flow.star", &source);
        let titles: Vec<&str> = summary.phases.iter().map(|phase| phase.id.as_str()).collect();
        let expected = [
            "triage · 意图识别",
            "A1 · 概念收敛",
            "A2 · GDD",
            "A3 · 技术选型",
            "A4 · 按机制并行实现",
            "A5 · 试玩与审查",
            "B1 · 项目定位",
            "B2 · 商业 GDD",
            "B3 · 架构选型",
            "C · 实施辅助",
        ];
        assert_eq!(titles, expected, "guardian phase checklist drifted");
        // The source preview is still present alongside the checklist.
        assert!(summary.source_preview.is_some());
    }
}
