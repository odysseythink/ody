# Algorithms — TUI `/preferences` slash command

## Opening the popup

```
function ChatWidget::open_preferences_popup():
    if slash_command_blocked_by_active_task(Preferences):
        add_error_message("'/preferences' is unavailable while a task is running.")
        return

    if active_side_conversation:
        add_error_message("'/preferences' is unavailable in side conversations.")
        return

    let mode = active_collaboration_mask.mode
                 .or(current_collaboration_mode.mode)
                 .unwrap_or(ModeKind::Default)

    let content = match mode:
        ModeKind::Design => PreferencesContent::DesignEditor(
            DesignReviewEditState::from_config(&self.config)
        )
        ModeKind::Plan => PreferencesContent::PlanPlaceholder
        ModeKind::Default => PreferencesContent::DefaultPlaceholder
        _ => PreferencesContent::DefaultPlaceholder  // non-TUI-visible modes

    let view = PreferencesView::new(
        content,
        mode,
        self.app_event_tx.clone(),
        self.bottom_pane.list_keymap(),
    )
    self.bottom_pane.show_view(Box::new(view))
```

## Design editor rendering (high-level)

```
function PreferencesView::render(area, buffer):
    draw centered modal frame with title "Preferences — Design mode"
    if error_message is Some:
        draw error banner at top

    match content:
        DefaultPlaceholder -> draw "No preferences available for Default mode yet." + hint
        PlanPlaceholder -> draw "No preferences available for Plan mode yet." + hint
        DesignEditor(state) -> draw form rows:
            [toggle]  design_review.enable
            [text]    design_review.review_model
            [toggle]  design_review.debate.enable
            [text]    design_review.debate.rounds
            [text]    design_review.debate.advocate_model
            [text]    design_review.debate.skeptic_model
            [text]    design_review.debate.judge_model
            [toggle]  design_review.debate.contest_critic
            [select]  design_review.debate.usability_lens (off / on / ask)

    draw footer hint: "↑/↓ navigate, Enter/Space toggle, Esc/Ctrl+C close, ? help"
```

## Key handling

```
function PreferencesView::handle_key_event(key):
    if key == Esc or (key == Ctrl+C and not editing text):
        set complete = true, completion = Cancelled
        return

    match content:
        Placeholder -> any key closes the view
        DesignEditor(state) ->
            if key == Up:
                move focus to previous field
            if key == Down:
                move focus to next field
            if key == Enter or Space:
                if focused field is boolean:
                    toggle the field
                    on_design_field_changed()
                if focused field is usability_lens enum:
                    cycle to next variant
                    on_design_field_changed()
            if key is printable and focused field is text:
                append to the text buffer; do not persist yet
            if key == Tab and focused field is text:
                on_text_field_blurred(focus)
```

## Field change persistence

```
function PreferencesView::on_design_field_changed():
    if let PreferencesContent::DesignEditor(ref state) = self.content:
        let edits = config_update::build_design_review_edits(state)
        self.app_event_tx.send(AppEvent::PersistDesignReviewPreferences { edits })

function PreferencesView::on_text_field_blurred(focus):
    if let PreferencesContent::DesignEditor(ref mut state) = self.content:
        match focus:
            DebateRounds:
                if state.debate_rounds is not empty:
                    if parse fails or value not in 1..=3:
                        self.error_message = "rounds must be an integer between 1 and 3"
                        return
            _:
                // model fields accept any non-empty string or empty (clear)
                pass
        self.error_message = None
        on_design_field_changed()
```

## App-layer persistence

```
function App::update_design_review_preferences(app_server, edits):
    let write_response = match config_update::write_config_batch(
        app_server.request_handle(),
        edits,
    ).await:
        Ok(response) => response,
        Err(err) => {
            tracing::error!(error = %err, "failed to persist design_review preferences")
            chat_widget.add_error_message("Failed to save design review preferences: {err}")
            send SyncDesignReviewPreferences {
                design_review: self.config.design_review_table_or_default(),
                error: Some(err.to_string()),
            }
            return false
        }

    if write_response.status == OkOverridden:
        let message = overridden_write_message(&write_response)
        tracing::warn!(message, "design_review preferences write was overridden")
        let Some(effective_config) =
            read_effective_config_after_overridden_write(app_server, "Design review preferences")
        else {
            send SyncDesignReviewPreferences {
                design_review: self.config.design_review_table_or_default(),
                error: Some("Could not read effective config after overridden write".to_string()),
            }
            return false
        }
        let design_review = extract_design_review_toml(effective_config)
        update self.config.design_review_* from design_review
        sync chat_widget config if a public setter exists
        send SyncDesignReviewPreferences {
            design_review,
            error: Some(message),
        }
        return true

    // Success
    update self.config.design_review_* from the edits (or reload from disk)
    sync chat_widget config if a public setter exists
    send SyncDesignReviewPreferences {
        design_review: self.config.design_review_table_or_default(),
        error: None,
    }
    return true
```

## State sync on the view

```
function PreferencesView::handle_app_event(event):
    if let AppEvent::SyncDesignReviewPreferences { design_review, error } = event:
        if let PreferencesContent::DesignEditor(ref mut state) = self.content:
            state.apply_from_toml(design_review)
        self.error_message = error
```

`apply_from_toml` overwrites the entire local `DesignReviewEditState` with the persisted truth. Because the builder sends all fields, this is always unambiguous. [C:INFERRED]
