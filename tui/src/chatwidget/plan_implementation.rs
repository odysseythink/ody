use std::path::Path;

use crossterm::event::KeyCode;
use ody_protocol::config_types::CollaborationModeMask;

use crate::app_event::AppEvent;
use crate::key_hint;
use crate::bottom_pane::SelectionAction;
use crate::bottom_pane::SelectionItem;
use crate::bottom_pane::SelectionViewParams;
use crate::bottom_pane::custom_prompt_view::CustomPromptView;
use crate::bottom_pane::popup_consts::standard_popup_hint_line;
use crate::chatwidget::ChatWidget;
use crate::chatwidget::plan_options::PlanApprovalChoice;
use crate::chatwidget::plan_options::parse_plan_options;
use crate::chatwidget::plan_options::plan_choice_handoff_suffix;

pub(super) const PLAN_IMPLEMENTATION_TITLE: &str = "Implement this plan?";
pub(super) const PLAN_IMPLEMENTATION_YES: &str = "Yes, implement this plan";
pub(super) const PLAN_IMPLEMENTATION_CLEAR_CONTEXT: &str = "Yes, clear context and implement";
pub(super) const PLAN_IMPLEMENTATION_NO: &str = "No, stay in Plan mode";
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
    current_plan_mask: Option<CollaborationModeMask>,
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

    let options = parse_plan_options(loaded_plan_markdown.as_deref().unwrap_or(""));
    let has_options = !options.is_empty();

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
        match (default_mask.clone(), loaded_plan_markdown.as_deref()) {
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

    let mut items: Vec<SelectionItem> = Vec::new();

    if has_options {
        for opt in options {
            let label = opt.label;
            let summary = opt.summary;

            // 1) Approve Option X (keep current context)
            let approve_text = {
                let suffix = plan_choice_handoff_suffix(&PlanApprovalChoice::ApproveOption {
                    label,
                    summary: summary.clone(),
                    clear_context: false,
                });
                match suffix {
                    Some(s) => format!("{PLAN_IMPLEMENTATION_CODING_MESSAGE}\n\n{s}"),
                    None => PLAN_IMPLEMENTATION_CODING_MESSAGE.to_string(),
                }
            };
            let approve_actions: Vec<SelectionAction> = match default_mask.clone() {
                Some(mask) => vec![Box::new(move |tx| {
                    tx.send(AppEvent::SubmitUserMessageWithMode {
                        text: approve_text.clone(),
                        collaboration_mode: mask.clone(),
                    });
                })],
                None => Vec::new(),
            };
            let shortcut = opt.label.to_ascii_lowercase();
            items.push(SelectionItem {
                name: format!("Approve Option {label}"),
                description: Some(if summary.is_empty() {
                    "Implement this option.".to_string()
                } else {
                    summary.clone()
                }),
                selected_description: None,
                is_current: false,
                display_shortcut: Some(key_hint::plain(KeyCode::Char(shortcut))),
                actions: approve_actions,
                disabled_reason: implement_disabled_reason.clone(),
                dismiss_on_select: true,
                ..Default::default()
            });

            // 2) Approve Option X (fresh context)
            let fresh_text = match loaded_plan_markdown.as_deref() {
                Some(plan) if !plan.trim().is_empty() => {
                    let suffix = plan_choice_handoff_suffix(&PlanApprovalChoice::ApproveOption {
                        label,
                        summary: summary.clone(),
                        clear_context: true,
                    });
                    let suffix = suffix.unwrap_or_default();
                    format!("{PLAN_IMPLEMENTATION_CLEAR_CONTEXT_PREFIX}\n\n{plan}\n\n{suffix}")
                }
                _ => String::new(),
            };
            let (fresh_actions, fresh_disabled_reason): (Vec<SelectionAction>, Option<String>) =
                match (default_mask.clone(), loaded_plan_markdown.as_deref()) {
                    (Some(_), Some(plan)) if !plan.trim().is_empty() => {
                        let text = fresh_text.clone();
                        (
                            vec![Box::new(move |tx| {
                                tx.send(AppEvent::ClearUiAndSubmitUserMessage {
                                    text: text.clone(),
                                });
                            })],
                            None,
                        )
                    }
                    (None, _) => (Vec::new(), Some(PLAN_IMPLEMENTATION_DEFAULT_UNAVAILABLE.to_string())),
                    _ => (Vec::new(), Some(no_plan_reason.to_string())),
                };
            items.push(SelectionItem {
                name: format!("Approve Option {label} (fresh context)"),
                description: Some(if summary.is_empty() {
                    "Fresh thread with this option.".to_string()
                } else {
                    format!("Fresh thread. {summary}")
                }),
                selected_description: None,
                is_current: false,
                actions: fresh_actions,
                disabled_reason: fresh_disabled_reason,
                dismiss_on_select: true,
                ..Default::default()
            });
        }

        // 3) Revise plan
        let revise_actions: Vec<SelectionAction> = match current_plan_mask.clone() {
            Some(mask) => vec![Box::new(move |tx| {
                tx.send(AppEvent::OpenPlanRevisionPrompt {
                    collaboration_mode: mask.clone(),
                });
            })],
            None => Vec::new(),
        };
        items.push(SelectionItem {
            name: "Revise plan".to_string(),
            description: Some("Provide feedback and continue planning.".to_string()),
            selected_description: None,
            is_current: false,
            actions: revise_actions,
            disabled_reason: current_plan_mask
                .clone()
                .map(|_| None)
                .unwrap_or_else(|| Some(PLAN_IMPLEMENTATION_DEFAULT_UNAVAILABLE.to_string())),
            dismiss_on_select: true,
            ..Default::default()
        });

        // 4) Reject plan
        let reject_actions: Vec<SelectionAction> = match default_mask.clone() {
            Some(mask) => vec![Box::new(move |tx| {
                tx.send(AppEvent::SetCollaborationMask(mask.clone()));
            })],
            None => Vec::new(),
        };
        items.push(SelectionItem {
            name: "Reject plan".to_string(),
            description: Some("Exit Plan mode without implementing.".to_string()),
            selected_description: None,
            is_current: false,
            actions: reject_actions,
            disabled_reason: default_mask
                .clone()
                .map(|_| None)
                .unwrap_or_else(|| Some(PLAN_IMPLEMENTATION_DEFAULT_UNAVAILABLE.to_string())),
            dismiss_on_select: true,
            ..Default::default()
        });
    } else {
        // 无多方案时退化到现有两项实施动作
        items.push(SelectionItem {
            name: PLAN_IMPLEMENTATION_YES.to_string(),
            description: Some("Switch to Default and start coding.".to_string()),
            selected_description: None,
            is_current: false,
            actions: implement_actions,
            disabled_reason: implement_disabled_reason,
            dismiss_on_select: true,
            ..Default::default()
        });

        let clear_context_description = clear_context_usage_label.map_or_else(
            || "Fresh thread with this plan.".to_string(),
            |label| format!("Fresh thread. Context: {label}."),
        );
        items.push(SelectionItem {
            name: PLAN_IMPLEMENTATION_CLEAR_CONTEXT.to_string(),
            description: Some(clear_context_description),
            selected_description: None,
            is_current: false,
            actions: clear_context_actions,
            disabled_reason: clear_context_disabled_reason,
            dismiss_on_select: true,
            ..Default::default()
        });
    }

    // 继续规划始终放在最后
    items.push(SelectionItem {
        name: if has_options {
            "Continue planning".to_string()
        } else {
            PLAN_IMPLEMENTATION_NO.to_string()
        },
        description: Some("Keep Plan mode and continue the conversation.".to_string()),
        selected_description: None,
        is_current: false,
        actions: Vec::new(),
        disabled_reason: None,
        dismiss_on_select: true,
        ..Default::default()
    });

    SelectionViewParams {
        title: Some(PLAN_IMPLEMENTATION_TITLE.to_string()),
        subtitle,
        footer_hint: Some(standard_popup_hint_line()),
        items,
        ..Default::default()
    }
}

impl ChatWidget {
    /// Open a free-form feedback prompt for revising the current plan.
    pub(crate) fn show_plan_revision_prompt(&mut self, collaboration_mode: CollaborationModeMask) {
        let tx = self.app_event_tx.clone();
        let view = CustomPromptView::new(
            "Revise plan".to_string(),
            "What would you like to change?".to_string(),
            /*initial_text*/ String::new(),
            /*context_label*/ None,
            Box::new(move |feedback: String| {
                let trimmed = feedback.trim().to_string();
                if trimmed.is_empty() {
                    return;
                }
                tx.send(AppEvent::SubmitUserMessageWithMode {
                    text: trimmed,
                    collaboration_mode: collaboration_mode.clone(),
                });
            }),
        );
        self.bottom_pane.show_view(Box::new(view));
    }
}
