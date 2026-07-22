pub const PLAN: &str = include_str!("../templates/plan.md");
pub const DESIGN: &str = include_str!("../templates/design.md");
pub const DESIGN_FULL_REMINDER: &str = include_str!("../templates/design_full_reminder.md");
pub const DESIGN_SPARSE_REMINDER: &str = include_str!("../templates/design_sparse_reminder.md");
pub const DEFAULT: &str = include_str!("../templates/default.md");
pub const EXECUTE: &str = include_str!("../templates/execute.md");
pub const PAIR_PROGRAMMING: &str = include_str!("../templates/pair_programming.md");
pub const PLAN_CONCISE: &str = include_str!("../templates/plan_concise.md");
pub const PLAN_RIGOR_WORKFLOW: &str = include_str!("../templates/plan_rigor_workflow.md");
pub const PLAN_RIGOR_COVERAGE: &str = include_str!("../templates/plan_rigor_coverage.md");
pub const PLAN_RIGOR_TASK_SKELETON: &str = include_str!("../templates/plan_rigor_task_skeleton.md");
pub const PLAN_RIGOR_SELFREVIEW: &str = include_str!("../templates/plan_rigor_selfreview.md");
pub const PLAN_RIGOR_INVARIANTS: &str = include_str!("../templates/plan_rigor_invariants.md");
pub const PLAN_RIGOR_GROUNDING: &str = include_str!("../templates/plan_rigor_grounding.md");
pub const PLAN_RIGOR_SCOPE: &str = include_str!("../templates/plan_rigor_scope.md");
pub const PLAN_RIGOR_RENAME: &str = include_str!("../templates/plan_rigor_rename.md");
pub const PLAN_RIGOR_RISKS: &str = include_str!("../templates/plan_rigor_risks.md");
pub const PLAN_RIGOR_SPIKE: &str = include_str!("../templates/plan_rigor_spike.md");
pub const PLAN_RIGOR_SPLIT: &str = include_str!("../templates/plan_rigor_split.md");
pub const PLAN_RIGOR_TURN_DISCIPLINE: &str =
    include_str!("../templates/plan_rigor_turn_discipline.md");

#[cfg(test)]
mod template_tests {
    use super::*;

    /// The templates may only name tools ody actually registers. The
    /// inherited-from-ody-code wording named `Read/Grep/Glob`, which ody did
    /// not ship; the model looked for them, found nothing, and fell back to raw
    /// `rg`/`cat` shell calls — the most context-expensive exploration path
    /// available. The tools now exist under their real names, so pin those.
    ///
    /// If a template ever names a tool again, cross-check it against the handler
    /// names in `core/src/tools/handlers/file_tools_spec.rs`.
    #[test]
    fn plan_templates_only_name_tools_that_exist() {
        for (name, body) in [("PLAN", PLAN), ("PLAN_RIGOR_WORKFLOW", PLAN_RIGOR_WORKFLOW)] {
            assert!(
                !body.contains("Read/Grep/Glob"),
                "{name} names Read/Grep/Glob; ody registers `read_file`/`grep`/`glob`"
            );
            for tool in ["`grep`", "`glob`", "`read_file`"] {
                assert!(
                    body.contains(tool),
                    "{name} must steer exploration at {tool} — otherwise the model \
                     defaults back to shelling out to rg/cat"
                );
            }
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

    /// Every `## Parts` File cell the model can see must be openable exactly as written.
    ///
    /// The cell is the manifest's locator: an index is routinely handed to a downstream reader — a
    /// human, or the agent that executes the plan — as text and nothing else. So it needs a real
    /// directory *and* a real file name.
    ///
    /// Two ways to fail it, both shipped:
    ///   - `<stem>/core-widget.md` — placeholder never substituted; the executing agent went looking
    ///     for a literal `<stem>` directory. The examples taught this: `<id>/core.md`, a File-column
    ///     rule reading "always `<id>/`", and a worked example pairing a concrete index with
    ///     `<stem>/` rows.
    ///   - `widget-core.md` — bare name; resolvable by nobody who does not already know the
    ///     directory. This was the over-correction for the first, and it is just as unusable.
    ///
    /// `normalize_part_path` keeps only the basename, so the core accepts all three forms and the
    /// damage is invisible at submit time. That makes this test the guard, not the runtime.
    #[test]
    fn parts_table_examples_are_openable_as_written() {
        for (name, body) in [("PLAN", PLAN), ("PLAN_RIGOR_SPLIT", PLAN_RIGOR_SPLIT)] {
            let rows = parts_table_file_cells(body);
            assert!(
                !rows.is_empty(),
                "{name} exposes no `## Parts` example rows to check — did the table format change?"
            );
            for cell in rows {
                assert!(
                    !cell.contains('<') && !cell.contains('>'),
                    "{name}: Parts example File cell {cell:?} carries an unsubstituted placeholder. \
                     Models copy example tables verbatim, so an example must be a finished artifact."
                );
                assert!(
                    cell.contains('/'),
                    "{name}: Parts example File cell {cell:?} has no directory. The cell is how a \
                     reader locates the part; a bare file name does not say where it lives."
                );
                assert!(
                    cell.ends_with(".md"),
                    "{name}: Parts example File cell {cell:?} is not a markdown file"
                );
            }
        }
    }

    /// Extracts the `File` column of every markdown row under a `## Parts` heading.
    fn parts_table_file_cells(body: &str) -> Vec<String> {
        let mut cells = Vec::new();
        let mut in_parts = false;
        for line in body.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("## ") {
                in_parts = trimmed == "## Parts";
                continue;
            }
            if !in_parts || !trimmed.starts_with('|') {
                continue;
            }
            let columns: Vec<&str> = trimmed.trim_matches('|').split('|').collect();
            let Some(file) = columns.get(1).map(|c| c.trim()) else {
                continue;
            };
            // Skip the header (`File`) and the `|---|` separator.
            if file.is_empty() || file == "File" || file.starts_with("---") {
                continue;
            }
            cells.push(file.trim_matches('`').to_string());
        }
        cells
    }

    /// Design mode must explicitly require the popup tool for closed-choice questions and
    /// must end with a next-action prompt. Otherwise models fall back to plain-text A/B lists.
    #[test]
    fn design_template_mandates_request_user_input_for_options_and_exit_prompt() {
        assert!(
            DESIGN.contains("**MUST** call `request_user_input`"),
            "DESIGN must mandate request_user_input for option-based questions"
        );
        assert!(
            DESIGN.contains("Enter Plan mode"),
            "DESIGN must include the Enter Plan mode next-action option"
        );
        assert!(
            DESIGN.contains("Compact and enter Plan mode"),
            "DESIGN must include the Compact and enter Plan mode next-action option"
        );
        assert!(
            DESIGN.contains("Stay in Design mode"),
            "DESIGN must include the Stay in Design mode next-action option"
        );
    }

    fn understand_step_of(body: &str) -> String {
        body.lines()
            .find(|line| line.starts_with("1. **Understand**"))
            .unwrap_or_default()
            .to_string()
    }
}
