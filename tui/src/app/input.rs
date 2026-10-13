//! Keyboard input, external editor, and status-line dispatch for the TUI app.
//!
//! This module owns global key bindings that sit above ChatWidget, including transcript overlay
//! entry, Ctrl-L clear, external editor launch, and agent navigation shortcuts.

use super::*;
use crate::app_backtrack::SIDE_EDIT_PREVIOUS_UNAVAILABLE_MESSAGE;
use crate::transcript_view::ViewAction;

impl App {
    pub(super) async fn launch_external_editor(&mut self, tui: &mut tui::Tui) {
        let editor_cmd = match external_editor::resolve_editor_command() {
            Ok(cmd) => cmd,
            Err(external_editor::EditorError::MissingEditor) => {
                self.chat_widget
                    .add_to_history(history_cell::new_error_event(
                        "Cannot open external editor: set $VISUAL or $EDITOR before starting Ody."
                            .to_string(),
                    ));
                self.reset_external_editor_state(tui);
                return;
            }
            Err(err) => {
                self.chat_widget
                    .add_to_history(history_cell::new_error_event(format!(
                        "Failed to open editor: {err}",
                    )));
                self.reset_external_editor_state(tui);
                return;
            }
        };

        let seed = self.chat_widget.composer_text_with_pending();
        let editor_result = tui
            .with_restored(tui::RestoreMode::KeepRaw, || async {
                external_editor::run_editor(&seed, &editor_cmd).await
            })
            .await;
        self.reset_external_editor_state(tui);

        match editor_result {
            Ok(new_text) => {
                // Trim trailing whitespace
                let cleaned = new_text.trim_end().to_string();
                self.chat_widget.apply_external_edit(cleaned);
            }
            Err(err) => {
                self.chat_widget
                    .add_to_history(history_cell::new_error_event(format!(
                        "Failed to open editor: {err}",
                    )));
            }
        }
        tui.frame_requester().schedule_frame();
    }

    pub(super) fn request_external_editor_launch(&mut self, tui: &mut tui::Tui) {
        self.chat_widget
            .set_external_editor_state(ExternalEditorState::Requested);
        self.chat_widget.set_footer_hint_override(Some(vec![(
            EXTERNAL_EDITOR_HINT.to_string(),
            String::new(),
        )]));
        tui.frame_requester().schedule_frame();
    }

    pub(super) fn reset_external_editor_state(&mut self, tui: &mut tui::Tui) {
        self.chat_widget
            .set_external_editor_state(ExternalEditorState::Closed);
        self.chat_widget.set_footer_hint_override(/*items*/ None);
        tui.frame_requester().schedule_frame();
    }

    pub(super) fn apply_raw_output_mode(
        &mut self,
        tui: &mut tui::Tui,
        enabled: bool,
        notify: bool,
    ) {
        if notify {
            self.chat_widget.set_raw_output_mode_and_notify(enabled);
        } else {
            self.chat_widget.set_raw_output_mode(enabled);
        }
        if let Err(err) = self.reflow_transcript_now(tui) {
            tracing::warn!(error = %err, "failed to reflow transcript after raw output mode toggle");
            self.chat_widget
                .add_error_message(format!("Failed to redraw transcript: {err}"));
        }
        tui.frame_requester().schedule_frame();
    }

    pub(super) async fn handle_key_event(
        &mut self,
        tui: &mut tui::Tui,
        app_server: &mut AppServerSession,
        key_event: KeyEvent,
    ) {
        // Some terminals, especially on macOS, encode Option+Left/Right as Option+b/f unless
        // enhanced keyboard reporting is available. We only treat those word-motion fallbacks as
        // agent-switch shortcuts when the composer is empty so we never steal the expected
        // editing behavior for moving across words inside a draft.
        let allow_agent_word_motion_fallback = !self.enhanced_keys_supported
            && self.chat_widget.composer_text_with_pending().is_empty();
        if self.overlay.is_none()
            && self.chat_widget.no_modal_or_popup_active()
            // Alt+Left/Right are also natural word-motion keys in the composer. Keep agent
            // fast-switch available only once the draft is empty so editing behavior wins whenever
            // there is text on screen.
            && self.chat_widget.composer_text_with_pending().is_empty()
            && previous_agent_shortcut_matches(key_event, allow_agent_word_motion_fallback)
        {
            if let Some(thread_id) = self
                .adjacent_thread_id_with_backfill(app_server, AgentNavigationDirection::Previous)
                .await
            {
                let _ = self
                    .select_agent_thread_and_discard_side(tui, app_server, thread_id)
                    .await;
            }
            return;
        }
        if self.overlay.is_none()
            && self.chat_widget.no_modal_or_popup_active()
            // Mirror the previous-agent rule above: empty drafts may use these keys for thread
            // switching, but non-empty drafts keep them for expected word-wise cursor motion.
            && self.chat_widget.composer_text_with_pending().is_empty()
            && next_agent_shortcut_matches(key_event, allow_agent_word_motion_fallback)
        {
            if let Some(thread_id) = self
                .adjacent_thread_id_with_backfill(app_server, AgentNavigationDirection::Next)
                .await
            {
                let _ = self
                    .select_agent_thread_and_discard_side(tui, app_server, thread_id)
                    .await;
            }
            return;
        }
        if side_return_shortcut_matches(key_event)
            && self.maybe_return_from_side(tui, app_server).await
        {
            return;
        }

        let app_keymap_shortcuts_available = self.app_keymap_shortcuts_available();
        let owned_transcript_input_available = self.owned_transcript_accepts_input(tui);

        // With a selection up, the copy binding copies what the reader selected instead of the last
        // agent message. Without one it keeps its usual meaning further down.
        if app_keymap_shortcuts_available
            && self.transcript_view.has_selection()
            && self.keymap.app.copy.is_pressed(key_event)
        {
            if let Some(text) = self.transcript_view.copy_selection(&self.transcript_cells) {
                self.copy_view_text(tui, text, /*follow*/ false);
            } else {
                self.transcript_view.clear_selection();
                self.schedule_view_frame(tui);
            }
            return;
        }

        if app_keymap_shortcuts_available && self.keymap.app.toggle_vim_mode.is_pressed(key_event) {
            self.chat_widget.toggle_vim_mode_and_notify();
            return;
        }

        if app_keymap_shortcuts_available
            && self.keymap.app.toggle_fast_mode.is_pressed(key_event)
            && self.chat_widget.can_toggle_fast_mode_from_keybinding()
        {
            self.chat_widget.toggle_fast_mode_from_ui();
            return;
        }

        if app_keymap_shortcuts_available && self.keymap.app.toggle_raw_output.is_pressed(key_event)
        {
            let enabled = !self.chat_widget.raw_output_mode();
            self.apply_raw_output_mode(tui, enabled, /*notify*/ false);
            return;
        }

        if app_keymap_shortcuts_available && self.keymap.app.open_transcript.is_pressed(key_event) {
            // Enter alternate screen and set viewport to full size.
            let _ = tui.enter_alt_screen();
            self.overlay = Some(Overlay::new_transcript(
                self.transcript_cells.clone(),
                self.keymap.pager.clone(),
                self.config.tui_mouse_scroll_speed.unwrap_or(1.0),
            ));
            tui.frame_requester().schedule_frame();
            return;
        }

        if app_keymap_shortcuts_available
            && self.keymap.app.open_external_editor.is_pressed(key_event)
        {
            // Only launch the external editor if there is no overlay and the bottom pane is not in use.
            // Note that it can be launched while a task is running to enable editing while the previous turn is ongoing.
            if self.overlay.is_none()
                && self.chat_widget.can_launch_external_editor()
                && self.chat_widget.external_editor_state() == ExternalEditorState::Closed
            {
                self.request_external_editor_launch(tui);
            }
            return;
        }

        // Find-in-transcript. `F3` follows the upstream default and is unused by Ody's keymap, so
        // it does not take a binding away from the composer or the pager.
        if owned_transcript_input_available
            && key_event.kind == KeyEventKind::Press
            && key_event.code == KeyCode::F(3)
        {
            self.transcript_view.begin_search();
            self.schedule_view_frame(tui);
            return;
        }

        // The owned transcript answers its own gestures before the app-level Esc ladder runs: find
        // mode closes, a selection clears, and reading back returns to the newest content. Only when
        // it declines the key does Esc keep its existing backtrack meaning.
        if owned_transcript_input_available && self.route_owned_transcript_key(tui, key_event) {
            return;
        }

        if matches!(key_event.code, KeyCode::Esc)
            && matches!(key_event.kind, KeyEventKind::Press | KeyEventKind::Repeat)
        {
            // Esc primes/advances backtracking only in normal (not working) mode
            // with the composer focused and empty. In any other state, forward
            // Esc so the active UI (e.g. status indicator, modals, popups)
            // handles it.
            if self.should_handle_backtrack_esc(key_event) {
                self.handle_backtrack_esc_key(tui);
            } else if self.should_reject_side_backtrack_esc(key_event) {
                self.reject_side_backtrack_esc();
            } else {
                self.chat_widget.handle_key_event(key_event);
            }
            return;
        }

        match key_event {
            _ if app_keymap_shortcuts_available
                && self.keymap.app.clear_terminal.is_pressed(key_event) =>
            {
                if !self.chat_widget.can_run_ctrl_l_clear_now() {
                    return;
                }
                if let Err(err) = self.clear_terminal_ui(tui, /*redraw_header*/ false) {
                    tracing::warn!(error = %err, "failed to clear terminal UI");
                    self.chat_widget
                        .add_error_message(format!("Failed to clear terminal UI: {err}"));
                } else {
                    self.reset_app_ui_state_after_clear();
                    self.queue_clear_ui_header(tui);
                    tui.frame_requester().schedule_frame();
                }
            }
            // Enter confirms backtrack when primed + count > 0. Otherwise pass to widget.
            KeyEvent {
                code: KeyCode::Enter,
                kind: KeyEventKind::Press,
                ..
            } if self.backtrack.primed
                && self.backtrack.nth_user_message != usize::MAX
                && self.chat_widget.composer_is_empty() =>
            {
                if let Some(selection) = self.confirm_backtrack_from_main() {
                    self.apply_backtrack_selection(tui, selection);
                }
            }
            KeyEvent {
                kind: KeyEventKind::Press | KeyEventKind::Repeat,
                ..
            } => {
                // Any non-Esc key press should cancel a primed backtrack.
                // This avoids stale "Esc-primed" state after the user starts typing
                // (even if they later backspace to empty).
                if key_event.code != KeyCode::Esc && self.backtrack.primed {
                    self.reset_backtrack_state();
                }
                self.chat_widget.handle_key_event(key_event);
            }
            _ => {
                self.chat_widget.handle_key_event(key_event);
            }
        };
    }

    pub(super) fn should_handle_backtrack_esc(&self, key_event: KeyEvent) -> bool {
        !self.chat_widget.side_conversation_active()
            && self.chat_widget.is_normal_backtrack_mode()
            && self.chat_widget.composer_is_empty()
            && !self.chat_widget.should_handle_vim_insert_escape(key_event)
    }

    pub(super) fn should_reject_side_backtrack_esc(&self, key_event: KeyEvent) -> bool {
        self.chat_widget.side_conversation_active()
            && self.chat_widget.is_normal_backtrack_mode()
            && self.chat_widget.composer_is_empty()
            && !self.chat_widget.should_handle_vim_insert_escape(key_event)
    }

    pub(super) fn reject_side_backtrack_esc(&mut self) {
        self.reset_backtrack_state();
        self.chat_widget
            .add_error_message(SIDE_EDIT_PREVIOUS_UNAVAILABLE_MESSAGE.to_string());
    }

    fn app_keymap_shortcuts_available(&self) -> bool {
        self.overlay.is_none() && self.chat_widget.no_modal_or_popup_active()
    }

    /// Whether the owned transcript view answers an event.
    ///
    /// Overlays and popups route their own mouse and key input, and Terminal mode keeps using the
    /// terminal's native scrollback, so only the bare owned view answers here.
    pub(super) fn owned_transcript_accepts_input(&self, tui: &tui::Tui) -> bool {
        tui.is_owned_screen()
            && self.overlay.is_none()
            && self.chat_widget.no_modal_or_popup_active()
    }

    /// Hand a key to the owned transcript view, returning whether it consumed the key.
    ///
    /// Find mode owns typing while the query is being edited. Outside it the view only claims its
    /// own gestures, and navigation is additionally gated on an empty draft because those keys are
    /// also legitimate draft text.
    ///
    /// The ownership check is repeated here so no caller can route around an overlay or popup.
    pub(super) fn route_owned_transcript_key(
        &mut self,
        tui: &mut tui::Tui,
        key_event: KeyEvent,
    ) -> bool {
        if !self.owned_transcript_accepts_input(tui) {
            return false;
        }
        let claimed = self.transcript_view.owns_interaction_key(key_event)
            || (self.chat_widget.composer_is_empty()
                && self
                    .transcript_view
                    .owns_navigation_key(key_event, &self.keymap.pager));
        if !claimed {
            return false;
        }
        let action = self.transcript_view.handle_key(
            key_event,
            &self.transcript_cells,
            &self.keymap.pager,
        );
        match action {
            Some(action) => {
                self.apply_view_action(tui, action);
                true
            }
            None => false,
        }
    }

    /// Apply one action the transcript view asked for.
    ///
    /// Clipboard and browser access live here because the view cannot reach those services.
    pub(super) fn apply_view_action(&mut self, tui: &mut tui::Tui, action: ViewAction) {
        match action {
            ViewAction::Changed => self.schedule_view_frame(tui),
            ViewAction::Copy(text) => self.copy_view_text(tui, text, /*follow*/ false),
            ViewAction::CopyAndFollow(text) => self.copy_view_text(tui, text, /*follow*/ true),
            ViewAction::OpenLink(destination) => {
                if let Err(error) = webbrowser::open(&destination) {
                    self.transcript_view
                        .set_notice(format!("Failed to open link: {error}"));
                    tracing::warn!(%error, %destination, "failed to open transcript link");
                }
                self.schedule_view_frame(tui);
            }
        }
    }

    fn copy_view_text(&mut self, tui: &mut tui::Tui, text: String, follow: bool) {
        let characters = text.chars().count();
        self.last_transcript_copy = Some(text.clone());
        match crate::clipboard_copy::copy_to_clipboard(&text) {
            Ok(lease) => {
                self.chat_widget.store_clipboard_lease(lease);
                self.transcript_view
                    .set_notice(format!("Copied {characters} chars to clipboard"));
            }
            Err(error) => {
                self.transcript_view
                    .set_notice(format!("Copy failed: {error}"));
                tracing::warn!(%error, "failed to copy transcript selection");
            }
        }
        if follow {
            self.transcript_view.jump_to_latest();
        }
        self.schedule_view_frame(tui);
    }

    /// Request a repaint, and keep the find scan going while it still has entries to cover.
    pub(super) fn schedule_view_frame(&mut self, tui: &mut tui::Tui) {
        if tui.is_owned_screen() && self.transcript_view.interaction_needs_frame() {
            self.transcript_view.search_step(&self.transcript_cells);
        }
        tui.frame_requester().schedule_frame();
    }

    /// Route mouse input that the owned transcript handles itself.
    ///
    /// Mouse capture is enabled for the owned screen, so wheel, click and drag events arrive even
    /// with no overlay present.
    pub(super) fn handle_mouse_event(&mut self, tui: &mut tui::Tui, event: MouseEvent) {
        if !self.owned_transcript_accepts_input(tui) {
            return;
        }
        if let Some(action) = self
            .transcript_view
            .handle_mouse(event, &self.transcript_cells)
        {
            self.apply_view_action(tui, action);
        }
    }

    pub(super) fn refresh_status_line(&mut self) {
        self.chat_widget.refresh_status_line();
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::make_test_app;

    #[tokio::test]
    async fn app_keymap_shortcuts_are_disabled_while_keymap_view_is_active() {
        let mut app = make_test_app().await;
        assert!(app.app_keymap_shortcuts_available());

        let keymap = app.keymap.clone();
        app.chat_widget.open_keymap_debug(&keymap);

        assert!(!app.app_keymap_shortcuts_available());
    }
}
