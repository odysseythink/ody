//! Shared submit core for Plan and Design mode artifact persistence.
//!
//! Extracted from `submit_plan.rs`; both `SubmitPlanHandler` and
//! `SubmitDesignHandler` are thin shells that call `handle_submit_artifact`.

use crate::design_completeness::design_completeness_report;
use crate::function_tool::FunctionCallError;
use crate::plan_artifact::PlanWriteOutcome;
use crate::plan_mode_injector::parts_manifest::RowStatus;
use crate::plan_mode_injector::parts_manifest::parse_parts_manifest;
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
fn rigor_structure_gap(plan: &str) -> Option<String> {
    let lower = plan.to_lowercase();
    let has_spec_coverage = lower.contains("## spec coverage") || lower.contains("## spec-coverage");
    let self_review_heading = lower.find("## self-review").or_else(|| lower.find("## self review"));

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
            let checklist_items = section[..section_end]
                .matches("- [ ]")
                .count()
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
    let item_id = format!("{}-{}", turn.sub_id, wording.noun);
    let artifact_path = artifact.path().map(PathBuf::from);

    session
        .emit_turn_item_started(
            turn.as_ref(),
            &TurnItem::Plan(PlanItem {
                id: item_id.clone(),
                text: String::new(),
                plan_file_path: artifact_path.clone(),
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

    // 8. Emit completed event
    session
        .emit_turn_item_completed(
            turn.as_ref(),
            TurnItem::Plan(PlanItem {
                id: item_id,
                text: markdown,
                plan_file_path: artifact_path,
            }),
        )
        .await;

    // 9. Build response message
    let message = if has_pending_parts {
        match artifact.stem_dir() {
            Some(stem_dir) => format!(
                "{} part saved. This {} is split into multiple parts and pending parts remain — stay in {} mode and keep writing them one per turn; do not treat this call as final. Write each part file at exactly {}/<part-name>.md (use the file names from the ## Parts table).",
                wording.noun, wording.noun, wording.mode_name, stem_dir.display()
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

    Ok(boxed_tool_output(FunctionToolOutput::from_text(
        message,
        Some(true),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(gap.contains("13"), "gap should mention the actual task count: {gap}");
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
        let plan =
            "### Task Breakdown\n\n### Tasks\n\n### Task: overview\n\n### Random heading\n\n## Parts\n";
        assert_eq!(count_task_headings(plan), 0);
    }

    #[test]
    fn split_threshold_gap_message_names_expected_format() {
        let plan = (1..=9).map(|n| format!("### Task {n}\n")).collect::<String>();
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
        assert!(gap.contains('3'), "gap message should mention the actual count: {gap}");
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
        assert!(gap.contains('2'), "gap message should mention 2, not count past the next heading: {gap}");
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
}
