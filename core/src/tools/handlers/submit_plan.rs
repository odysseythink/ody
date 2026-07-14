use crate::function_tool::FunctionCallError;
use crate::plan_artifact::PlanWriteOutcome;
use crate::plan_mode_injector::parts_manifest::RowStatus;
use crate::plan_mode_injector::parts_manifest::parse_parts_manifest;
use crate::plan_mode_injector::parts_manifest::row_is_verified_done;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::submit_plan_spec::SUBMIT_PLAN_TOOL_NAME;
use crate::tools::handlers::submit_plan_spec::create_submit_plan_tool;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use ody_config::config_toml::PlanModeTier;
use ody_protocol::config_types::ModeKind;
use ody_protocol::items::PlanItem;
use ody_protocol::items::TurnItem;
use ody_protocol::protocol::EventMsg;
use ody_protocol::protocol::PlanDeltaEvent;
use ody_protocol::protocol::WarningEvent;
use ody_protocol::submit_plan_tool::SubmitPlanArgs;
use ody_tools::ToolName;
use ody_tools::ToolSpec;
use std::path::PathBuf;

const PLAN_SUBMITTED_MESSAGE: &str = "Plan submitted";
const REQUIRED_SELF_REVIEW_ITEMS: usize = 7;

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

/// Counts `### Task N` headings — the convention every observed plan and
/// template example uses for the "Tasks" section. Used to mechanically detect
/// a plan that should have been split per the base template's "Large plan
/// splitting" rule but wasn't.
fn count_task_headings(plan: &str) -> usize {
    plan.lines()
        .filter(|line| {
            line.trim_start()
                .strip_prefix("### Task ")
                .and_then(|rest| rest.trim_start().chars().next())
                .is_some_and(|c| c.is_ascii_digit())
        })
        .count()
}

/// Returns a description of why a single-file (no `## Parts` table) plan
/// should have been split, or `None` if it is within the configured
/// `split_threshold`. Models reliably talk themselves into "this exceeds the
/// threshold, I should split" and then submit a single file anyway, so this
/// is checked mechanically rather than trusted.
fn split_threshold_gap(plan: &str, split_threshold: usize) -> Option<String> {
    let task_count = count_task_headings(plan);
    if task_count > split_threshold {
        Some(format!(
            "has {task_count} `### Task` headings but no `## Parts` table — the plan-mode instructions require splitting into multiple files once a plan exceeds {split_threshold} distinct tasks (see \"Large plan splitting\")"
        ))
    } else {
        None
    }
}

#[derive(Debug)]
pub struct SubmitPlanHandler;

impl ToolExecutor<ToolInvocation> for SubmitPlanHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(SUBMIT_PLAN_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_submit_plan_tool()
    }

    fn handle(&self, invocation: ToolInvocation) -> ody_tools::ToolExecutorFuture<'_> {
        Box::pin(self.handle_call(invocation))
    }
}

impl CoreToolRuntime for SubmitPlanHandler {}

impl SubmitPlanHandler {
    async fn handle_call(
        &self,
        invocation: ToolInvocation,
    ) -> Result<Box<dyn crate::tools::context::ToolOutput>, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            call_id,
            payload,
            ..
        } = invocation;

        let arguments = match payload {
            ToolPayload::Function { arguments } => arguments,
            _ => {
                return Err(FunctionCallError::RespondToModel(format!(
                    "{SUBMIT_PLAN_TOOL_NAME} handler received unsupported payload"
                )));
            }
        };

        if turn.collaboration_mode.mode != ModeKind::Plan {
            return Err(FunctionCallError::RespondToModel(
                "submit_plan is only available in Plan mode".to_string(),
            ));
        }

        let Some(artifact) = turn.plan_artifact.as_ref() else {
            return Err(FunctionCallError::RespondToModel(
                "submit_plan unavailable: no plan artifact".to_string(),
            ));
        };

        let args: SubmitPlanArgs = parse_arguments(&arguments)?;

        // Guard against silently discarding an in-progress split plan's *completed*
        // work. If a prior turn already verified one or more parts done on disk
        // (tracked via `last_manifest_snapshot`, updated by
        // `PlanModeInjector::after_plan_turn` after each turn) and this submission
        // has no `## Parts` table at all, accepting it as-is would overwrite the
        // index and silently lose those done parts. Reject so the model resubmits
        // the full index instead.
        //
        // If nothing has been verified done yet (only pending rows so far), there is
        // nothing to lose — allow the drop. This matters because a model that can
        // never write part files in Plan mode (no working file-write tool, or one
        // stuck on the wrong path) has no other way to escape a split plan; without
        // this exception every subsequent single-file resubmission would be rejected
        // forever, even after the user explicitly agrees to abandon the split.
        let previously_had_done_parts = artifact
            .last_manifest_snapshot()
            .is_some_and(|snapshot| snapshot.done_count > 0);
        if previously_had_done_parts && parse_parts_manifest(&args.plan).manifest.is_none() {
            return Err(FunctionCallError::RespondToModel(
                "submit_plan rejected: the previous turn's plan had a `## Parts` table with pending rows, but this submission has no `## Parts` table at all. This call was not persisted. Resubmit the full index markdown (Goal/Architecture/File Structure/etc. plus the `## Parts` table with every row's current status) — submit_plan always writes the single index file, never an individual part's content.".to_string(),
            ));
        }

        let item_id = format!("{}-plan", turn.sub_id);
        let plan_file_path = artifact.path().map(PathBuf::from);

        session
            .emit_turn_item_started(
                turn.as_ref(),
                &TurnItem::Plan(PlanItem {
                    id: item_id.clone(),
                    text: String::new(),
                    plan_file_path: plan_file_path.clone(),
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
                    delta: args.plan.clone(),
                }),
            )
            .await;

        let persist = turn
            .config
            .plan_mode
            .as_ref()
            .and_then(|pm| pm.persist_plan_file)
            .unwrap_or(true);
        let outcome = artifact.write_plan(&args.plan, persist).await;

        if let PlanWriteOutcome::Failed { error } = &outcome {
            session
                .send_event(
                    turn.as_ref(),
                    EventMsg::Warning(WarningEvent {
                        message: format!("Failed to persist plan: {error}"),
                    }),
                )
                .await;
        }

        // A split plan (see `## Parts` table, `plan_mode_injector`) writes its index and
        // each part through this same tool. Only the call that leaves no pending rows
        // is the real terminal submission — an index/part call with pending rows must
        // not end the Plan-mode turn, or the remaining parts are never written (the
        // model gets stranded outside Plan mode after the first `submit_plan` call).
        //
        // A row can only count as done if its part file actually exists on disk —
        // otherwise a model that flips a row to `done` without ever writing the file
        // (e.g. because it had no working file-write tool) would silently end Plan
        // mode with the part never persisted.
        let has_pending_parts = match artifact.stem_dir() {
            Some(stem_dir) => parse_parts_manifest(&args.plan)
                .manifest
                .is_some_and(|manifest| {
                    manifest
                        .rows
                        .iter()
                        .any(|row| !row_is_verified_done(&stem_dir, row))
                }),
            None => parse_parts_manifest(&args.plan)
                .manifest
                .is_some_and(|manifest| {
                    manifest
                        .rows
                        .iter()
                        .any(|row| row.status == RowStatus::Pending)
                }),
        };

        // Rigor-tier plans are instructed (via the injected rigor-tier prompt
        // fragments) to include a `## Spec coverage` table and a `## Self-review`
        // section with all seven items, but models frequently reference these
        // requirements without actually satisfying them. Only check on the call
        // that would otherwise be terminal — an in-progress split plan's index/part
        // calls are not the final artifact yet.
        //
        // Likewise, the base template's "Large plan splitting" rule is advisory
        // text the model is prone to reasoning its way past ("this clearly exceeds
        // the threshold... I should split" — then submitting a single file anyway).
        // Check the task count mechanically whenever this submission has no `## Parts`
        // table of its own; that check takes priority since "should this even be one
        // file?" is more fundamental than "is this one file complete?".
        let rigor_gap = if has_pending_parts {
            None
        } else if parse_parts_manifest(&args.plan).manifest.is_none() {
            let split_threshold = turn
                .config
                .plan_mode
                .as_ref()
                .and_then(|pm| pm.split_threshold)
                .unwrap_or(8);
            split_threshold_gap(&args.plan, split_threshold).or_else(|| {
                if artifact.plan_mode_tier() == Some(PlanModeTier::Rigor) {
                    rigor_structure_gap(&args.plan)
                } else {
                    None
                }
            })
        } else if artifact.plan_mode_tier() == Some(PlanModeTier::Rigor) {
            rigor_structure_gap(&args.plan)
        } else {
            None
        };

        session
            .emit_turn_item_completed(
                turn.as_ref(),
                TurnItem::Plan(PlanItem {
                    id: item_id,
                    text: args.plan,
                    plan_file_path,
                }),
            )
            .await;

        let message = if has_pending_parts {
            // The model must know the exact resolved stem directory to write part
            // files at `<stem>/<part>.md` — the plan's on-disk filename is
            // auto-derived from its title (see `PlanArtifact::finalize_name`) and
            // the model cannot reliably reconstruct it on its own. Without this,
            // a guessed part-file path can miss the plan-mode write whitelist
            // entirely and the model is left with no way to persist the part.
            match artifact.stem_dir() {
                Some(stem_dir) => format!(
                    "Plan part saved. This plan is split into multiple parts and pending parts remain — stay in Plan mode and keep writing them one per turn; do not treat this call as final. Write each part file at exactly {}/<part-name>.md (use the file names from the ## Parts table).",
                    stem_dir.display()
                ),
                None => "Plan part saved. This plan is split into multiple parts and pending parts remain — stay in Plan mode and keep writing them one per turn; do not treat this call as final.".to_string(),
            }
        } else if let Some(gap) = rigor_gap {
            format!(
                "Plan part saved, but this is a rigor-tier plan and it is {gap}. This call was persisted but is NOT treated as final — stay in Plan mode, add the missing section(s) per the rigor-tier addendum instructions, and call submit_plan again with the complete plan."
            )
        } else {
            artifact.mark_submitted();
            PLAN_SUBMITTED_MESSAGE.to_string()
        };

        Ok(boxed_tool_output(FunctionToolOutput::from_text(
            message,
            Some(true),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Regression test: the real session that prompted this check had exactly
    /// this task-heading shape (13 tasks spanning config/core/app-server/tui,
    /// no `## Parts` table) with `split_threshold = 3` configured, and the
    /// model reasoned "this clearly exceeds the threshold, I should split"
    /// before submitting a single file anyway.
    #[test]
    fn split_threshold_gap_flags_real_world_13_task_single_file_plan() {
        let plan = format!(
            "# [mode_models] 按协作模式配置模型 — 执行计划\n\n## File Structure\n\n## Tasks\n\n{}",
            tasks_with(13)
        );
        assert!(split_threshold_gap(&plan, 3).is_some());
    }

    #[test]
    fn count_task_headings_ignores_unrelated_headings_and_prose() {
        let plan = "# Plan\n\n## Tasks\n\nSee Task 1 above.\n\n### Task 1: real\n\n#### Task 1a: not counted (wrong heading level)\n\n### Task 2: real\n";
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
