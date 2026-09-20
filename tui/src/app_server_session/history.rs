//! Bounded app-server transcript loading for resume.

use std::collections::HashMap;
use std::collections::HashSet;

use color_eyre::eyre::Result;
use color_eyre::eyre::WrapErr as _;
use ody_app_server_protocol::ClientRequest;
use ody_app_server_protocol::SortDirection;
use ody_app_server_protocol::ThreadTurnsListParams;
use ody_app_server_protocol::ThreadTurnsListResponse;
use ody_app_server_protocol::Turn;
use ody_app_server_protocol::TurnItemsView;
use ody_app_server_protocol::TurnsPage;
use ody_protocol::ThreadId;

use crate::history_cell::HistoryCell;
use crate::history_cell::HistoryRenderMode;
use crate::thread_transcript::RawReasoningVisibility;
use crate::thread_transcript::turns_to_transcript_cells;

/// Number of turns fetched per `thread/turns/list` page during resume.
pub(crate) const INITIAL_HISTORY_TURN_LIMIT: u32 = 5;

/// Pagination state for a thread resumed with bounded history.
///
/// A missing entry means "full history already loaded" (legacy server
/// response or a thread that predates pagination), so `has_older_history`
/// stays false and top-up never fires.
#[derive(Clone, Debug, Default)]
pub(crate) struct ThreadHistoryPagination {
    pub(super) next_turn_cursor: Option<String>,
    pub(super) seen_turn_cursors: HashSet<String>,
    pub(super) loading_older: bool,
}

impl ThreadHistoryPagination {
    pub(super) fn has_older(&self) -> bool {
        self.next_turn_cursor.is_some()
    }
}

/// Seed pagination state from a resume response's initial turns page.
///
/// `None` means the server ignored the experimental pagination params
/// (older app server) and returned full turns in `thread.turns`; leave the
/// map without an entry so the thread behaves exactly like today's
/// full-history resume.
pub(super) fn seed_history_pagination(
    pagination: &mut HashMap<ThreadId, ThreadHistoryPagination>,
    thread_id: ThreadId,
    initial_page: Option<&TurnsPage>,
) {
    match initial_page {
        Some(page) => {
            pagination.insert(
                thread_id,
                ThreadHistoryPagination {
                    next_turn_cursor: page.next_cursor.clone(),
                    ..ThreadHistoryPagination::default()
                },
            );
        }
        None => {
            pagination.remove(&thread_id);
        }
    }
}

/// Safety cap on total turns scanned during hydration, bounding requests even
/// when rows undercount (e.g. turns whose items all render to zero height).
pub(crate) const HISTORY_TURN_SCAN_LIMIT: usize = 100;

/// Advance to `next` unless it was already seen (server cursor loop guard).
pub(super) fn advancing_cursor(
    current: Option<String>,
    next: Option<String>,
    seen_cursors: &mut HashSet<String>,
) -> Option<String> {
    if let Some(current) = current {
        seen_cursors.insert(current);
    }
    next.filter(|next| seen_cursors.insert(next.clone()))
}

/// Cumulative viewport rows for rendering `turns`, mirroring the terminal
/// scrollback accounting used by resize reflow.
pub(crate) fn rendered_turns_rows(
    cwd: &std::path::Path,
    turns: &[Turn],
    visibility: RawReasoningVisibility,
    mode: HistoryRenderMode,
    width: u16,
) -> usize {
    turns_to_transcript_cells(cwd, turns, visibility)
        .iter()
        .fold(0usize, |rows, cell| {
            let height = usize::from(cell.desired_height_for_mode(width, mode));
            rows + height + usize::from(height != 0 && rows != 0 && !cell.is_stream_continuation())
        })
}

impl super::AppServerSession {
    pub(crate) async fn thread_turns_page(
        &mut self,
        thread_id: ThreadId,
        cursor: Option<String>,
    ) -> Result<ThreadTurnsListResponse> {
        let request_id = self.next_request_id();
        self.client
            .request_typed(ClientRequest::ThreadTurnsList {
                request_id,
                params: ThreadTurnsListParams {
                    thread_id: thread_id.to_string(),
                    cursor,
                    limit: Some(INITIAL_HISTORY_TURN_LIMIT),
                    sort_direction: Some(SortDirection::Desc),
                    items_view: Some(TurnItemsView::Full),
                },
            })
            .await
            .wrap_err("failed to load a bounded thread history page")
    }

    /// Fetch older turn pages until the terminal row budget is filled.
    ///
    /// `turns` already holds the initial page in chronological order; older
    /// pages (descending) are prepended in front. With `row_budget == None`
    /// (no `terminal_resize_reflow` cap) every older page is fetched, which
    /// degrades gracefully to the pre-pagination full-history resume.
    pub(crate) async fn hydrate_initial_turns_window(
        &mut self,
        thread_id: ThreadId,
        turns: &mut Vec<Turn>,
        cwd: &std::path::Path,
        visibility: RawReasoningVisibility,
        render_mode: HistoryRenderMode,
        width: u16,
        row_budget: Option<usize>,
    ) -> Result<()> {
        if row_budget.is_none() {
            while let Some(cursor) = self.take_next_turn_cursor(thread_id) {
                let page = self.thread_turns_page(thread_id, Some(cursor)).await?;
                self.record_turn_cursor_advance(thread_id, page.next_cursor);
                turns.splice(0..0, page.data.into_iter().rev());
            }
            return Ok(());
        }
        let budget = row_budget.expect("checked above");
        let mut rendered_rows = rendered_turns_rows(cwd, turns, visibility, render_mode, width);
        let mut scanned_turns = turns.len();
        while rendered_rows < budget
            && scanned_turns < HISTORY_TURN_SCAN_LIMIT
            && self.has_older_history(thread_id)
        {
            let Some(cursor) = self.take_next_turn_cursor(thread_id) else {
                break;
            };
            let page = self.thread_turns_page(thread_id, Some(cursor)).await?;
            self.record_turn_cursor_advance(thread_id, page.next_cursor);
            if page.data.is_empty() {
                break;
            }
            scanned_turns = scanned_turns.saturating_add(page.data.len());
            let older: Vec<Turn> = page.data.into_iter().rev().collect();
            rendered_rows = rendered_rows.saturating_add(rendered_turns_rows(
                cwd, &older, visibility, render_mode, width,
            ));
            turns.splice(0..0, older);
        }
        Ok(())
    }

    fn take_next_turn_cursor(&mut self, thread_id: ThreadId) -> Option<String> {
        self.history_pagination
            .get_mut(&thread_id)?
            .next_turn_cursor
            .take()
    }

    fn record_turn_cursor_advance(&mut self, thread_id: ThreadId, next: Option<String>) {
        if let Some(state) = self.history_pagination.get_mut(&thread_id) {
            let current = state.next_turn_cursor.take();
            state.next_turn_cursor = advancing_cursor(current, next, &mut state.seen_turn_cursors);
        }
    }

    /// Begin one older-history page request; returns the cursor to fetch.
    pub(crate) fn begin_older_history_page(&mut self, thread_id: ThreadId) -> Option<String> {
        let page = self.history_pagination.get_mut(&thread_id)?;
        if page.loading_older {
            return None;
        }
        let cursor = page.next_turn_cursor.clone()?;
        page.loading_older = true;
        Some(cursor)
    }

    pub(crate) fn cancel_older_history_page(&mut self, thread_id: ThreadId) {
        if let Some(page) = self.history_pagination.get_mut(&thread_id) {
            page.loading_older = false;
        }
    }

    /// Validate a page response against the in-flight cursor and return the
    /// older turns (chronological order) ready to prepend. Empty vec means
    /// ignore (stale response or no older turns).
    pub(crate) fn apply_older_turns_page(
        &mut self,
        thread_id: ThreadId,
        cursor: &str,
        page: impl Into<TurnsPage>,
    ) -> Vec<Turn> {
        let Some(state) = self.history_pagination.get_mut(&thread_id) else {
            return Vec::new();
        };
        if !state.loading_older || state.next_turn_cursor.as_deref() != Some(cursor) {
            return Vec::new();
        }
        let page = page.into();
        state.loading_older = false;
        state.next_turn_cursor = advancing_cursor(
            state.next_turn_cursor.take(),
            page.next_cursor,
            &mut state.seen_turn_cursors,
        );
        page.data.into_iter().rev().collect()
    }
}

#[cfg(test)]
pub(crate) mod history_test_support {
    use super::*;

    pub(crate) fn pagination_with_cursor(cursor: &str) -> ThreadHistoryPagination {
        ThreadHistoryPagination {
            next_turn_cursor: Some(cursor.to_string()),
            ..ThreadHistoryPagination::default()
        }
    }
}

#[cfg(test)]
#[path = "history_tests.rs"]
mod tests;
