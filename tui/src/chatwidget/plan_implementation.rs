use std::path::Path;

use ody_protocol::config_types::CollaborationModeMask;

use crate::app_event::AppEvent;
use crate::bottom_pane::SelectionAction;
use crate::bottom_pane::SelectionItem;
use crate::bottom_pane::SelectionViewParams;
use crate::bottom_pane::popup_consts::standard_popup_hint_line;

pub(super) const PLAN_IMPLEMENTATION_TITLE: &str = "Implement this plan?";
const PLAN_IMPLEMENTATION_YES: &str = "Yes, implement this plan";
const PLAN_IMPLEMENTATION_CLEAR_CONTEXT: &str = "Yes, clear context and implement";
const PLAN_IMPLEMENTATION_NO: &str = "No, stay in Plan mode";
pub(super) const PLAN_IMPLEMENTATION_CODING_MESSAGE: &str = "Implement the plan.";
pub(super) const PLAN_IMPLEMENTATION_CLEAR_CONTEXT_PREFIX: &str = concat!(
    "A previous agent produced the plan below to accomplish the user's task. ",
    "Implement the plan in a fresh context. Treat the plan as the source of ",
    "user intent, re-read files as needed, and carry the work through ",
    "implementation and verification."
);
pub(super) const PLAN_IMPLEMENTATION_DEFAULT_UNAVAILABLE: &str = "Default mode unavailable";
pub(super) const PLAN_IMPLEMENTATION_NO_APPROVED_PLAN: &str = "No approved plan available";
pub(super) const PLAN_IMPLEMENTATION_PLAN_FILE_READ_FAILED: &str =
    "Could not read plan file";

/// Builds the confirmation prompt shown after a plan is approved in Plan mode.
///
/// The optional usage label is already phrased for display, such as `89% used`
/// or `123K used`. This module only decides where that label belongs in the
/// decision copy so action wiring stays separate from token accounting.
pub(super) fn selection_view_params(
    default_mask: Option<CollaborationModeMask>,
    plan_markdown: Option<&str>,
    clear_context_usage_label: Option<&str>,
    plan_file_path: Option<&Path>,
) -> SelectionViewParams {
    // When the plan was persisted to disk, reload it at approval time so the
    // handoff payload is exactly what is on disk rather than a memory snapshot.
    let (loaded_plan_markdown, disk_read_failed) = match plan_file_path {
        Some(path) => match std::fs::read_to_string(path) {
            Ok(content) if !content.trim().is_empty() => (Some(content), false),
            Ok(_) => (None, true),
            Err(err) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %err,
                    "failed to read plan file for implementation prompt"
                );
                (None, true)
            }
        },
        None => (plan_markdown.map(|s| s.to_string()), false),
    };

    let subtitle = plan_file_path.map(|path| format!("Plan file: {}", path.display()));

    let (implement_actions, implement_disabled_reason) = match default_mask.clone() {
        Some(mask) => {
            let user_text = PLAN_IMPLEMENTATION_CODING_MESSAGE.to_string();
            let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
                tx.send(AppEvent::SubmitUserMessageWithMode {
                    text: user_text.clone(),
                    collaboration_mode: mask.clone(),
                });
            })];
            (actions, None)
        }
        None => (
            Vec::new(),
            Some(PLAN_IMPLEMENTATION_DEFAULT_UNAVAILABLE.to_string()),
        ),
    };

    let no_plan_reason = if disk_read_failed {
        PLAN_IMPLEMENTATION_PLAN_FILE_READ_FAILED
    } else {
        PLAN_IMPLEMENTATION_NO_APPROVED_PLAN
    };

    let (clear_context_actions, clear_context_disabled_reason) =
        match (default_mask, loaded_plan_markdown.as_deref()) {
            (None, _) => (
                Vec::new(),
                Some(PLAN_IMPLEMENTATION_DEFAULT_UNAVAILABLE.to_string()),
            ),
            (Some(_), Some(plan_markdown)) if !plan_markdown.trim().is_empty() => {
                let user_text =
                    format!("{PLAN_IMPLEMENTATION_CLEAR_CONTEXT_PREFIX}\n\n{plan_markdown}");
                let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
                    tx.send(AppEvent::ClearUiAndSubmitUserMessage {
                        text: user_text.clone(),
                    });
                })];
                (actions, None)
            }
            (Some(_), _) => (
                Vec::new(),
                Some(no_plan_reason.to_string()),
            ),
        };

    let clear_context_description = clear_context_usage_label.map_or_else(
        || "Fresh thread with this plan.".to_string(),
        |label| format!("Fresh thread. Context: {label}."),
    );

    SelectionViewParams {
        title: Some(PLAN_IMPLEMENTATION_TITLE.to_string()),
        subtitle,
        footer_hint: Some(standard_popup_hint_line()),
        items: vec![
            SelectionItem {
                name: PLAN_IMPLEMENTATION_YES.to_string(),
                description: Some("Switch to Default and start coding.".to_string()),
                selected_description: None,
                is_current: false,
                actions: implement_actions,
                disabled_reason: implement_disabled_reason,
                dismiss_on_select: true,
                ..Default::default()
            },
            SelectionItem {
                name: PLAN_IMPLEMENTATION_CLEAR_CONTEXT.to_string(),
                description: Some(clear_context_description),
                selected_description: None,
                is_current: false,
                actions: clear_context_actions,
                disabled_reason: clear_context_disabled_reason,
                dismiss_on_select: true,
                ..Default::default()
            },
            SelectionItem {
                name: PLAN_IMPLEMENTATION_NO.to_string(),
                description: Some("Continue planning with the model.".to_string()),
                selected_description: None,
                is_current: false,
                actions: Vec::new(),
                dismiss_on_select: true,
                ..Default::default()
            },
        ],
        ..Default::default()
    }
}
