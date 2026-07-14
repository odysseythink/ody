pub const PLAN: &str = include_str!("../templates/plan.md");
pub const DESIGN: &str = include_str!("../templates/design.md");
pub const DEFAULT: &str = include_str!("../templates/default.md");
pub const EXECUTE: &str = include_str!("../templates/execute.md");
pub const PAIR_PROGRAMMING: &str = include_str!("../templates/pair_programming.md");
pub const PLAN_RIGOR_WORKFLOW: &str = include_str!("../templates/plan_rigor_workflow.md");
pub const PLAN_RIGOR_COVERAGE: &str = include_str!("../templates/plan_rigor_coverage.md");
pub const PLAN_RIGOR_TASK_SKELETON: &str = include_str!("../templates/plan_rigor_task_skeleton.md");
pub const PLAN_RIGOR_SELFREVIEW: &str = include_str!("../templates/plan_rigor_selfreview.md");
pub const PLAN_RIGOR_INVARIANTS: &str = include_str!("../templates/plan_rigor_invariants.md");
pub const PLAN_RIGOR_GROUNDING: &str = include_str!("../templates/plan_rigor_grounding.md");
pub const PLAN_RIGOR_SCOPE: &str = include_str!("../templates/plan_rigor_scope.md");
pub const PLAN_RIGOR_RENAME: &str = include_str!("../templates/plan_rigor_rename.md");
pub const PLAN_RIGOR_RISKS: &str = include_str!("../templates/plan_rigor_risks.md");
pub const PLAN_RIGOR_SPLIT: &str = include_str!("../templates/plan_rigor_split.md");
pub const PLAN_RIGOR_TURN_DISCIPLINE: &str =
    include_str!("../templates/plan_rigor_turn_discipline.md");

#[cfg(test)]
mod template_tests {
    use super::*;

    /// ody-rs ships no Read/Grep/Glob tools — the model-facing surface is
    /// `shell_command`/`exec_command` (see core/src/tools/handlers/*_spec.rs).
    /// Naming them sends the model looking for tools that do not exist, and it
    /// falls back to raw `rg`/`cat` shell calls, the most context-expensive
    /// exploration path available. The wording was inherited verbatim from
    /// ody-code, which does ship those tools.
    #[test]
    fn plan_templates_do_not_name_nonexistent_tools() {
        for (name, body) in [("PLAN", PLAN), ("PLAN_RIGOR_WORKFLOW", PLAN_RIGOR_WORKFLOW)] {
            assert!(
                !body.contains("Read/Grep/Glob"),
                "{name} still names the nonexistent Read/Grep/Glob tools"
            );
        }
    }

    /// The conversational template and the rigor addendum are two different
    /// delivery paths for the same Workflow step 1: PLAN goes into the session
    /// prompt, while PLAN_RIGOR_WORKFLOW is re-injected by
    /// `render_full_reminder()` (core/src/plan_mode_injector.rs). If the two
    /// drift, a long plan-mode session receives contradictory exploration
    /// guidance mid-flight.
    #[test]
    fn plan_and_rigor_workflow_share_the_same_understand_step() {
        let understand_step = understand_step_of(PLAN);
        assert_eq!(
            understand_step,
            understand_step_of(PLAN_RIGOR_WORKFLOW),
            "PLAN and PLAN_RIGOR_WORKFLOW must carry an identical Workflow step 1"
        );
        assert!(
            !understand_step.is_empty(),
            "neither template exposes a Workflow step 1 to compare"
        );
    }

    fn understand_step_of(body: &str) -> String {
        body.lines()
            .find(|line| line.starts_with("1. **Understand**"))
            .unwrap_or_default()
            .to_string()
    }
}
