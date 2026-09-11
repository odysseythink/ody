//! Post-product next-step prompt shown when Product mode finalizes a
//! requirements document (`submit_product`).
//!
//! Mirrors [`super::design_next_step`]: after the requirements turn completes,
//! the TUI offers a selection popup. Collaboration mode is owned by the client,
//! so selecting "Enter Plan mode" / "Enter Design mode" here actually switches
//! the collaboration mode via `AppEvent::SubmitUserMessageWithMode`; the host
//! only finalizes the document and emits the finalized plan item.

use std::path::Path;

use ody_protocol::config_types::CollaborationModeMask;

use crate::app_event::AppEvent;
use crate::bottom_pane::SelectionAction;
use crate::bottom_pane::SelectionItem;
use crate::bottom_pane::SelectionViewParams;
use crate::bottom_pane::popup_consts::standard_popup_hint_line;

pub(super) const PRODUCT_NEXT_STEP_TITLE: &str = "Requirements ready — what next?";
pub(super) const PRODUCT_NEXT_STEP_ENTER_PLAN: &str = "Enter Plan mode and plan the implementation";
pub(super) const PRODUCT_NEXT_STEP_ENTER_DESIGN: &str = "Enter Design mode and design the solution";
pub(super) const PRODUCT_NEXT_STEP_STAY: &str = "Stay in Product mode";
const MODE_UNAVAILABLE: &str = "mode unavailable in this model catalog";
const PLAN_HANDOFF_PROMPT: &str =
    "Write a step-by-step implementation plan from the approved requirements document.";
const DESIGN_HANDOFF_PROMPT: &str =
    "Design the solution from the approved requirements document.";

fn handoff_text(prompt: &str, requirements_file_path: Option<&Path>) -> String {
    match requirements_file_path {
        Some(path) => format!("{prompt}\n\nRequirements file: {}", path.display()),
        None => prompt.to_string(),
    }
}

fn switch_actions(
    mask: Option<CollaborationModeMask>,
    text: String,
) -> (Vec<SelectionAction>, Option<String>) {
    match mask {
        Some(mask) => {
            let text_clone = text.clone();
            let mask_clone = mask.clone();
            let keep: Vec<SelectionAction> = vec![Box::new(move |tx| {
                tx.send(AppEvent::SubmitUserMessageWithMode {
                    text: text_clone.clone(),
                    collaboration_mode: mask_clone.clone(),
                });
            })];
            let clear: Vec<SelectionAction> = vec![Box::new(move |tx| {
                tx.send(AppEvent::ClearUiAndSubmitUserMessageWithMode {
                    text: text.clone(),
                    collaboration_mode: mask.clone(),
                });
            })];
            let mut actions = keep;
            actions.extend(clear);
            (actions, None)
        }
        None => (Vec::new(), Some(MODE_UNAVAILABLE.to_string())),
    }
}

/// Build the next-step prompt shown after `submit_product` finalizes the
/// requirements document in Product mode.
pub(super) fn selection_view_params(
    plan_mask: Option<CollaborationModeMask>,
    design_mask: Option<CollaborationModeMask>,
    requirements_file_path: Option<&Path>,
) -> SelectionViewParams {
    let subtitle = requirements_file_path
        .map(|path| format!("Requirements file: {}", path.display()));

    // The handoff prompt names the requirements file so the next mode's turn
    // can read it — the document is always persisted to disk, so a cleared
    // context can still recover full intent from the file alone.
    let (plan_actions, plan_disabled) = switch_actions(
        plan_mask,
        handoff_text(PLAN_HANDOFF_PROMPT, requirements_file_path),
    );
    let (design_actions, design_disabled) = switch_actions(
        design_mask,
        handoff_text(DESIGN_HANDOFF_PROMPT, requirements_file_path),
    );
    let plan_is_default = plan_disabled.is_none();
    let design_is_default = !plan_is_default && design_disabled.is_none();

    let items = vec![
        SelectionItem {
            name: PRODUCT_NEXT_STEP_ENTER_PLAN.to_string(),
            description: Some(
                "Switch to Plan mode and start turning the requirements into an executable plan."
                    .to_string(),
            ),
            is_default: plan_is_default,
            actions: plan_actions,
            disabled_reason: plan_disabled,
            dismiss_on_select: true,
            ..Default::default()
        },
        SelectionItem {
            name: PRODUCT_NEXT_STEP_ENTER_DESIGN.to_string(),
            description: Some(
                "Switch to Design mode and design the solution from the requirements."
                    .to_string(),
            ),
            is_default: design_is_default,
            actions: design_actions,
            disabled_reason: design_disabled,
            dismiss_on_select: true,
            ..Default::default()
        },
        SelectionItem {
            name: PRODUCT_NEXT_STEP_STAY.to_string(),
            description: Some("Keep refining or discussing the requirements.".to_string()),
            dismiss_on_select: true,
            ..Default::default()
        },
    ];

    SelectionViewParams {
        title: Some(PRODUCT_NEXT_STEP_TITLE.to_string()),
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

    fn mask(name: &str, mode: ModeKind) -> CollaborationModeMask {
        CollaborationModeMask {
            name: name.to_string(),
            mode: Some(mode),
            model: None,
            reasoning_effort: None,
            developer_instructions: None,
            design_audit_level: None,
        }
    }

    #[test]
    fn offers_plan_design_and_stay_options() {
        let params = selection_view_params(
            Some(mask("Plan", ModeKind::Plan)),
            Some(mask("Design", ModeKind::Design)),
            Some(Path::new("/d/requirements.md")),
        );
        assert_eq!(params.items.len(), 3);
        assert_eq!(params.items[0].name, PRODUCT_NEXT_STEP_ENTER_PLAN);
        assert_eq!(params.items[1].name, PRODUCT_NEXT_STEP_ENTER_DESIGN);
        assert_eq!(params.items[2].name, PRODUCT_NEXT_STEP_STAY);
        assert_eq!(
            params.subtitle.as_deref(),
            Some("Requirements file: /d/requirements.md")
        );
    }

    #[test]
    fn enter_plan_is_default_and_actionable_when_mask_present() {
        let params = selection_view_params(
            Some(mask("Plan", ModeKind::Plan)),
            Some(mask("Design", ModeKind::Design)),
            None,
        );
        let enter = &params.items[0];
        assert!(enter.is_default);
        assert!(enter.disabled_reason.is_none());
        assert_eq!(
            enter.actions.len(),
            2,
            "Enter Plan must carry keep-context and clear-context switch actions"
        );
    }

    #[test]
    fn enter_design_disabled_without_mask() {
        let params = selection_view_params(None, None, None);
        assert_eq!(
            params.items[0].disabled_reason.as_deref(),
            Some(MODE_UNAVAILABLE)
        );
        assert_eq!(
            params.items[1].disabled_reason.as_deref(),
            Some(MODE_UNAVAILABLE)
        );
        assert!(params.items[0].actions.is_empty());
        assert!(params.items[1].actions.is_empty());
        // Stay is always available and nothing is default when both are disabled.
        assert!(!params.items[0].is_default);
        assert!(!params.items[1].is_default);
    }

    #[test]
    fn design_becomes_default_when_plan_unavailable() {
        let params = selection_view_params(
            None,
            Some(mask("Design", ModeKind::Design)),
            None,
        );
        assert!(!params.items[0].is_default);
        assert!(params.items[1].is_default);
    }

    #[test]
    fn handoff_text_names_the_requirements_file() {
        let text = handoff_text(
            PLAN_HANDOFF_PROMPT,
            Some(Path::new("/d/requirements.md")),
        );
        assert!(text.contains(PLAN_HANDOFF_PROMPT));
        assert!(text.contains("Requirements file: /d/requirements.md"));
    }
}
