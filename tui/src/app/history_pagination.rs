//! Load older transcript pages without rewriting terminal-native scrollback.

use super::*;
use crate::app_server_session::INITIAL_HISTORY_TURN_LIMIT;
use ody_app_server_protocol::ClientRequest;
use ody_app_server_protocol::SortDirection;
use ody_app_server_protocol::ThreadTurnsListParams;
use ody_app_server_protocol::ThreadTurnsListResponse;
use ody_app_server_protocol::TurnItemsView;

impl App {
    /// Start one bounded older-history page request for scrollback refill.
    ///
    /// Returns false when a request is already in flight or the paginated
    /// history is exhausted.
    pub(crate) fn request_older_history_page(
        &self,
        app_server: &mut AppServerSession,
        thread_id: ThreadId,
    ) -> bool {
        let Some(cursor) = app_server.begin_older_history_page(thread_id) else {
            return false;
        };
        let request_id = app_server.next_request_id_pub();
        let request_handle = app_server.request_handle();
        let app_event_tx = self.app_event_tx.clone();
        tokio::spawn(async move {
            let result = request_handle
                .request_typed::<ThreadTurnsListResponse>(ClientRequest::ThreadTurnsList {
                    request_id,
                    params: ThreadTurnsListParams {
                        thread_id: thread_id.to_string(),
                        cursor: Some(cursor.clone()),
                        limit: Some(INITIAL_HISTORY_TURN_LIMIT),
                        sort_direction: Some(SortDirection::Desc),
                        items_view: Some(TurnItemsView::Full),
                    },
                })
                .await
                .map_err(|err| err.to_string());
            let _ = app_event_tx.send(AppEvent::OlderThreadHistoryLoaded {
                thread_id,
                cursor,
                result,
            });
        });
        true
    }

    pub(super) async fn handle_older_history_page(
        &mut self,
        tui: &mut tui::Tui,
        app_server: &mut AppServerSession,
        thread_id: ThreadId,
        cursor: &str,
        result: Result<ThreadTurnsListResponse, String>,
    ) -> Result<()> {
        if self.chat_widget.thread_id() != Some(thread_id) {
            app_server.cancel_older_history_page(thread_id);
            return Ok(());
        }
        let page = match result {
            Ok(page) => page,
            Err(err) => {
                // Failed page fetches must not wedge the loading_older guard.
                app_server.cancel_older_history_page(thread_id);
                tracing::warn!(%thread_id, %err, "failed to load older thread history page");
                return Ok(());
            }
        };
        let older_turns = app_server.apply_older_turns_page(thread_id, cursor, page);
        if older_turns.is_empty() {
            return Ok(());
        }
        let Some(store) = self
            .thread_event_channels
            .get(&thread_id)
            .map(|channel| Arc::clone(&channel.store))
        else {
            return Ok(());
        };
        let (cwd, older_turns) = {
            let mut store = store.lock().await;
            let cwd = store
                .session
                .as_ref()
                .map_or_else(|| self.config.cwd.clone(), |session| session.cwd.clone());
            let older_turns: Vec<_> = older_turns
                .into_iter()
                .filter(|turn| !store.turns.iter().any(|known| known.id == turn.id))
                .collect();
            store.turns.splice(0..0, older_turns.clone());
            (cwd, older_turns)
        };
        let visibility = if self.config.show_raw_agent_reasoning {
            crate::thread_transcript::RawReasoningVisibility::Visible
        } else {
            crate::thread_transcript::RawReasoningVisibility::Hidden
        };
        let cells = crate::thread_transcript::turns_to_transcript_cells(
            cwd.as_path(),
            &older_turns,
            visibility,
        );
        if cells.is_empty() {
            return Ok(());
        }
        self.scrollback_has_older_history = app_server.has_older_history(thread_id);
        // Insert older cells below the session banner so the banner stays on top.
        let index = self
            .transcript_cells
            .iter()
            .rposition(|cell| cell.as_any().is::<history_cell::SessionInfoCell>())
            .map_or(/*default*/ 0, |index| index.saturating_add(/*rhs*/ 1));
        self.transcript_cells.splice(index..index, cells);
        let width = self
            .chat_widget
            .history_wrap_width(tui.terminal.last_known_screen_size.width);
        let rendered_rows = self.render_transcript_lines_for_reflow(width).lines.len();
        self.schedule_immediate_resize_reflow(tui);
        if self.scrollback_history_needs_top_up(rendered_rows)
            && self.request_older_history_page(app_server, thread_id)
        {
            return Ok(());
        }
        tui.frame_requester().schedule_frame();
        Ok(())
    }
}
