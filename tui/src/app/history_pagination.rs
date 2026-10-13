//! Load older transcript pages into the owned view and the native scrollback.
//!
//! Two consumers share one request path, and they want opposite things from a page:
//!
//! - **Terminal mode** refills native scrollback. It pages until the configured row cap is
//!   satisfied, then stops; the reader sees older content by scrolling the terminal itself.
//! - **Owned mode** serves a viewport. It pages when the *view* says it is near the top of what is
//!   loaded, or when find needs older entries to keep scanning. Nothing is written to the terminal.
//!
//! Both paths dedupe by turn id before prepending, so an overlapping page or an event that arrived
//! while the request was in flight cannot duplicate a turn. The owned path additionally hands the
//! splice range to the view, which re-resolves its reading anchor and resumes a parked search.

use super::*;
use crate::app_server_session::INITIAL_HISTORY_TURN_LIMIT;
use crate::transcript_history::TranscriptHistoryState;
use ody_app_server_protocol::ClientRequest;
use ody_app_server_protocol::SortDirection;
use ody_app_server_protocol::ThreadTurnsListParams;
use ody_app_server_protocol::ThreadTurnsListResponse;
use ody_app_server_protocol::TurnItemsView;

impl App {
    /// Start one bounded older-history page request for scrollback refill or the owned view.
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

    /// Ask the view whether it wants more history, and start one page if so.
    ///
    /// Owned mode only: in Terminal mode the viewport is the terminal, so paging is driven by the
    /// scrollback row budget instead (`finish_inline_history_page`).
    pub(super) fn request_owned_history(
        &mut self,
        tui: &mut tui::Tui,
        app_server: &mut AppServerSession,
    ) {
        if !tui.is_owned_screen() || !self.scrollback_has_older_history {
            return;
        }
        let Some(thread_id) = self.chat_widget.thread_id() else {
            return;
        };
        if !self.transcript_view.needs_history(&self.transcript_cells) {
            return;
        }
        if !app_server.has_older_history(thread_id) {
            // The server has nothing left; make sure the view stops showing a loading affordance.
            self.transcript_view
                .set_history_complete(/*complete*/ true);
            return;
        }
        if self.request_older_history_page(app_server, thread_id) {
            self.transcript_view.mark_history_loading();
        }
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
                if tui.is_owned_screen() {
                    // Surface the failure instead of silently stopping: the reader keeps their
                    // position, and find can retry from the same place.
                    self.transcript_view.mark_history_failed();
                    self.transcript_view.mark_search_page_failed();
                    tui.frame_requester().schedule_frame();
                }
                return Ok(());
            }
        };
        let older_turns = app_server.apply_older_turns_page(thread_id, cursor, page);
        if older_turns.is_empty() {
            // A page that contributed nothing still tells us whether more remain.
            self.scrollback_has_older_history = app_server.has_older_history(thread_id);
            if tui.is_owned_screen() {
                self.transcript_view
                    .set_history_complete(!self.scrollback_has_older_history);
                // No cells were spliced, but a parked scan still has to learn whether history is
                // exhausted; an empty splice range settles it without touching any anchor.
                self.transcript_view
                    .history_loaded(&self.transcript_cells, 0..0);
                self.finish_owned_history_page(tui, app_server, thread_id);
            }
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
        let count = cells.len();
        self.transcript_cells.splice(index..index, cells);
        if tui.is_owned_screen() {
            // Owned mode prepends into the view's source data only: the anchor is keyed by cell
            // identity, so the reading position survives, and no terminal scrollback is rebuilt.
            self.transcript_view
                .history_loaded(&self.transcript_cells, index..index + count);
            self.finish_owned_history_page(tui, app_server, thread_id);
            return Ok(());
        }
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

    /// Advance the owned view after a page: settle the state, finish a beginning jump, keep paging.
    fn finish_owned_history_page(
        &mut self,
        tui: &mut tui::Tui,
        app_server: &mut AppServerSession,
        thread_id: ThreadId,
    ) {
        let was_loading_beginning = self.transcript_view.is_loading_beginning();
        self.transcript_view
            .set_history_complete(!self.scrollback_has_older_history);
        if was_loading_beginning && !self.scrollback_has_older_history {
            // The start is now loaded; put it at the top, which is what the reader asked for.
            self.transcript_view
                .land_on_beginning(&self.transcript_cells);
        }
        // Chain pages while the view still wants them: a pending beginning jump, a parked search, or
        // a viewport still sitting at the boundary.
        if self.transcript_view.needs_history(&self.transcript_cells)
            && app_server.has_older_history(thread_id)
            && self.request_older_history_page(app_server, thread_id)
        {
            self.transcript_view.mark_history_loading();
        }
        tui.frame_requester().schedule_frame();
    }

    /// Reset owned paging state, e.g. on thread switch or `/clear`.
    pub(super) fn reset_owned_history_state(&mut self) {
        self.transcript_view.history = TranscriptHistoryState::Idle;
    }
}
