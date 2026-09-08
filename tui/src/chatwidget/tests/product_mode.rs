//! Post-product next-step menu: `submit_product` finalizes the requirements
//! document with a finalized completed plan item; the TUI must then offer the
//! handoff menu (Enter Plan / Enter Design / Stay). Mirrors the post-design
//! gating tests in `design_mode.rs`.

use super::*;
use ody_protocol::config_types::CollaborationModeMask;
use ody_protocol::config_types::ModeKind;

fn product_mask() -> CollaborationModeMask {
    CollaborationModeMask {
        name: "Product".to_string(),
        mode: Some(ModeKind::Product),
        model: None,
        reasoning_effort: None,
        developer_instructions: None,
        design_audit_level: None,
    }
}

/// Only a *finalized* completed plan item arms the post-product handoff menu.
/// Product turns complete plan items only via `submit_product` (which sets
/// `finalized: true`), but the gate is the shared flag — pin both directions
/// so a regression in the flag plumbing cannot silently open or close it.
#[tokio::test]
async fn product_finalize_arms_next_step_menu_but_checkpoint_does_not() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::CollaborationModes, /*enabled*/ true);
    chat.set_collaboration_mask(product_mask());
    assert_eq!(chat.active_collaboration_mode_kind(), ModeKind::Product);

    let plan_item = |finalized: bool| {
        ServerNotification::ItemCompleted(ItemCompletedNotification {
            thread_id: String::new(),
            turn_id: "turn-1".to_string(),
            completed_at_ms: 0,
            item: AppServerThreadItem::Plan {
                id: "product-1".to_string(),
                text: "# Requirements\n\n## Open Questions\n- none".to_string(),
                plan_file_path: None,
                finalized,
            },
        })
    };

    // Non-finalized completion (anything else that completes a plan item):
    // no menu.
    chat.handle_server_notification(plan_item(/*finalized*/ false), /*replay_kind*/ None);
    assert!(!chat.transcript.saw_finalized_plan_item_this_turn);
    chat.maybe_prompt_product_next_step();
    assert!(
        !chat.bottom_pane.has_active_view(),
        "a non-finalized completion must not open the post-product handoff menu"
    );

    // Finalized completion (what submit_product emits): menu opens.
    chat.handle_server_notification(plan_item(/*finalized*/ true), /*replay_kind*/ None);
    assert!(chat.transcript.saw_finalized_plan_item_this_turn);
    chat.maybe_prompt_product_next_step();
    assert!(
        chat.bottom_pane.has_active_view(),
        "submit_product's finalized item must open the post-product handoff menu"
    );
}

/// The menu must not open while the session is in another mode, even if a
/// finalized item somehow shows up — same cross-mode guard as the design menu.
#[tokio::test]
async fn product_next_step_menu_requires_product_mode() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::CollaborationModes, /*enabled*/ true);
    // Default mode: no product mask.
    chat.handle_server_notification(
        ServerNotification::ItemCompleted(ItemCompletedNotification {
            thread_id: String::new(),
            turn_id: "turn-1".to_string(),
            completed_at_ms: 0,
            item: AppServerThreadItem::Plan {
                id: "product-1".to_string(),
                text: "# Requirements".to_string(),
                plan_file_path: None,
                finalized: true,
            },
        }),
        /*replay_kind*/ None,
    );
    chat.maybe_prompt_product_next_step();
    assert!(
        !chat.bottom_pane.has_active_view(),
        "the post-product menu must never open outside Product mode"
    );
}
