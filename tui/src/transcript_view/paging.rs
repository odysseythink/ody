//! Viewport-driven history paging for the owned transcript.
//!
//! The app owns the request; this module only answers two questions and repairs its own state when
//! a page lands: *does the reader need older history?* and *what does the reading position mean
//! while a page is in flight?* Both are answered from content anchors, so a prepend never renumbers
//! the anchor the reader is standing on — the same property that makes ordinary scrolling survive
//! older pages.
//!
//! Two triggers exist, mirroring upstream:
//!
//! - **Viewport proximity.** The window has reached, or is within one screen of, the oldest loaded
//!   row. Scrolling is the common case; a viewport that fits entirely inside the loaded history
//!   (a tall terminal, a short session) also qualifies, which is why the first page can follow
//!   immediately after resume without the reader doing anything.
//! - **Search demand.** Find has walked to the oldest loaded entry and wants to keep going. The
//!   search module records that in `Progress::AwaitingHistory`; the reader is not scrolling, so
//!   proximity alone would never fire.
//!
//! `LoadingBeginning` is the one state with extra semantics: it means "the reader asked for the
//! very start of the thread". The view then refuses to resolve the position to the newest-loaded
//! row after each page, so pages chain until history is exhausted and only then lands on row zero.

use super::*;

impl TranscriptView {
    /// Whether the window is at, or within one screen of, the oldest loaded content.
    ///
    /// Uses the *resolved* start, so an anchor pointing at a row that a prepend has pushed down
    /// still measures its real distance to the top.
    pub(crate) fn near_start(&mut self, cells: &[Arc<dyn HistoryCell>]) -> bool {
        if self.area.is_empty() {
            // Before the first frame there is no window to be near anything; resume decides the
            // initial budget instead, so do not page a second time on top of it.
            return false;
        }
        if !self.history.has_unloaded_history() {
            return false;
        }
        let (index, row) = self.start(cells);
        let threshold = usize::from(self.area.height);
        let mut distance = row;
        if distance > threshold {
            return false;
        }
        for previous in (0..index).rev() {
            distance = distance.saturating_add(
                self.layout(cells, previous)
                    .map_or(/*default*/ 0, |layout| layout.row_count()),
            );
            if distance > threshold {
                return false;
            }
        }
        true
    }

    /// Whether older history should be requested for the current window.
    ///
    /// A pending beginning jump always qualifies: the reader asked for the start, so pages chain
    /// until it is reached even though the window is far from the top of what is loaded. Find
    /// counts while parked, because a query that has scanned every loaded entry has nothing left to
    /// do until an older page arrives.
    pub(crate) fn needs_history(&mut self, cells: &[Arc<dyn HistoryCell>]) -> bool {
        if !self.history.has_unloaded_history() {
            return false;
        }
        if self.history == TranscriptHistoryState::LoadingBeginning {
            return true;
        }
        if self.search_awaiting_history() {
            return true;
        }
        self.near_start(cells)
    }

    /// Record that a page request is in flight.
    ///
    /// A beginning jump keeps its own state so page chaining and the held position both survive.
    pub(crate) fn mark_history_loading(&mut self) {
        if self.history != TranscriptHistoryState::LoadingBeginning {
            self.history = TranscriptHistoryState::LoadingOlder;
        }
    }

    /// Record that the most recent page request failed. Retryable.
    pub(crate) fn mark_history_failed(&mut self) {
        self.history = TranscriptHistoryState::Failed;
    }

    /// Update the loaded-history state from the server's cursor.
    pub(crate) fn set_history_complete(&mut self, complete: bool) {
        self.history = if complete {
            TranscriptHistoryState::Complete
        } else if self.history == TranscriptHistoryState::LoadingBeginning {
            // Still chaining toward the start: keep holding the position.
            TranscriptHistoryState::LoadingBeginning
        } else {
            TranscriptHistoryState::Partial
        };
    }

    /// Whether the reader is waiting to be taken all the way to the start of the thread.
    pub(crate) fn is_loading_beginning(&self) -> bool {
        self.history == TranscriptHistoryState::LoadingBeginning
    }

    /// Whether a beginning jump is still waiting for pages.
    ///
    /// Distinct from `is_loading_beginning`: this is the "should another page be requested?" half,
    /// and it goes quiet once the jump is abandoned by navigation.
    pub(crate) fn beginning_needs_more(&self) -> bool {
        self.history == TranscriptHistoryState::LoadingBeginning
    }

    /// Abandon a beginning jump, e.g. because the reader navigated away.
    pub(crate) fn cancel_beginning(&mut self) {
        if self.history == TranscriptHistoryState::LoadingBeginning {
            // Older pages may still be useful; keep loading, just stop holding the position.
            self.history = TranscriptHistoryState::LoadingOlder;
        }
    }

    /// Accept an older page that has just been prepended into `cells`.
    ///
    /// `inserted` is the splice range inside `cells`. Anchors resolve by identity, so the only work
    /// needed is to re-resolve the reading position to its new index and let find resume scanning
    /// from the page it just received.
    pub(crate) fn history_loaded(&mut self, cells: &[Arc<dyn HistoryCell>], inserted: Range<usize>) {
        if let Position::Reading(anchor) = self.position {
            let index = self.resolve(cells, anchor);
            self.position = Position::Reading(Anchor {
                key: self.entry_key(cells, index),
                index,
                ..anchor
            });
        }
        self.search_history_loaded(cells, inserted);
    }

    /// Land on the oldest loaded row after a beginning jump that has finished.
    ///
    /// The app calls this once no older page remains, so the first row of the thread is actually at
    /// the top instead of the first row that happened to be loaded when the jump started.
    pub(crate) fn land_on_beginning(&mut self, cells: &[Arc<dyn HistoryCell>]) {
        let Some(first) = self.next_nonempty(cells, 0) else {
            return;
        };
        let offset = self
            .layout(cells, first)
            .map_or(/*default*/ 0, |layout| layout.position_at(0, 0));
        self.position = Position::Reading(Anchor {
            key: self.entry_key(cells, first),
            index: first,
            offset,
            row_bias: 0,
        });
        // Landing at the start never touches `tail_visible`: a reader who was already away from the
        // bottom keeps the return-to-bottom hint, and one who jumped up from the tail does not gain
        // it just because the thread beginning finally arrived.
        self.unseen_activity = false;
        self.pending_mouse_scroll = 0.0;
    }

    /// Whether a page is worth requesting for this thread right now.
    ///
    /// Trivial predicate kept here so the app can express "should I ask?" without duplicating the
    /// state rules.
    pub(crate) fn wants_older_history(&self) -> bool {
        self.history.has_unloaded_history() && self.history != TranscriptHistoryState::Failed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view_with_area(height: u16) -> TranscriptView {
        let mut view = TranscriptView::default();
        view.area = Rect::new(0, 0, 80, height);
        view
    }

    #[test]
    fn near_start_is_false_without_a_rendered_window() {
        let mut view = TranscriptView::default();
        view.history = TranscriptHistoryState::Partial;
        assert!(!view.near_start(&[]));
    }

    #[test]
    fn near_start_is_false_once_history_is_complete() {
        let mut view = view_with_area(24);
        view.history = TranscriptHistoryState::Complete;
        assert!(!view.near_start(&[]));
        assert!(!view.needs_history(&[]));
    }

    #[test]
    fn loading_beginning_survives_a_completed_page() {
        let mut view = view_with_area(10);
        view.history = TranscriptHistoryState::LoadingBeginning;
        assert!(view.beginning_needs_more());
        // Still older pages available: keep holding.
        view.set_history_complete(false);
        assert!(view.is_loading_beginning());
        assert!(view.beginning_needs_more());
        // Exhausted: the state becomes terminal.
        view.set_history_complete(true);
        assert!(!view.beginning_needs_more());
        assert_eq!(view.history, TranscriptHistoryState::Complete);
    }

    #[test]
    fn a_failed_page_is_not_requested_again_by_the_automated_path() {
        let mut view = view_with_area(10);
        view.history = TranscriptHistoryState::Failed;
        assert!(view.history.has_unloaded_history());
        // The reader can still retry explicitly, but nothing re-requests on its own.
        assert!(!view.wants_older_history());
    }

    #[test]
    fn loading_a_page_keeps_a_beginning_jump_marked() {
        let mut view = view_with_area(10);
        view.history = TranscriptHistoryState::LoadingBeginning;
        view.mark_history_loading();
        assert!(view.is_loading_beginning());

        view.history = TranscriptHistoryState::Partial;
        view.mark_history_loading();
        assert_eq!(view.history, TranscriptHistoryState::LoadingOlder);
    }
}
