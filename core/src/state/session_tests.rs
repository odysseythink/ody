use super::*;
use crate::session::tests::make_session_configuration_for_tests;
use crate::state::AutoCompactWindowSnapshot;
use ody_protocol::plan_tool::PlanItemArg;
use ody_protocol::plan_tool::StepStatus;
use pretty_assertions::assert_eq;

fn plan_item(step: &str, status: StepStatus) -> PlanItemArg {
    PlanItemArg {
        step: step.to_string(),
        status,
    }
}

fn unfinished_plan() -> Vec<PlanItemArg> {
    vec![
        plan_item("step 1", StepStatus::InProgress),
        plan_item("step 2", StepStatus::Pending),
    ]
}

// PlanItemArg / StepStatus have no PartialEq, so compare on a projected shape.
fn steps_of(plan: &[PlanItemArg]) -> Vec<(String, &'static str)> {
    plan.iter()
        .map(|item| {
            let status = match item.status {
                StepStatus::Pending => "pending",
                StepStatus::InProgress => "in_progress",
                StepStatus::Completed => "completed",
            };
            (item.step.clone(), status)
        })
        .collect()
}

#[tokio::test]
// Verifies connector merging deduplicates repeated IDs.
async fn merge_connector_selection_deduplicates_entries() {
    let session_configuration = make_session_configuration_for_tests().await;
    let mut state = SessionState::new(session_configuration);
    let merged = state.merge_connector_selection([
        "calendar".to_string(),
        "calendar".to_string(),
        "drive".to_string(),
    ]);

    assert_eq!(
        merged,
        HashSet::from(["calendar".to_string(), "drive".to_string()])
    );
}

#[tokio::test]
// Verifies clearing connector selection removes all saved IDs.
async fn clear_connector_selection_removes_entries() {
    let session_configuration = make_session_configuration_for_tests().await;
    let mut state = SessionState::new(session_configuration);
    state.merge_connector_selection(["calendar".to_string()]);

    state.clear_connector_selection();

    assert_eq!(state.get_connector_selection(), HashSet::new());
}

#[tokio::test]
async fn replace_history_clears_auto_compact_window_prefill() {
    let session_configuration = make_session_configuration_for_tests().await;
    let mut state = SessionState::new(session_configuration);

    state.set_auto_compact_window_estimated_prefill(/*tokens*/ 100);
    state.replace_history(Vec::new(), /*reference_context_item*/ None);

    assert_eq!(
        state.auto_compact_window_snapshot(),
        AutoCompactWindowSnapshot {
            prefill_input_tokens: None,
        }
    );
}

#[tokio::test]
// An unfinished plan left untouched fires exactly on the threshold tick, not before.
async fn plan_staleness_reminder_fires_on_threshold() {
    let session_configuration = make_session_configuration_for_tests().await;
    let mut state = SessionState::new(session_configuration);
    state.set_active_plan(unfinished_plan());

    // set_active_plan resets the clock, so the first threshold-1 ticks stay quiet.
    for _ in 0..2 {
        assert!(state.tick_plan_staleness_reminder(3).is_none());
    }
    // The threshold-th tick surfaces the current checklist.
    let fired = state.tick_plan_staleness_reminder(3).expect("reminder due");
    assert_eq!(steps_of(&fired), steps_of(&unfinished_plan()));
}

#[tokio::test]
// A completed plan is never nudged; there is nothing left to advance.
async fn plan_staleness_reminder_skips_completed_plan() {
    let session_configuration = make_session_configuration_for_tests().await;
    let mut state = SessionState::new(session_configuration);
    state.set_active_plan(vec![
        plan_item("step 1", StepStatus::Completed),
        plan_item("step 2", StepStatus::Completed),
    ]);

    for _ in 0..10 {
        assert!(state.tick_plan_staleness_reminder(3).is_none());
    }
}

#[tokio::test]
// No active plan means no reminder regardless of how many turns elapse.
async fn plan_staleness_reminder_requires_active_plan() {
    let session_configuration = make_session_configuration_for_tests().await;
    let mut state = SessionState::new(session_configuration);

    for _ in 0..10 {
        assert!(state.tick_plan_staleness_reminder(3).is_none());
    }
}

#[tokio::test]
// After firing, a fresh update_plan write resets the clock and re-arms the cadence.
async fn plan_staleness_reminder_resets_on_write() {
    let session_configuration = make_session_configuration_for_tests().await;
    let mut state = SessionState::new(session_configuration);
    state.set_active_plan(unfinished_plan());

    for _ in 0..2 {
        assert!(state.tick_plan_staleness_reminder(3).is_none());
    }
    assert!(state.tick_plan_staleness_reminder(3).is_some());

    // A new write resets both counters, so the next reminder waits a full interval.
    state.set_active_plan(unfinished_plan());
    for _ in 0..2 {
        assert!(state.tick_plan_staleness_reminder(3).is_none());
    }
    assert!(state.tick_plan_staleness_reminder(3).is_some());
}

#[tokio::test]
// Once fired, the reminder stays quiet for a full interval before firing again.
async fn plan_staleness_reminder_throttles_between_reminders() {
    let session_configuration = make_session_configuration_for_tests().await;
    let mut state = SessionState::new(session_configuration);
    state.set_active_plan(unfinished_plan());

    // First reminder on the 3rd tick.
    for _ in 0..2 {
        assert!(state.tick_plan_staleness_reminder(3).is_none());
    }
    assert!(state.tick_plan_staleness_reminder(3).is_some());

    // No re-fire until another full interval elapses even though the plan is
    // still stale (never re-written).
    for _ in 0..2 {
        assert!(state.tick_plan_staleness_reminder(3).is_none());
    }
    assert!(state.tick_plan_staleness_reminder(3).is_some());
}

#[tokio::test]
async fn usability_decision_records_reads_and_resets_across_designs() {
    let session_configuration = make_session_configuration_for_tests().await;
    let mut state = SessionState::new(session_configuration);

    // Undecided for a fresh design.
    assert_eq!(state.design_usability_decision_for("design-a"), None);

    // Record and read back within the same design.
    state.record_design_usability_decision("design-a".to_string(), true);
    assert_eq!(state.design_usability_decision_for("design-a"), Some(true));

    // A different design has no inherited decision (stale state dropped).
    assert_eq!(state.design_usability_decision_for("design-b"), None);

    // Finalizing (clear) resets the cached decision.
    state.record_design_usability_decision("design-b".to_string(), false);
    assert_eq!(state.design_usability_decision_for("design-b"), Some(false));
    state.clear_design_signoff_seen();
    assert_eq!(state.design_usability_decision_for("design-b"), None);
}
