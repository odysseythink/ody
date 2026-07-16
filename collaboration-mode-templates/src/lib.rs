pub const PLAN: &str = include_str!("../templates/plan.md");
pub const DESIGN: &str = include_str!("../templates/design.md");
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
pub const PLAN_RIGOR_SPLIT: &str = include_str!("../templates/plan_rigor_split.md");
pub const PLAN_RIGOR_TURN_DISCIPLINE: &str =
    include_str!("../templates/plan_rigor_turn_discipline.md");

#[cfg(test)]
mod template_tests {
    use super::*;

    /// The templates may only name tools ody-rs actually registers. The
    /// inherited-from-ody-code wording named `Read/Grep/Glob`, which ody-rs did
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
                "{name} names Read/Grep/Glob; ody-rs registers `read_file`/`grep`/`glob`"
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

    /// Every `## Parts` table example the model can see must be copy-ready.
    ///
    /// Regression: PLAN_RIGOR_SPLIT's examples used `<id>/core.md` and `<stem>/protocol-core.md`,
    /// and its File-column rule read "always `<id>/`" — an instruction, not a placeholder. Its
    /// worked example was internally inconsistent too: a concrete index (`2026-07-10-design-mode.md`)
    /// beside a Parts table full of `<stem>/`. Meanwhile PLAN's example used bare `core.md`. A rigor
    /// prompt therefore showed three different formats for one column, and a real plan shipped with
    /// `<stem>/core-widget.md` rows.
    ///
    /// The core tolerates it — `normalize_part_path` keeps only the basename — so nothing failed at
    /// submit time. The damage lands on whoever reads the index next: the executing agent looked for
    /// a `<stem>` directory and found nothing.
    ///
    /// The `File` cell is a bare file name. `submit_plan` supplies the directory at runtime.
    #[test]
    fn parts_table_examples_use_bare_copy_ready_file_names() {
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
                     Models copy example tables verbatim; write the cell as it should appear on disk."
                );
                assert!(
                    !cell.contains('/'),
                    "{name}: Parts example File cell {cell:?} has a directory prefix. The File cell \
                     is a bare file name — `normalize_part_path` keeps only the basename, and \
                     submit_plan supplies the directory."
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

    fn understand_step_of(body: &str) -> String {
        body.lines()
            .find(|line| line.starts_with("1. **Understand**"))
            .unwrap_or_default()
            .to_string()
    }
}
