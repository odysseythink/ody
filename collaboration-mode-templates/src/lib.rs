pub const PLAN: &str = include_str!("../templates/plan.md");
pub const DESIGN: &str = include_str!("../templates/design.md");
pub const DESIGN_FULL_REMINDER: &str = include_str!("../templates/design_full_reminder.md");
pub const DESIGN_SPARSE_REMINDER: &str = include_str!("../templates/design_sparse_reminder.md");
pub const DEFAULT: &str = include_str!("../templates/default.md");
pub const EXTERNAL_GROUNDING: &str = include_str!("../templates/external_grounding.md");
pub const EXECUTE: &str = include_str!("../templates/execute.md");
pub const PAIR_PROGRAMMING: &str = include_str!("../templates/pair_programming.md");
pub const PLAN_CONCISE: &str = include_str!("../templates/plan_concise.md");
pub const PRODUCT: &str = include_str!("../templates/product.md");
pub const PRODUCT_FULL_REMINDER: &str = include_str!("../templates/product_full_reminder.md");
pub const PRODUCT_REENTRY: &str = include_str!("../templates/product_reentry.md");
pub const PRODUCT_SPARSE_REMINDER: &str = include_str!("../templates/product_sparse_reminder.md");
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

    #[test]
    fn external_grounding_requires_search_then_original_sources() {
        for requirement in [
            "classify external research as **Required** or **Not required**",
            "Search-result snippets are discovery hints, not evidence",
            "at least two distinct external URLs",
            "at least one primary source",
            "## External Evidence",
        ] {
            assert!(
                EXTERNAL_GROUNDING.contains(requirement),
                "shared external-grounding contract must contain {requirement:?}"
            );
        }
    }

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

    #[test]
    fn split_templates_require_complete_detail_and_freeze_the_initial_manifest() {
        for (name, body) in [("PLAN", PLAN), ("PLAN_RIGOR_SPLIT", PLAN_RIGOR_SPLIT)] {
            assert!(
                body.contains("Do not replace") || body.contains("Do not compress"),
                "{name} must forbid replacing concrete plan detail with a summary"
            );
            assert!(
                !body.contains("byte budget"),
                "{name} must not encourage compacting a plan to a default byte budget"
            );
            assert!(
                body.contains("{{ max_part_bytes }}"),
                "{name} must expose the configured byte limit before part writing"
            );
            assert!(
                body.contains("frozen"),
                "{name} must prohibit repartitioning after the initial index"
            );
        }
    }

    #[test]
    fn plan_templates_require_risk_driven_verification_with_actionable_e2e_contracts() {
        assert!(
            PLAN.contains("## Risk-driven verification strategy (every tier)"),
            "PLAN must require every tier to choose a verification level"
        );
        for requirement in [
            "Stable pass condition",
            "Environment and isolation",
            "Execution owner",
            "Logs may aid diagnosis but must not be the sole proof of success",
            "Not applicable",
        ] {
            assert!(
                PLAN.contains(requirement),
                "PLAN verification strategy must define {requirement:?}"
            );
        }
        for requirement in [
            "### Verification level and acceptance",
            "**Verification:**",
            "controlled E2E or smoke verification",
            "A log line is diagnostic evidence, never the only pass condition",
        ] {
            assert!(
                PLAN_RIGOR_TASK_SKELETON.contains(requirement),
                "PLAN_RIGOR_TASK_SKELETON must define {requirement:?}"
            );
        }
    }

    #[test]
    fn templates_require_conditional_state_change_impact_analysis() {
        for (name, body) in [
            ("DESIGN", DESIGN),
            ("PLAN", PLAN),
            ("PLAN_RIGOR_SELFREVIEW", PLAN_RIGOR_SELFREVIEW),
        ] {
            assert!(
                body.contains("cross-boundary observable state"),
                "{name} must scope state-change analysis to observable state"
            );
            assert!(
                body.contains("State-change impact surface: N/A")
                    || body.contains("Mutation Surface: N/A"),
                "{name} must provide an explicit non-applicable path"
            );
        }
        assert!(
            PLAN.contains("A task that changes only static display text is **not** a state change"),
            "PLAN must prevent static text changes from being misclassified as state mutations"
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
            DESIGN.contains("Clear context and enter Plan mode"),
            "DESIGN must include the Clear context and enter Plan mode next-action option"
        );
        assert!(
            DESIGN.contains("Stay in Design mode"),
            "DESIGN must include the Stay in Design mode next-action option"
        );
    }

    /// The product-mode P0 triage contract: one entry point, three internal
    /// paths (direction undecided / direction settled / change-request), so the
    /// complexity of the requirements-analysis workflow lives inside the mode's
    /// routing instead of in the user's choice of which mode to enter.
    #[test]
    fn product_template_pins_p0_triage_contract() {
        for requirement in [
            // P0 定轨: exactly three paths, anchored to the book's two entry
            // templates (new-system vs change/optimization).
            "Path A",
            "Path B",
            "Path C",
            "change/optimization",
            // Turn discipline inherited from the ody-code product mode.
            "ONE question at a time",
            "`request_user_input`",
            // Confidence and evidence tagging.
            "[C:USER]",
            "[C:INFERRED]",
            "[V:TRANSACTED]",
            "[V:OBSERVED]",
            "[V:STATED]",
            // The book's four-level priority vocabulary.
            "must-do",
            "should-do",
            "could-do",
            "wont-do",
            // Three-tier hard gate: requirement models allowed, implementation
            // details forbidden.
            "no code",
            "mermaid",
            // Artifact location.
            ".ody-code/products/",
        ] {
            assert!(
                PRODUCT.contains(requirement),
                "PRODUCT must contain {requirement:?}"
            );
        }
    }

    /// Periodic reminders for product mode: the full reminder re-injects the
    /// complete operating contract (turn discipline, evidence grading, hard
    /// gate, priority vocabulary) on the full cadence; the sparse reminder
    /// keeps the load-bearing rules alive between full reinjections. Both
    /// must never drift from the pinned contract in `product.md`.
    #[test]
    fn product_reminders_restate_the_load_bearing_rules() {
        for requirement in [
            // Mode identity — the model must never confuse this with a
            // plan/design reminder.
            "Product Mode",
            // Turn discipline: one question at a time via pop-ups.
            "request_user_input",
            "ONE question at a time",
            // Evidence grading and confidence tagging.
            "[V:STATED]",
            "[C:INFERRED]",
            // The book's four-level priority vocabulary.
            "must-do",
            "wont-do",
        ] {
            assert!(
                PRODUCT_FULL_REMINDER.contains(requirement),
                "PRODUCT_FULL_REMINDER must contain {requirement:?}"
            );
            assert!(
                PRODUCT_SPARSE_REMINDER.contains(requirement),
                "PRODUCT_SPARSE_REMINDER must contain {requirement:?}"
            );
        }
        // The full reminder alone carries the hard gate (no code) — the rule
        // the whole mode exists to enforce.
        assert!(
            PRODUCT_FULL_REMINDER.contains("no code"),
            "PRODUCT_FULL_REMINDER must restate the no-code hard gate:\n{}",
            PRODUCT_FULL_REMINDER
        );
    }

    /// Re-entry contract for product mode: when a requirements document
    /// already exists on disk (continued session, re-entering the mode, or
    /// cross-day work), the model must continue that document instead of
    /// restarting P0 triage. The host interpolates `{{ product_path }}` with
    /// the restored artifact path before appending this fragment.
    #[test]
    fn product_reentry_pins_the_continue_dont_restart_contract() {
        for requirement in [
            "{{ product_path }}",
            "Do NOT restart P0 triage",
            "request_user_input",
            "ONE question at a time",
            "## Open Questions",
            "[V:TRANSACTED|OBSERVED|STATED]",
            "must-do",
            "no code",
        ] {
            assert!(
                PRODUCT_REENTRY.contains(requirement),
                "PRODUCT_REENTRY must contain {requirement:?}"
            );
        }
    }

    /// The mode-ending contract: the model confirms handoff with the user via
    /// `request_user_input`, then calls `submit_product` (no arguments) so the
    /// host can finalize the document and the client can show the handoff
    /// menu. Without this pin the model asks the user to switch modes
    /// manually — the auto-exit mechanism never fires.
    #[test]
    fn product_template_ends_with_submit_product_handoff() {
        for requirement in [
            "submit_product",
            "request_user_input",
            "ONE question",
            "Enter Plan Mode",
            "Enter Design Mode",
        ] {
            assert!(
                PRODUCT.contains(requirement),
                "PRODUCT ending must contain {requirement:?}"
            );
        }
    }

    fn understand_step_of(body: &str) -> String {
        body.lines()
            .find(|line| line.starts_with("1. **Understand**"))
            .unwrap_or_default()
            .to_string()
    }
}
