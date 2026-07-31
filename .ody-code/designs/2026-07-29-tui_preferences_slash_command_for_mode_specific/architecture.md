# Architecture — TUI `/preferences` slash command

## Approach

Use the existing **BottomPaneView overlay + AppEvent asynchronous persistence** pattern, consistent with `MemoriesSettingsView` and `ExperimentalFeaturesView`. [C:USER]

## Components and data flow

```
User types /preferences
        |
        v
+----------------------------+
| SlashCommand::Preferences  |  (tui/src/slash_command.rs)
+----------------------------+
        |
        v
+----------------------------+
| ChatWidget::dispatch_command|  (tui/src/chatwidget/slash_dispatch.rs)
+----------------------------+
        |
        v
+----------------------------+
| ChatWidget::open_preferences_popup() | (tui/src/chatwidget/settings_popups.rs)
+----------------------------+
        |
        v
+----------------------------+
| PreferencesView (BottomPaneView) | (tui/src/bottom_pane/preferences_view.rs)
+----------------------------+
        |
        |  on change / on blur
        v
+----------------------------+
| AppEvent::PersistDesignReviewPreferences { edits } | (tui/src/app_event.rs)
+----------------------------+
        |
        v
+----------------------------+
| App::dispatch event handler |  (tui/src/app/event_dispatch.rs)
+----------------------------+
        |
        v
+----------------------------+
| App::update_design_review_preferences() | (tui/src/app/config_persistence.rs)
+----------------------------+
        |
        v
+----------------------------+
| config_update::build_design_review_edits() + write_config_batch() | (tui/src/config_update.rs)
+----------------------------+
        |
        v
+----------------------------+
| AppServerSession::ConfigBatchWrite -> config.toml + reload | (app-server protocol)
+----------------------------+
        |
        v
+----------------------------+
| AppEvent::SyncDesignReviewPreferences { design_review } | (tui/src/app_event.rs)
+----------------------------+
        |
        v
+----------------------------+
| PreferencesView replaces local state with server-side truth | (tui/src/bottom_pane/preferences_view.rs)
+----------------------------+
```

## Mode-specific rendering

`PreferencesView` receives the effective collaboration mode at construction time:
- If `mode == ModeKind::Design`: render a form with the 9 `design_review` fields.
- If `mode == ModeKind::Plan` or `ModeKind::Default`: render a centered placeholder with a short explanation and a hint that future releases will add mode-specific settings.

The mode is determined by `ChatWidget` from `active_collaboration_mask.mode` (if a mask is active) or `current_collaboration_mode.mode`. [C:INFERRED]

## Persistence and rollback

1. The view holds a local `DesignReviewEditState` initialized from `ChatWidget.config`.
2. For boolean/enum fields: toggling sends a `PersistDesignReviewPreferences` event immediately.
3. For text fields: edits are held locally until the field loses focus; on blur, validation runs and then an event is sent.
4. The App layer writes a `ConfigBatchWrite` with the changed edits.
5. On success: App updates `self.config` and sends `AppEvent::SyncDesignReviewPreferences { design_review }` so the view can reconcile its state with the persisted truth.
6. On failure: App also sends `AppEvent::SyncDesignReviewPreferences` with the current in-memory `design_review` (the pre-edit truth) and sets the overlay error banner via an event or via `add_error_message` plus the sync payload.
7. On `OkOverridden` write status: App reads the effective config and sends the sync event, matching the existing memory-settings behavior. [C:UPSTREAM]

## Extension points for Default and Plan modes

The `PreferencesView` is split into a generic shell (overlay frame, key handling, error banner) and a mode-specific content enum:

```rust
pub(crate) enum PreferencesContent {
    DefaultPlaceholder,
    PlanPlaceholder,
    DesignEditor(DesignReviewEditState),
}
```

Adding Default or Plan preferences later only requires:
- adding a new state struct to `PreferencesContent`,
- adding the corresponding `ConfigEdit` builder in `config_update.rs`, and
- adding an `AppEvent` variant and handler if the new preferences need app-server persistence.

No changes to the slash command, dispatch, or overlay framework are needed. [C:INFERRED]
