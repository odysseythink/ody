use super::*;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::bottom_pane_view::BottomPaneView;
use crate::keymap::RuntimeKeymap;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use ody_config::config_toml::DesignReviewToml;
use ody_protocol::config_types::ModeKind;
use tokio::sync::mpsc::unbounded_channel;

fn make_view(
    mode: ModeKind,
    state: DesignReviewEditState,
) -> (
    PreferencesView,
    tokio::sync::mpsc::UnboundedReceiver<AppEvent>,
) {
    let (tx, rx) = unbounded_channel::<AppEvent>();
    let view = PreferencesView::new(
        mode,
        PreferencesContent::Design { state },
        AppEventSender::new(tx),
        RuntimeKeymap::defaults().list,
    );
    (view, rx)
}

#[test]
fn design_view_renders_all_nine_fields() {
    let (view, _rx) = make_view(ModeKind::Design, DesignReviewEditState::default());
    let rows = view.build_rows();
    assert_eq!(rows.len(), 9);
    assert!(rows[0].name.contains("Enable design review"));
    assert!(rows[1].name.contains("Review model"));
    assert!(rows[2].name.contains("Enable debate"));
    assert!(rows[3].name.contains("Debate rounds"));
    assert!(rows[4].name.contains("Advocate model"));
    assert!(rows[5].name.contains("Skeptic model"));
    assert!(rows[6].name.contains("Judge model"));
    assert!(rows[7].name.contains("Contest critic"));
    assert!(rows[8].name.contains("Usability lens"));
}

#[test]
fn toggling_enable_emits_persist_event() {
    let (mut view, mut rx) = make_view(ModeKind::Design, DesignReviewEditState::default());
    view.handle_key_event(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    match rx.try_recv() {
        Ok(AppEvent::PersistDesignReviewPreferences { edits }) => {
            assert!(edits.iter().any(|e| {
                e.key_path == "design_review.enable" && e.value == serde_json::json!(true)
            }));
        }
        other => panic!("expected PersistDesignReviewPreferences, got {other:?}"),
    }
}

#[test]
fn cycling_usability_lens_emits_persist_event() {
    let (mut view, mut rx) = make_view(ModeKind::Design, DesignReviewEditState::default());
    for _ in 0..8 {
        view.move_down();
    }
    view.handle_key_event(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    match rx.try_recv() {
        Ok(AppEvent::PersistDesignReviewPreferences { edits }) => {
            assert!(edits.iter().any(|e| {
                e.key_path == "design_review.debate.usability_lens"
                    && e.value == serde_json::json!("on")
            }));
        }
        other => panic!("expected PersistDesignReviewPreferences, got {other:?}"),
    }
}

#[test]
fn placeholder_mode_has_no_fields() {
    let (tx, _rx) = unbounded_channel::<AppEvent>();
    let view = PreferencesView::new(
        ModeKind::Default,
        PreferencesContent::Placeholder,
        AppEventSender::new(tx),
        RuntimeKeymap::defaults().list,
    );
    assert_eq!(view.build_rows().len(), 0);
}

#[test]
fn sync_event_with_error_restores_baseline() {
    let (mut view, _rx) = make_view(
        ModeKind::Design,
        DesignReviewEditState {
            enable: true,
            ..Default::default()
        },
    );
    view.toggle_bool(PreferencesField::Enable);
    assert!(!view.edit_state.enable);

    let event = AppEvent::SyncDesignReviewPreferences {
        design_review: DesignReviewToml::default(),
        error: Some("write failed".to_string()),
    };
    assert!(view.handle_app_event(&event));
    assert!(view.edit_state.enable);
    assert_eq!(view.error_message, Some("write failed".to_string()));
}

#[test]
fn sync_event_without_error_applies_toml() {
    let (mut view, _rx) = make_view(ModeKind::Design, DesignReviewEditState::default());
    let event = AppEvent::SyncDesignReviewPreferences {
        design_review: DesignReviewToml {
            enable: true,
            review_model: Some("m".to_string()),
            debate: None,
        },
        error: None,
    };
    assert!(view.handle_app_event(&event));
    assert!(view.edit_state.enable);
    assert_eq!(view.edit_state.review_model, Some("m".to_string()));
    assert_eq!(view.error_message, None);
}

#[test]
fn rounds_validation_rejects_out_of_bounds() {
    let (mut view, _rx) = make_view(ModeKind::Design, DesignReviewEditState::default());
    assert!(!view.apply_rounds_text("0".to_string()));
    assert!(!view.apply_rounds_text("4".to_string()));
    assert!(view.apply_rounds_text("3".to_string()));
    assert_eq!(view.edit_state.rounds, Some(3));
}
