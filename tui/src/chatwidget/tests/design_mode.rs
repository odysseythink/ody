use super::*;
use crate::bottom_pane::slash_commands::BuiltinCommandFlags;
use crate::bottom_pane::slash_commands::builtins_for_input;
use pretty_assertions::assert_eq;

fn all_enabled_flags() -> BuiltinCommandFlags {
    BuiltinCommandFlags {
        collaboration_modes_enabled: true,
        connectors_enabled: true,
        plugins_command_enabled: true,
        service_tier_commands_enabled: true,
        goal_command_enabled: true,
        personality_command_enabled: true,
        allow_elevate_sandbox: true,
        side_conversation_active: false,
        plan_mode_active: false,
    }
}

#[tokio::test]
async fn design_slash_command_switches_to_design_mode() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::CollaborationModes, /*enabled*/ true);

    chat.dispatch_command(SlashCommand::Design);

    // No preset audit level, so the host-managed picker opens first.
    assert!(chat.bottom_pane.has_active_view());
    assert_eq!(
        chat.bottom_pane.active_view_id(),
        Some("design_audit_level_picker")
    );

    // Select the default Standard item.
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    let event = rx
        .try_recv()
        .expect("expected SetDesignCollaborationMask event");
    let AppEvent::SetDesignCollaborationMask {
        mask,
        pending_user_message,
    } = event
    else {
        panic!("expected SetDesignCollaborationMask, got {event:?}");
    };
    assert_eq!(mask.mode, Some(ModeKind::Design));
    assert_eq!(
        mask.design_audit_level,
        Some(Some(DesignAuditLevel::Standard))
    );
    assert!(pending_user_message.is_none());

    // Apply the event payload as the app layer would.
    chat.set_collaboration_mask(mask);
    assert_eq!(chat.active_collaboration_mode_kind(), ModeKind::Design);
}

#[tokio::test]
async fn design_slash_command_opens_audit_level_picker() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(None).await;
    chat.set_feature_enabled(Feature::CollaborationModes, true);

    chat.dispatch_command(SlashCommand::Design);

    assert!(chat.bottom_pane.has_active_view());
    assert_eq!(
        chat.bottom_pane.active_view_id(),
        Some("design_audit_level_picker")
    );
}

#[tokio::test]
async fn selecting_standard_in_picker_switches_to_design_mode() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(None).await;
    chat.set_feature_enabled(Feature::CollaborationModes, true);

    chat.dispatch_command(SlashCommand::Design);

    // Simulate pressing Enter on the default Standard item.
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    let event = rx
        .try_recv()
        .expect("expected SetDesignCollaborationMask event");
    let AppEvent::SetDesignCollaborationMask {
        mask,
        pending_user_message,
    } = event
    else {
        panic!("expected SetDesignCollaborationMask, got {event:?}");
    };
    assert_eq!(mask.mode, Some(ModeKind::Design));
    assert_eq!(
        mask.design_audit_level,
        Some(Some(DesignAuditLevel::Standard))
    );
    assert!(pending_user_message.is_none());
}

#[tokio::test]
async fn design_mode_with_configured_audit_level_skips_picker() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(None).await;
    chat.set_feature_enabled(Feature::CollaborationModes, true);
    let mut mask = collaboration_modes::design_mask(chat.model_catalog.as_ref())
        .expect("expected design collaboration mode");
    mask.design_audit_level = Some(Some(DesignAuditLevel::Deep));

    chat.set_collaboration_mask(mask.clone());

    // /design with a preset level should apply directly without opening a picker.
    chat.dispatch_command(SlashCommand::Design);
    assert!(!chat.bottom_pane.has_active_view());
    assert_eq!(chat.active_collaboration_mode_kind(), ModeKind::Design);
}

#[tokio::test]
async fn shift_tab_cycles_through_design_mode() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::CollaborationModes, /*enabled*/ true);

    // Default -> Plan -> Design
    chat.handle_key_event(KeyEvent::from(KeyCode::BackTab));
    assert_eq!(chat.active_collaboration_mode_kind(), ModeKind::Plan);
    chat.handle_key_event(KeyEvent::from(KeyCode::BackTab));
    assert_eq!(chat.active_collaboration_mode_kind(), ModeKind::Design);
}

#[tokio::test]
async fn design_mode_renders_design_footer_label() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::CollaborationModes, /*enabled*/ true);
    let design_mask = collaboration_modes::design_mask(chat.model_catalog.as_ref())
        .expect("expected design collaboration mode");
    chat.set_collaboration_mask(design_mask);

    assert_eq!(chat.active_collaboration_mode_kind(), ModeKind::Design);
    assert_eq!(chat.collaboration_mode_label(), Some("Design"));
}

#[tokio::test]
async fn inline_design_command_submits_message_after_picker_selection() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(None).await;
    chat.set_feature_enabled(Feature::CollaborationModes, true);

    chat.dispatch_command_with_args(SlashCommand::Design, "设计一个缓存".to_string(), Vec::new());

    assert!(chat.bottom_pane.has_active_view());
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    let mut found_mask = None;
    while let Ok(event) = rx.try_recv() {
        if let AppEvent::SetDesignCollaborationMask {
            mask,
            pending_user_message,
        } = event
        {
            assert_eq!(
                mask.design_audit_level,
                Some(Some(DesignAuditLevel::Standard))
            );
            assert!(pending_user_message.is_some());
            found_mask = Some(mask);
        }
    }
    assert!(
        found_mask.is_some(),
        "expected SetDesignCollaborationMask event"
    );
}

#[test]
fn design_command_hidden_when_collaboration_modes_disabled() {
    let mut flags = all_enabled_flags();
    flags.collaboration_modes_enabled = false;
    assert!(
        builtins_for_input(flags)
            .into_iter()
            .all(|(_, c)| c != SlashCommand::Design && c != SlashCommand::Plan)
    );
}
