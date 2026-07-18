//! Shared submit core for Plan and Design mode artifact persistence.
//!
//! Extracted from `submit_plan.rs`; both `SubmitPlanHandler` and
//! `SubmitDesignHandler` are thin shells that call `handle_submit_artifact`.

use crate::design_completeness::design_completeness_report;
use crate::design_review::orchestrator::DesignReviewOrchestrator;
use crate::design_review::orchestrator::format_review_appendix_for_submit;
use crate::design_review::types::DesignReviewRequest;
use crate::function_tool::FunctionCallError;
use crate::plan_artifact::PlanWriteOutcome;
use crate::plan_mode_injector::parts_manifest::RowStatus;
use crate::plan_mode_injector::parts_manifest::parse_parts_manifest;
use crate::plan_mode_injector::parts_manifest::part_file_cell_problem;
use crate::plan_mode_injector::parts_manifest::row_is_verified_done;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use ody_config::config_toml::PlanModeTier;
use ody_protocol::config_types::ModeKind;
use ody_protocol::items::PlanItem;
use ody_protocol::items::TurnItem;
use ody_protocol::protocol::EventMsg;
use ody_protocol::protocol::PlanDeltaEvent;
use ody_protocol::protocol::WarningEvent;
use regex_lite::Regex;
use std::path::Path;
use std::path::PathBuf;
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Wording: mode-parameterized copy for messages, tool names, and paths
// ---------------------------------------------------------------------------

/// Mode-specific wording bundle so the shared core produces the right tool
/// name, directory path, and noun in every message without branching on mode.
pub(crate) struct SubmitWording {
    pub tool_name: &'static str,
    pub mode_name: &'static str,
    pub noun: &'static str,
    pub out_dir: &'static str,
}

pub(crate) static PLAN_WORDING: SubmitWording = SubmitWording {
    tool_name: "submit_plan",
    mode_name: "Plan",
    noun: "plan",
    out_dir: ".ody-code/plans/",
};

pub(crate) static DESIGN_WORDING: SubmitWording = SubmitWording {
    tool_name: "submit_design",
    mode_name: "Design",
    noun: "design",
    out_dir: ".ody-code/designs/",
};

const REQUIRED_SELF_REVIEW_ITEMS: usize = 7;

// ---------------------------------------------------------------------------
// Private helpers (moved verbatim from submit_plan.rs)
// ---------------------------------------------------------------------------

/// Returns a description of what a rigor-tier plan is missing, or `None` if it
/// satisfies the structural bar the rigor-tier prompt fragments (`plan_rigor_coverage`,
/// `plan_rigor_selfreview`) mandate: a `## Spec coverage` table and a `## Self-review`
/// section reproducing all seven checklist items. Models reliably reference these
/// requirements ("all seven items") without actually including the sections, so this
/// is checked mechanically rather than trusted.
/// Every `## Parts` File cell in `markdown` that cannot be followed as written, explained.
///
/// Empty when the plan has no manifest (a single-file plan) or every cell is usable.
fn parts_manifest_cell_problems(stem_dir: &Path, markdown: &str) -> Vec<String> {
    parse_parts_manifest(markdown)
        .manifest
        .map(|manifest| {
            manifest
                .rows
                .iter()
                .filter_map(|row| part_file_cell_problem(stem_dir, &row.file))
                .collect()
        })
        .unwrap_or_default()
}

fn rigor_structure_gap(plan: &str) -> Option<String> {
    let lower = plan.to_lowercase();
    let has_spec_coverage =
        lower.contains("## spec coverage") || lower.contains("## spec-coverage");
    let self_review_heading = lower
        .find("## self-review")
        .or_else(|| lower.find("## self review"));

    match (has_spec_coverage, self_review_heading) {
        (false, None) => Some(
            "missing both the `## Spec coverage` table and the `## Self-review` section"
                .to_string(),
        ),
        (false, Some(_)) => Some("missing the `## Spec coverage` table".to_string()),
        (true, None) => Some("missing the `## Self-review` section".to_string()),
        (true, Some(heading_idx)) => {
            let section_start = heading_idx + "## self-review".len();
            let section = &lower[section_start..];
            let section_end = section.find("\n## ").unwrap_or(section.len());
            let checklist_items = section[..section_end].matches("- [ ]").count()
                + section[..section_end].matches("- [x]").count();
            if checklist_items < REQUIRED_SELF_REVIEW_ITEMS {
                Some(format!(
                    "its `## Self-review` section has only {checklist_items} checklist item(s); all {REQUIRED_SELF_REVIEW_ITEMS} items must be reproduced"
                ))
            } else {
                None
            }
        }
    }
}

/// Counts `Task N` headings (## to ####, case-insensitive, optional
/// punctuation after the number) — the convention every observed plan and
/// template example uses for the "Tasks" section. Used to mechanically detect
/// a plan that should have been split per the base template's "Large plan
/// splitting" rule but wasn't. Deliberately conservative: headings that only
/// contain the word "Task" without a number (e.g. "### Task Breakdown",
/// "### Tasks") are NOT counted.
fn count_task_headings(plan: &str) -> usize {
    static TASK_HEADING_RE: OnceLock<Regex> = OnceLock::new();
    let re = TASK_HEADING_RE.get_or_init(|| Regex::new(r"(?i)^#{2,4}\s+task\s+\d").unwrap());
    plan.lines()
        .filter(|line| re.is_match(line.trim_start()))
        .count()
}

/// Returns a description of why a single-file (no `## Parts` table) plan
/// should have been split, or `None` if it is within the configured
/// `split_threshold`. Models reliably talk themselves into "this exceeds the
/// threshold, I should split" and then submit a single file anyway, so this
/// is checked mechanically rather than trusted.
fn split_threshold_gap(plan: &str, split_threshold: usize) -> Option<String> {
    // 0 disables splitting (config_toml.rs schema doc); without this guard
    // every plan with at least one task heading would be rejected.
    if split_threshold == 0 {
        return None;
    }
    let task_count = count_task_headings(plan);
    if task_count > split_threshold {
        Some(format!(
            "has {task_count} task headings but no `## Parts` table — the plan-mode instructions require splitting into multiple files once a plan exceeds {split_threshold} distinct tasks (see \"Large plan splitting\"). Task headings must look like `### Task N` (## to #### allowed, `Task` case-insensitive, N a number); rename non-task headings so they are not counted."
        ))
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Shared core
// ---------------------------------------------------------------------------

/// Persist a plan or design artifact to disk, emit lifecycle events, detect
/// split-plan pending parts, and either mark the turn terminal (all parts done,
/// completeness gate passed) or return a message keeping the session in the
/// current mode.
///
/// # Contract
///
/// 1. Mode guard — rejects calls where `turn.collaboration_mode.mode != expected_mode`.
/// 2. Done-parts guard — rejects bare index submissions that would silently
///    discard previously-verified done parts.
/// 3. Events — emits `TurnItem::Plan(started)`, `PlanDelta`, `Warning` (on
///    persist failure), and `TurnItem::Plan(completed)`.
/// 4. Persist — calls `artifact.write_plan(&markdown, persist)`.
/// 5. Checkpoint — when `finalize` is `false`, the artifact is persisted and
///    shown but the turn stays in the current mode (never terminal). This lets
///    Design mode checkpoint a partial/skeleton design each turn without the
///    completeness gate accidentally finalizing it.
/// 6. Pending-parts detection — returns non-terminal message with `stem_dir`
///    path when manifest has pending rows.
/// 7. Terminal candidate — for Plan: `split_threshold_gap` then
///    `rigor_structure_gap`; for Design: `design_completeness_report`.
/// 8. Terminal — calls `artifact.mark_submitted()` and returns the submitted
///    message.
pub(crate) fn should_trigger_design_review(
    expected_mode: ModeKind,
    finalize: bool,
    gap: Option<&str>,
    has_pending_parts: bool,
    review_model: Option<&str>,
) -> bool {
    expected_mode == ModeKind::Design
        && finalize
        && gap.is_none()
        && !has_pending_parts
        && review_model.is_some()
}

pub(crate) async fn handle_submit_artifact(
    invocation: ToolInvocation,
    expected_mode: ModeKind,
    wording: &SubmitWording,
    markdown: String,
    finalize: bool,
) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
    let ToolInvocation {
        session,
        turn,
        call_id: _,
        payload,
        ..
    } = invocation;

    // Reject non-Function payloads (same guard as submit_plan.rs:137-144).
    let _arguments = match payload {
        ToolPayload::Function { arguments } => arguments,
        _ => {
            return Err(FunctionCallError::RespondToModel(format!(
                "{} handler received unsupported payload",
                wording.tool_name
            )));
        }
    };

    // 1. Mode guard
    if turn.collaboration_mode.mode != expected_mode {
        return Err(FunctionCallError::RespondToModel(format!(
            "{} is only available in {} mode",
            wording.tool_name, wording.mode_name
        )));
    }

    // 2. Artifact guard
    let Some(artifact) = turn.plan_artifact.as_ref() else {
        return Err(FunctionCallError::RespondToModel(format!(
            "{} unavailable: no {} artifact",
            wording.tool_name, wording.noun
        )));
    };

    // 3. Done-parts guard (verbatim from submit_plan.rs:160-181)
    let previously_had_done_parts = artifact
        .last_manifest_snapshot()
        .is_some_and(|snapshot| snapshot.done_count > 0);
    if previously_had_done_parts && parse_parts_manifest(&markdown).manifest.is_none() {
        return Err(FunctionCallError::RespondToModel(format!(
            "{} rejected: the previous turn's {} had a `## Parts` table with pending rows, but this submission has no `## Parts` table at all. This call was not persisted. Resubmit the full index markdown (Goal/Architecture/File Structure/etc. plus the `## Parts` table with every row's current status) — {} always writes the single index file, never an individual part's content.",
            wording.tool_name, wording.noun, wording.tool_name
        )));
    }

    // 4. Emit start event
    //
    // No path yet, deliberately. The artifact is still `Temporary` here — its real name is derived
    // from the markdown's title inside `write_plan` below — so the only path available at this
    // point is the `tmp-<thread>-<date>.md` placeholder, which is never written and never exists.
    // Reporting it made the TUI show a dead path; `None` correctly says "not persisted yet".
    let item_id = format!("{}-{}", turn.sub_id, wording.noun);

    session
        .emit_turn_item_started(
            turn.as_ref(),
            &TurnItem::Plan(PlanItem {
                id: item_id.clone(),
                text: String::new(),
                plan_file_path: None,
            }),
        )
        .await;

    session
        .send_event(
            turn.as_ref(),
            EventMsg::PlanDelta(PlanDeltaEvent {
                thread_id: session.thread_id.to_string(),
                turn_id: turn.sub_id.clone(),
                item_id: item_id.clone(),
                delta: markdown.clone(),
            }),
        )
        .await;

    // 5. Persist
    let persist = turn
        .config
        .plan_mode
        .as_ref()
        .and_then(|pm| pm.persist_plan_file)
        .unwrap_or(true);
    let outcome = artifact.write_plan(&markdown, persist).await;

    // The name is only decided inside `write_plan`, so this is the earliest point the real path
    // exists. Take it from the outcome rather than re-reading the artifact: `Written` carries the
    // exact path that was written, and the other outcomes genuinely have no file to point at.
    let persisted_path = match &outcome {
        PlanWriteOutcome::Written { path } => Some(PathBuf::from(path)),
        PlanWriteOutcome::InlineOnly | PlanWriteOutcome::Failed { .. } => None,
    };

    if let PlanWriteOutcome::Failed { error } = &outcome {
        session
            .send_event(
                turn.as_ref(),
                EventMsg::Warning(WarningEvent {
                    message: format!("Failed to persist {}: {error}", wording.noun),
                }),
            )
            .await;
    }

    // Rows whose `File` cell cannot be followed as written. Collected here because `markdown` is
    // moved into the completed event below, and reported in the response: such a row can never
    // verify, so without naming it the model would be told to keep writing parts every turn with no
    // clue why the ones it already wrote do not count.
    let bad_part_cells: Vec<String> = match artifact.stem_dir() {
        Some(stem_dir) => parts_manifest_cell_problems(&stem_dir, &markdown),
        None => Vec::new(),
    };

    // 6. Pending parts detection
    let has_pending_parts = match artifact.stem_dir() {
        Some(stem_dir) => parse_parts_manifest(&markdown)
            .manifest
            .is_some_and(|manifest| {
                manifest
                    .rows
                    .iter()
                    .any(|row| !row_is_verified_done(&stem_dir, row))
            }),
        None => parse_parts_manifest(&markdown)
            .manifest
            .is_some_and(|manifest| {
                manifest
                    .rows
                    .iter()
                    .any(|row| row.status == RowStatus::Pending)
            }),
    };

    // 7. Gap detection — mode-dependent. Skipped for non-final checkpoints:
    // a checkpoint is never terminal, so the completeness/rigor gate (whose
    // sole effect is to gate finalization) does not apply.
    let gap: Option<String> = if !finalize || has_pending_parts {
        None
    } else if expected_mode == ModeKind::Design {
        // Design: C1–C8 completeness (replaces plan-specific split/rigor checks)
        design_completeness_report(&markdown)
    } else {
        // Plan: existing split-threshold + rigor logic (verbatim)
        if parse_parts_manifest(&markdown).manifest.is_none() {
            let split_threshold = turn
                .config
                .plan_mode
                .as_ref()
                .and_then(|pm| pm.split_threshold)
                .unwrap_or(8);
            split_threshold_gap(&markdown, split_threshold).or_else(|| {
                if artifact.plan_mode_tier() == Some(PlanModeTier::Rigor) {
                    rigor_structure_gap(&markdown)
                } else {
                    None
                }
            })
        } else if artifact.plan_mode_tier() == Some(PlanModeTier::Rigor) {
            rigor_structure_gap(&markdown)
        } else {
            None
        }
    };

    // 7.5 — automatic adversarial review after structural gate
    let review_appendix: Option<String> = if should_trigger_design_review(
        expected_mode,
        finalize,
        gap.as_deref(),
        has_pending_parts,
        turn.config.review_model.as_deref(),
    ) {
        let request = DesignReviewRequest {
            design_markdown: markdown.clone(),
            review_model: turn.config.review_model.clone().expect("checked above"),
        };
        match DesignReviewOrchestrator::review(&session, &turn, request).await {
            Ok(output) => Some(format_review_appendix_for_submit(&output)),
            Err(err) => {
                let message = format!("Design review failed: {err}");
                session
                    .send_event(turn.as_ref(), EventMsg::Warning(WarningEvent { message }))
                    .await;
                Some(format!("\n\n[Design review could not be completed: {err}]"))
            }
        }
    } else {
        None
    };

    // 8. Emit completed event
    session
        .emit_turn_item_completed(
            turn.as_ref(),
            TurnItem::Plan(PlanItem {
                id: item_id,
                text: markdown,
                plan_file_path: persisted_path,
            }),
        )
        .await;

    // 9. Build response message
    //
    // A row whose `File` cell is still a placeholder can never verify, so without naming those rows
    // the model would be told "keep writing parts" every turn with no clue why the parts it already
    // wrote do not count. Computed before `markdown` is moved into the completed event above.
    // A bad `File` cell must block finalization, not just colour the mid-flight message. A bare
    // `core.md` still resolves — only the basename is used — so the rows verify, `has_pending_parts`
    // goes false, and a plan whose manifest points nowhere would otherwise sail straight through to
    // `submitted`. Placed before the `gap` arm so it applies to every tier, not just Rigor.
    let bad_cells_message = (!bad_part_cells.is_empty()).then(|| {
        format!(
            "{} saved, but these ## Parts File cells cannot be followed as written: {}. Each cell must be the part's path relative to the index with the real directory filled in — for this plan, `{}/<part-name>.md`. This call was persisted but is NOT final: whoever reads the index next may have only its text, so the cell has to be openable as written. Fix the cells and call {} again with the corrected index.",
            wording.noun,
            bad_part_cells.join("; "),
            artifact
                .stem_dir()
                .and_then(|dir| dir.file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_default(),
            wording.tool_name
        )
    });

    let message = if let Some(bad_cells) = bad_cells_message {
        bad_cells
    } else if has_pending_parts {
        match artifact.stem_dir() {
            Some(stem_dir) => format!(
                "{} part saved. This {} is split into multiple parts and pending parts remain — stay in {} mode and keep writing them one per turn; do not treat this call as final. Write each part file at exactly {}/<part-name>.md (use the file names from the ## Parts table).",
                wording.noun,
                wording.noun,
                wording.mode_name,
                stem_dir.display()
            ),
            None => format!(
                "{} part saved. This {} is split into multiple parts and pending parts remain — stay in {} mode and keep writing them one per turn; do not treat this call as final.",
                wording.noun, wording.noun, wording.mode_name
            ),
        }
    } else if !finalize {
        // Checkpoint: persisted and shown, but explicitly non-terminal. The
        // completeness gate was skipped; do not mark the artifact submitted.
        format!(
            "{} checkpoint saved — still in {} mode (not final). Keep building the {}; call {} with `final: true` only once it is complete and you intend to exit.",
            wording.noun, wording.mode_name, wording.noun, wording.tool_name
        )
    } else if let Some(g) = gap {
        if expected_mode == ModeKind::Design {
            format!(
                "{} saved, but it is incomplete: {g} This call was persisted but is NOT final — stay in {} mode, add the missing section(s), and call {} again with the complete design.",
                wording.noun, wording.mode_name, wording.tool_name
            )
        } else {
            format!(
                "Plan part saved, but this is a rigor-tier plan and it is {g}. This call was persisted but is NOT treated as final — stay in Plan mode, add the missing section(s) per the rigor-tier addendum instructions, and call submit_plan again with the complete plan."
            )
        }
    } else {
        artifact.mark_submitted();
        format!("{} submitted", wording.noun)
    };

    let message = if let Some(appendix) = review_appendix {
        format!("{message}\n\n{appendix}")
    } else {
        message
    };

    Ok(boxed_tool_output(FunctionToolOutput::from_text(
        message,
        Some(true),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_with(file_cells: [&str; 2]) -> String {
        format!(
            "# Plan\n\n## Parts\n| # | File | Scope | Status |\n|---|---|---|---|\n\
             | 1 | `{}` | core | done |\n| 2 | `{}` | tests | done |\n",
            file_cells[0], file_cells[1]
        )
    }

    /// Both manifests below actually shipped, and both left the executing agent unable to open the
    /// parts. Neither failed at submit time, because only the basename is used to resolve a cell.
    #[test]
    fn parts_manifest_cell_problems_flags_both_shipped_failures() {
        let stem = Path::new("/plans/2026-07-16-pinnedtodowidget_implementation_plan");

        // Shipped once: the template's own examples used this notation, so the model copied it.
        let placeholders = manifest_with(["<stem>/core-widget.md", "<stem>/tests.md"]);
        let problems = parts_manifest_cell_problems(stem, &placeholders);
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems[0].contains("placeholder"), "{problems:?}");

        // Shipped next: bare names. Resolvable only by someone who already knows the directory —
        // which the reader of a handed-over index does not.
        let bare = manifest_with(["widget-core.md", "tests.md"]);
        let problems = parts_manifest_cell_problems(stem, &bare);
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(
            problems[0].contains("2026-07-16-pinnedtodowidget_implementation_plan/widget-core.md"),
            "the report must spell out the path to use: {problems:?}"
        );

        // The form that works: real directory, real file name, nothing to expand.
        let good = manifest_with([
            "2026-07-16-pinnedtodowidget_implementation_plan/widget-core.md",
            "2026-07-16-pinnedtodowidget_implementation_plan/tests.md",
        ]);
        assert!(parts_manifest_cell_problems(stem, &good).is_empty());
    }

    /// A single-file plan has no manifest and must not be dragged into this check.
    #[test]
    fn parts_manifest_cell_problems_ignores_plans_without_a_manifest() {
        let stem = Path::new("/plans/2026-07-16-topic");
        let plan = "# Plan\n\n## Summary\n\nNo parts here.\n";
        assert!(parts_manifest_cell_problems(stem, plan).is_empty());
    }

    // Re-exported from submit_plan.rs's test module — same helpers, same tests.
    // They remain here because the functions they test moved here.
    // (The submit_plan.rs test module will be trimmed to only test the thin shell.)

    fn self_review_with(n: usize) -> String {
        let items: String = (0..n).map(|_| "- [ ] item\n").collect();
        format!("## Self-review\n\n{items}")
    }

    fn tasks_with(n: usize) -> String {
        (1..=n)
            .map(|i| format!("### Task {i}: do something\n\nsome detail\n\n"))
            .collect()
    }

    #[test]
    fn split_threshold_gap_none_when_within_threshold() {
        let plan = format!("# Plan\n\n{}", tasks_with(3));
        assert_eq!(split_threshold_gap(&plan, 3), None);
    }

    #[test]
    fn split_threshold_gap_flags_exceeding_threshold() {
        let plan = format!("# Plan\n\n{}", tasks_with(13));
        let gap = split_threshold_gap(&plan, 3).expect("13 tasks exceeds threshold 3");
        assert!(
            gap.contains("13"),
            "gap should mention the actual task count: {gap}"
        );
        assert!(gap.contains("## Parts"));
    }

    #[test]
    fn split_threshold_gap_flags_real_world_13_task_single_file_plan() {
        let plan = format!(
            "# [mode_models] 按协作模式配置模型 — 执行计划\n\n## File Structure\n\n## Tasks\n\n{}",
            tasks_with(13)
        );
        assert!(split_threshold_gap(&plan, 3).is_some());
    }

    #[test]
    fn count_task_headings_accepts_common_variants() {
        let plan =
            "## Task 1\n\n### Task 2: wire the API\n\n#### task 3 — migrate\n\n### Task 12\n";
        assert_eq!(count_task_headings(plan), 4);
    }

    #[test]
    fn count_task_headings_ignores_non_task_headings() {
        // MUST-SURVIVE: prose-ish headings that merely contain "Task" must not
        // be counted, or ordinary single-file plans would trip the split check.
        let plan = "### Task Breakdown\n\n### Tasks\n\n### Task: overview\n\n### Random heading\n\n## Parts\n";
        assert_eq!(count_task_headings(plan), 0);
    }

    #[test]
    fn split_threshold_gap_message_names_expected_format() {
        let plan = (1..=9)
            .map(|n| format!("### Task {n}\n"))
            .collect::<String>();
        let gap = split_threshold_gap(&plan, 8).expect("9 tasks should exceed threshold 8");
        assert!(
            gap.contains("### Task N"),
            "error should name the expected heading format so the model can self-correct: {gap}"
        );
    }

    #[test]
    fn split_threshold_zero_disables_split_requirement() {
        let plan = (1..=13)
            .map(|n| format!("### Task {n}\n\nDo thing {n}.\n"))
            .collect::<String>();
        assert_eq!(count_task_headings(&plan), 13);
        assert!(
            split_threshold_gap(&plan, 0).is_none(),
            "split_threshold = 0 must disable the split requirement"
        );
    }

    #[test]
    fn count_task_headings_ignores_unrelated_headings_and_prose() {
        let plan = "# Plan\n\n## Tasks\n\nSee Task 1 above.\n\n### Task 1: real\n\n##### Task 5: not counted (too deep)\n\n### Task 2: real\n";
        assert_eq!(count_task_headings(plan), 2);
    }

    #[test]
    fn rigor_structure_gap_none_when_both_sections_complete() {
        let plan = format!(
            "# Plan\n\n## Spec coverage\n\n| Requirement | Task | Status |\n\n{}",
            self_review_with(7)
        );
        assert_eq!(rigor_structure_gap(&plan), None);
    }

    #[test]
    fn rigor_structure_gap_flags_missing_both_sections() {
        let plan = "# Plan\n\n## Risks\n\nsome risks\n\n## Assumptions\n\nsome assumptions\n";
        let gap = rigor_structure_gap(plan).expect("both sections missing");
        assert!(gap.contains("Spec coverage"));
        assert!(gap.contains("Self-review"));
    }

    #[test]
    fn rigor_structure_gap_flags_missing_self_review_only() {
        let plan = "# Plan\n\n## Spec coverage\n\n| Requirement | Task | Status |\n";
        let gap = rigor_structure_gap(plan).expect("self-review missing");
        assert!(gap.contains("Self-review"));
        assert!(!gap.contains("Spec coverage"));
    }

    #[test]
    fn rigor_structure_gap_flags_missing_spec_coverage_only() {
        let plan = format!("# Plan\n\n{}", self_review_with(7));
        let gap = rigor_structure_gap(&plan).expect("spec coverage missing");
        assert!(gap.contains("Spec coverage"));
    }

    #[test]
    fn rigor_structure_gap_flags_incomplete_checklist() {
        let plan = format!(
            "# Plan\n\n## Spec coverage\n\n| Requirement | Task | Status |\n\n{}",
            self_review_with(3)
        );
        let gap = rigor_structure_gap(&plan).expect("only 3 of 7 items present");
        assert!(
            gap.contains('3'),
            "gap message should mention the actual count: {gap}"
        );
    }

    #[test]
    fn rigor_structure_gap_ignores_content_after_next_heading() {
        // Checkbox items belonging to a later section (e.g. a caller's own
        // separate to-do list) must not be counted toward this plan's
        // self-review, which real plans reliably do NOT include.
        let plan = format!(
            "# Plan\n\n## Spec coverage\n\n| Requirement | Task | Status |\n\n{}\n## Unrelated\n\n{}",
            self_review_with(2),
            "- [ ] a\n- [ ] b\n- [ ] c\n- [ ] d\n- [ ] e\n"
        );
        let gap = rigor_structure_gap(&plan).expect("only 2 real self-review items");
        assert!(
            gap.contains('2'),
            "gap message should mention 2, not count past the next heading: {gap}"
        );
    }

    /// Regression test: this is the actual heading set from a real session's
    /// rejected single-file plan (see `.ody-code/plans/...mode_models...md`),
    /// which had `## Summary`, `## Key Changes`, `## Test Plan`, `## Assumptions`,
    /// `## Risks` but no `## Spec coverage` or `## Self-review` at all, despite
    /// the rigor-tier prompt mandating both.
    #[test]
    fn rigor_structure_gap_flags_real_world_missing_sections() {
        let plan = "# Plan\n\n## Summary\n\n- a\n\n## Key Changes\n\n- b\n\n## Test Plan\n\n- c\n\n## Assumptions\n\n- d\n\n## Risks\n\n| # | Risk | Mitigation |\n";
        let gap = rigor_structure_gap(plan).expect("real-world plan omitted both sections");
        assert!(gap.contains("Spec coverage"));
        assert!(gap.contains("Self-review"));
    }

    #[test]
    fn should_trigger_design_review_true_when_all_conditions_met() {
        use ody_protocol::config_types::ModeKind;
        assert!(should_trigger_design_review(
            ModeKind::Design,
            /*finalize*/ true,
            /*gap*/ None,
            /*has_pending_parts*/ false,
            Some("gpt-review"),
        ));
    }

    #[test]
    fn should_trigger_design_review_false_when_not_final() {
        use ody_protocol::config_types::ModeKind;
        assert!(!should_trigger_design_review(
            ModeKind::Design,
            /*finalize*/ false,
            /*gap*/ None,
            /*has_pending_parts*/ false,
            Some("gpt-review"),
        ));
    }

    #[test]
    fn should_trigger_design_review_false_when_gap_present() {
        use ody_protocol::config_types::ModeKind;
        assert!(!should_trigger_design_review(
            ModeKind::Design,
            /*finalize*/ true,
            /*gap*/ Some("missing section"),
            /*has_pending_parts*/ false,
            Some("gpt-review"),
        ));
    }

    #[test]
    fn should_trigger_design_review_false_when_review_model_missing() {
        use ody_protocol::config_types::ModeKind;
        assert!(!should_trigger_design_review(
            ModeKind::Design,
            /*finalize*/ true,
            /*gap*/ None,
            /*has_pending_parts*/ false,
            None,
        ));
    }

    #[test]
    fn should_trigger_design_review_false_when_not_design_mode() {
        use ody_protocol::config_types::ModeKind;
        assert!(!should_trigger_design_review(
            ModeKind::Plan,
            /*finalize*/ true,
            /*gap*/ None,
            /*has_pending_parts*/ false,
            Some("gpt-review"),
        ));
    }
}
