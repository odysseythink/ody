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
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::CollaborationModes, /*enabled*/ true);

    chat.dispatch_command(SlashCommand::Design);

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

#[test]
fn design_command_hidden_when_collaboration_modes_disabled() {
    let mut flags = all_enabled_flags();
    flags.collaboration_modes_enabled = false;
    assert!(builtins_for_input(flags)
        .into_iter()
        .all(|(_, c)| c != SlashCommand::Design && c != SlashCommand::Plan));
}
