use super::*;
use crate::bottom_pane::slash_commands::BuiltinCommandFlags;
use crate::bottom_pane::slash_commands::builtins_for_input;
use ody_protocol::ThreadId;
use ody_protocol::config_types::Settings;
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
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::CollaborationModes, /*enabled*/ true);

    // Default -> Plan -> Design
    chat.handle_key_event(KeyEvent::from(KeyCode::BackTab));
    assert_eq!(chat.active_collaboration_mode_kind(), ModeKind::Plan);

    // Plan -> Design opens the audit level picker first.
    chat.handle_key_event(KeyEvent::from(KeyCode::BackTab));
    assert!(chat.bottom_pane.has_active_view());
    assert_eq!(
        chat.bottom_pane.active_view_id(),
        Some("design_audit_level_picker")
    );

    // Select Standard to enter Design mode.
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));
    let mut found_mask = None;
    while let Ok(event) = rx.try_recv() {
        if let AppEvent::SetDesignCollaborationMask { mask, .. } = event {
            found_mask = Some(mask);
        }
    }
    let mask = found_mask.expect("expected SetDesignCollaborationMask event");
    assert_eq!(mask.mode, Some(ModeKind::Design));
    assert_eq!(
        mask.design_audit_level,
        Some(Some(DesignAuditLevel::Standard))
    );
}

#[tokio::test]
async fn session_configured_in_design_mode_opens_audit_level_picker() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::CollaborationModes, /*enabled*/ true);

    let thread_id = ThreadId::new();
    let configured = crate::session_state::ThreadSessionState {
        thread_id,
        forked_from_id: None,
        fork_parent_title: None,
        thread_name: None,
        model: "test-model".to_string(),
        model_provider_id: "test-provider".to_string(),
        service_tier: None,
        approval_policy: ody_app_server_protocol::AskForApproval::Never,
        approvals_reviewer: ody_protocol::config_types::ApprovalsReviewer::User,
        permission_profile: ody_protocol::models::PermissionProfile::read_only(),
        active_permission_profile: None,
        cwd: test_path_buf("/home/user/project").abs(),
        runtime_workspace_roots: Vec::new(),
        instruction_source_paths: Vec::new(),
        reasoning_effort: Some(ody_protocol::model_metadata::ReasoningEffort::Medium),
        collaboration_mode: Some(Box::new(ody_protocol::config_types::CollaborationMode {
            mode: ModeKind::Design,
            settings: Settings {
                model: "test-model".to_string(),
                reasoning_effort: Some(ody_protocol::model_metadata::ReasoningEffort::Medium),
                developer_instructions: Some("design instructions".to_string()),
                design_audit_level: None,
            },
        })),
        personality: None,
        message_history: None,
        network_proxy: None,
        rollout_path: None,
    };
    chat.handle_thread_session(configured);

    assert!(chat.bottom_pane.has_active_view());
    assert_eq!(
        chat.bottom_pane.active_view_id(),
        Some("design_audit_level_picker")
    );

    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));
    let mut found_mask = None;
    while let Ok(event) = rx.try_recv() {
        if let AppEvent::SetDesignCollaborationMask { mask, .. } = event {
            found_mask = Some(mask);
        }
    }
    let mask = found_mask.expect("expected SetDesignCollaborationMask event");
    assert_eq!(mask.mode, Some(ModeKind::Design));
    assert_eq!(
        mask.design_audit_level,
        Some(Some(DesignAuditLevel::Standard))
    );
}

#[tokio::test]
async fn design_mode_renders_design_footer_label() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::CollaborationModes, /*enabled*/ true);
    let mut design_mask = collaboration_modes::design_mask(chat.model_catalog.as_ref())
        .expect("expected design collaboration mode");
    design_mask.design_audit_level = Some(Some(DesignAuditLevel::Standard));
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

#[tokio::test]
async fn design_audit_picker_uses_configured_language() {
    let (mut chat, _rx, _op_rx) =
        make_chatwidget_manual_with_language(/*model_override*/ None, Some("zh-CN")).await;
    chat.set_feature_enabled(Feature::CollaborationModes, true);

    chat.dispatch_command(SlashCommand::Design);

    assert!(chat.bottom_pane.has_active_view());
    assert_eq!(
        chat.bottom_pane.active_view_id(),
        Some("design_audit_level_picker")
    );

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    let compact = popup.replace(|c: char| c.is_whitespace(), "");
    assert!(
        compact.contains("选择设计审计级别"),
        "expected Chinese title in popup, got:\n{popup}"
    );
    assert!(
        compact.contains("基础"),
        "expected Chinese 'Basic' item in popup, got:\n{popup}"
    );
    assert!(
        compact.contains("标准"),
        "expected Chinese 'Standard' item in popup, got:\n{popup}"
    );
    assert!(
        compact.contains("深入"),
        "expected Chinese 'Deep' item in popup, got:\n{popup}"
    );
}

/// A non-terminal design checkpoint (`submit_design final: false`) must NOT arm
/// the post-design next-step menu. The model ends its turn to ask a clarifying
/// question between checkpoints, so `TurnComplete` fires while the design is
/// still a skeleton — keying the menu off the mere presence of a completed plan
/// item used to pop "Design ready — what next?" mid-design. Only a *finalized*
/// completed item arms it. Regression test for that bug.
#[tokio::test]
async fn design_checkpoint_does_not_arm_next_step_menu_but_finalize_does() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::CollaborationModes, /*enabled*/ true);
    let mut mask = collaboration_modes::design_mask(chat.model_catalog.as_ref())
        .expect("expected design collaboration mode");
    mask.design_audit_level = Some(Some(DesignAuditLevel::Standard));
    chat.set_collaboration_mask(mask);
    assert_eq!(chat.active_collaboration_mode_kind(), ModeKind::Design);

    let plan_item = |finalized: bool| {
        ServerNotification::ItemCompleted(ItemCompletedNotification {
            thread_id: String::new(),
            turn_id: "turn-1".to_string(),
            completed_at_ms: 0,
            item: AppServerThreadItem::Plan {
                id: "design-1".to_string(),
                text: "# Draft design\n\n## Scope In\n- something".to_string(),
                plan_file_path: None,
                finalized,
            },
        })
    };

    // Checkpoint: completed plan item with finalized = false.
    chat.handle_server_notification(plan_item(/*finalized*/ false), /*replay_kind*/ None);
    assert!(
        !chat.transcript.saw_finalized_plan_item_this_turn,
        "a checkpoint must not arm the next-step menu"
    );
    chat.maybe_prompt_design_next_step();
    assert!(
        !chat.bottom_pane.has_active_view(),
        "checkpoint must not open the post-design next-step menu"
    );

    // Finalization: completed plan item with finalized = true.
    chat.handle_server_notification(plan_item(/*finalized*/ true), /*replay_kind*/ None);
    assert!(
        chat.transcript.saw_finalized_plan_item_this_turn,
        "a finalized design must arm the next-step menu"
    );
    chat.maybe_prompt_design_next_step();
    assert!(
        chat.bottom_pane.has_active_view(),
        "finalized design must open the post-design next-step menu"
    );
}
