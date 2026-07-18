//! Post-design next-step prompt shown when Design mode finishes.
//!
//! Mirrors [`super::plan_implementation`]: after the design turn completes, the
//! TUI offers a selection popup. Unlike the old host-driven request_user_input
//! menu (which could only *advise* the user to run `/plan` because collaboration
//! mode is owned by the client), selecting "Enter Plan mode" here actually
//! switches the collaboration mode via `AppEvent::SubmitUserMessageWithMode`.

use std::path::Path;

use ody_protocol::config_types::CollaborationModeMask;

use crate::app_event::AppEvent;
use crate::bottom_pane::SelectionAction;
use crate::bottom_pane::SelectionItem;
use crate::bottom_pane::SelectionViewParams;
use crate::bottom_pane::popup_consts::standard_popup_hint_line;

pub(super) const DESIGN_NEXT_STEP_TITLE: &str = "Design ready — what next?";
pub(super) const DESIGN_NEXT_STEP_ENTER_PLAN: &str = "Enter Plan mode and write the plan";
pub(super) const DESIGN_NEXT_STEP_COMPACT_PLAN: &str =
    "Clear context and enter Plan mode";
pub(super) const DESIGN_NEXT_STEP_STAY: &str = "Stay in Design mode";
const PLAN_MODE_UNAVAILABLE: &str = "Plan mode unavailable";
const PLAN_HANDOFF_PROMPT: &str =
    "Write a step-by-step implementation plan from the approved design.";

/// Build the next-step prompt shown after a design finalizes in Design mode.
pub(super) fn selection_view_params(
    plan_mask: Option<CollaborationModeMask>,
    design_file_path: Option<&Path>,
) -> SelectionViewParams {
    let subtitle = design_file_path.map(|path| format!("Design file: {}", path.display()));

    // The handoff prompt names the design file so the plan turn can read it — the
    // design is always persisted to disk, so a cleared context can still recover
    // full intent from the file alone.
    let handoff_text = match design_file_path {
        Some(path) => format!("{PLAN_HANDOFF_PROMPT}\n\nDesign file: {}", path.display()),
        None => PLAN_HANDOFF_PROMPT.to_string(),
    };

    let (enter_actions, enter_disabled_reason) = match plan_mask.clone() {
        Some(mask) => {
            let text = handoff_text.clone();
            let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
                tx.send(AppEvent::SubmitUserMessageWithMode {
                    text: text.clone(),
                    collaboration_mode: mask.clone(),
                });
            })];
            (actions, None)
        }
        None => (Vec::new(), Some(PLAN_MODE_UNAVAILABLE.to_string())),
    };

    // Clear-context variant: mirrors the plan-mode "clear context and implement"
    // option, but hands off into Plan mode. Starts a fresh session so the plan
    // turn is not weighed down by the whole design conversation.
    let (compact_actions, compact_disabled_reason) = match plan_mask {
        Some(mask) => {
            let text = handoff_text.clone();
            let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
                tx.send(AppEvent::ClearUiAndSubmitUserMessageWithMode {
                    text: text.clone(),
                    collaboration_mode: mask.clone(),
                });
            })];
            (actions, None)
        }
        None => (Vec::new(), Some(PLAN_MODE_UNAVAILABLE.to_string())),
    };

    let items = vec![
        SelectionItem {
            name: DESIGN_NEXT_STEP_ENTER_PLAN.to_string(),
            description: Some(
                "Switch to Plan mode and start turning the design into an executable plan."
                    .to_string(),
            ),
            is_default: enter_disabled_reason.is_none(),
            actions: enter_actions,
            disabled_reason: enter_disabled_reason,
            dismiss_on_select: true,
            ..Default::default()
        },
        SelectionItem {
            name: DESIGN_NEXT_STEP_COMPACT_PLAN.to_string(),
            description: Some(
                "Start a fresh context, then enter Plan mode. The plan is written from the design file."
                    .to_string(),
            ),
            actions: compact_actions,
            disabled_reason: compact_disabled_reason,
            dismiss_on_select: true,
            ..Default::default()
        },
        SelectionItem {
            name: DESIGN_NEXT_STEP_STAY.to_string(),
            description: Some("Keep refining or discussing the design.".to_string()),
            dismiss_on_select: true,
            ..Default::default()
        },
    ];

    SelectionViewParams {
        title: Some(DESIGN_NEXT_STEP_TITLE.to_string()),
        subtitle,
        footer_hint: Some(standard_popup_hint_line()),
        items,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ody_protocol::config_types::ModeKind;

    fn plan_mask() -> CollaborationModeMask {
        CollaborationModeMask {
            name: "Plan".to_string(),
            mode: Some(ModeKind::Plan),
            model: None,
            reasoning_effort: None,
            developer_instructions: None,
            design_audit_level: None,
        }
    }

    #[test]
    fn enter_plan_is_actionable_when_mask_present() {
        let params = selection_view_params(Some(plan_mask()), None);
        let enter = &params.items[0];
        assert_eq!(enter.name, DESIGN_NEXT_STEP_ENTER_PLAN);
        assert!(enter.disabled_reason.is_none());
        assert_eq!(enter.actions.len(), 1, "Enter Plan must carry a switch action");
        assert!(enter.is_default);
    }

    #[test]
    fn enter_plan_disabled_when_no_plan_mode() {
        let params = selection_view_params(None, None);
        let enter = &params.items[0];
        assert_eq!(enter.disabled_reason.as_deref(), Some(PLAN_MODE_UNAVAILABLE));
        assert!(enter.actions.is_empty());
    }

    #[test]
    fn offers_three_options_with_compact_plan_in_the_middle() {
        let params = selection_view_params(Some(plan_mask()), Some(Path::new("/d/design.md")));
        assert_eq!(params.items.len(), 3);
        assert_eq!(params.items[0].name, DESIGN_NEXT_STEP_ENTER_PLAN);
        assert_eq!(params.items[1].name, DESIGN_NEXT_STEP_COMPACT_PLAN);
        assert_eq!(params.items[2].name, DESIGN_NEXT_STEP_STAY);
    }

    #[test]
    fn compact_plan_is_actionable_when_mask_present() {
        let params = selection_view_params(Some(plan_mask()), None);
        let compact = &params.items[1];
        assert!(compact.disabled_reason.is_none());
        assert_eq!(
            compact.actions.len(),
            1,
            "Clear-context Plan must carry a switch action"
        );
        // Only the first (keep-context) option is the default.
        assert!(!compact.is_default);
    }

    #[test]
    fn compact_plan_disabled_when_no_plan_mode() {
        let params = selection_view_params(None, None);
        let compact = &params.items[1];
        assert_eq!(
            compact.disabled_reason.as_deref(),
            Some(PLAN_MODE_UNAVAILABLE)
        );
        assert!(compact.actions.is_empty());
    }

    #[test]
    fn always_offers_stay_in_design() {
        let params = selection_view_params(Some(plan_mask()), None);
        assert_eq!(params.items.last().unwrap().name, DESIGN_NEXT_STEP_STAY);
    }

    #[test]
    fn subtitle_names_the_design_file() {
        let params = selection_view_params(Some(plan_mask()), Some(Path::new("/d/design.md")));
        assert_eq!(params.subtitle.as_deref(), Some("Design file: /d/design.md"));
    }
}
