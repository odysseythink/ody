//! A virtualized conversation viewport over the app's retained transcript cells.
//!
//! The app owns history, pagination and the live active cell. This view owns only the reading
//! position and the bounded layout cache for the rows it actually paints. The terminal scrollback
//! is never touched from here: Owned mode draws history, the live tail and the composer inside a
//! single Ratatui frame.
//!
//! Reading positions are anchored to *content*, not to screen rows: an anchor stores the identity
//! of the entry it lives in plus a byte offset inside that entry's source text. Prepending older
//! history therefore never renumbers an anchor, and a terminal width change lands the reader on the
//! same character instead of an unrelated screen row.

mod disclosure;
mod follow_control;
mod input;
mod layout;
mod mutations;
mod paging;
mod search;
mod selection;
mod text;

use std::ops::Range;
use std::sync::Arc;
use std::time::Instant;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Clear;
use ratatui::widgets::Widget;

use crate::chatwidget::ActiveCellTranscriptKey;
use crate::history_cell::HistoryCell;
use crate::history_cell::HistoryRenderMode;
use crate::terminal_hyperlinks::HyperlinkLine;
use crate::transcript_history::TranscriptHistoryState;

use layout::LayoutCache;
use search::SearchState;
use selection::Selection;
use text::TextLayout;

pub(crate) use input::ViewAction;

/// Identity of a transcript entry that survives index changes.
///
/// Committed cells are identified by the allocation address of the `Arc` the app already retains,
/// which is stable across prepends, splices and rebuilds. `Live` addresses the in-flight active
/// cell, which is always the last entry.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum EntryKey {
    Cell(usize),
    Live,
}

impl EntryKey {
    fn cell(cell: &Arc<dyn HistoryCell>) -> Self {
        Self::Cell(Arc::as_ptr(cell).cast::<()>() as usize)
    }
}

/// A reading position inside one entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Anchor {
    key: EntryKey,
    /// Last known index of the entry, used as a fallback when the key no longer resolves.
    index: usize,
    /// Byte offset of the anchored content inside the entry's source text.
    offset: usize,
    /// Row of the anchored content minus the top row of the viewport.
    ///
    /// Synthetic rows (separators) share source offsets with their neighbour, so the bias keeps
    /// the viewport from jumping by a row after a re-wrap.
    row_bias: isize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Position {
    #[default]
    Latest,
    Reading(Anchor),
}

pub(crate) struct TranscriptView {
    /// Rows carried per wheel notch, matching the pager overlay's `tui.mouse_scroll_speed`.
    pub(crate) mouse_scroll_speed: f64,
    pending_mouse_scroll: f64,
    position: Position,
    cache: LayoutCache,
    live: Option<Arc<TextLayout>>,
    /// Source lines of the live tail, retained so a re-layout can rebuild it at a new width.
    live_lines: Option<Vec<HyperlinkLine>>,
    live_separated: Option<Arc<TextLayout>>,
    live_key: Option<(u16, ActiveCellTranscriptKey)>,
    live_continuation: bool,
    area: Rect,
    /// `(entry index, row inside the entry)` for each screen row of the last frame.
    ///
    /// Recorded rather than recomputed so pointer hit tests and highlights agree with exactly what
    /// was painted, including the live tail and any entry that scrolled between frames.
    visible: Vec<(usize, usize)>,
    mode: HistoryRenderMode,
    tail_visible: bool,
    unseen_activity: bool,
    last_tail: Option<EntryKey>,
    selection: Option<Selection>,
    search: SearchState,
    /// Link captured by the last left-click press, opened only if the release is stationary.
    pressed_link: Option<String>,
    last_click: Option<(u16, u16, Instant)>,
    /// Transient feedback (`Copied 128 chars`) shown in the reserved status row.
    notice: Option<String>,
    /// How much of the thread's history is loaded. Owned by the app; the view reads it to decide
    /// whether to hold a reading position or ask for older pages.
    pub(crate) history: TranscriptHistoryState,
}

impl Default for TranscriptView {
    fn default() -> Self {
        Self {
            mouse_scroll_speed: 1.0,
            pending_mouse_scroll: 0.0,
            position: Position::Latest,
            cache: LayoutCache::default(),
            live: None,
            live_lines: None,
            live_separated: None,
            live_key: None,
            live_continuation: false,
            area: Rect::default(),
            visible: Vec::new(),
            mode: HistoryRenderMode::Rich,
            tail_visible: true,
            unseen_activity: false,
            last_tail: None,
            selection: None,
            search: SearchState::default(),
            pressed_link: None,
            last_click: None,
            notice: None,
            history: TranscriptHistoryState::Idle,
        }
    }
}

impl TranscriptView {
    pub(crate) fn new(mouse_scroll_speed: f64) -> Self {
        Self {
            mouse_scroll_speed,
            ..Self::default()
        }
    }

    /// Switch between rich and raw transcript presentation.
    ///
    /// Wrapped rows are cached per mode, so a mode change only needs to drop the reading position's
    /// resolved index; the cache entries simply stop matching.
    pub(crate) fn set_mode(&mut self, mode: HistoryRenderMode) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        self.cache.clear();
        self.live_key = None;
        self.live_separated = None;
    }

    pub(crate) fn is_following(&self) -> bool {
        self.position == Position::Latest
    }

    /// Whether a "return to bottom" affordance should be offered.
    pub(crate) fn needs_return_to_bottom(&self) -> bool {
        !self.is_following() && !self.tail_visible
    }

    /// Whether the tail grew while the reader was away from the bottom.
    pub(crate) fn has_unseen_activity(&self) -> bool {
        self.unseen_activity
    }

    #[cfg(test)]
    pub(super) fn cached_entry_count(&self) -> usize {
        self.cache.len()
    }

    /// Whether the last transcript row is currently inside the visible window.
    #[cfg(test)]
    pub(super) fn tail_visible(&self) -> bool {
        self.tail_visible
    }

    /// Drop all view state, e.g. after `/clear` or when switching threads.
    pub(crate) fn reset(&mut self) {
        self.position = Position::Latest;
        self.pending_mouse_scroll = 0.0;
        self.cache.clear();
        self.live = None;
        self.live_lines = None;
        self.live_separated = None;
        self.live_key = None;
        self.live_continuation = false;
        self.area = Rect::default();
        self.visible.clear();
        self.tail_visible = true;
        self.unseen_activity = false;
        self.last_tail = None;
        self.selection = None;
        self.search = SearchState::default();
        self.pressed_link = None;
        self.last_click = None;
        self.notice = None;
        self.history = TranscriptHistoryState::Idle;
    }

    /// Set the transient feedback shown in the reserved status row.
    ///
    /// Any later input clears it, so no timer is needed to retire the message.
    pub(crate) fn set_notice(&mut self, notice: impl Into<String>) {
        self.notice = Some(notice.into());
    }

    /// Refresh the cached live tail from the active cell.
    ///
    /// Returns whether the tail changed. `lines` is only called when the cache key (terminal width
    /// plus the active-cell revision/animation tick) changed, so an idle streaming cell is never
    /// re-formatted on every frame.
    pub(crate) fn sync_live_tail(
        &mut self,
        width: u16,
        key: Option<ActiveCellTranscriptKey>,
        lines: impl FnOnce(u16) -> Option<Vec<HyperlinkLine>>,
    ) -> bool {
        let next = key.map(|key| (width, key));
        if key.is_some() && self.live_key == next {
            return false;
        }
        let revision_changed = self.live_key.is_none()
            || self.live_key.map(|(_, key)| key.revision) != next.map(|(_, key)| key.revision);
        self.live_key = next;
        self.live_separated = None;
        self.live_continuation = key.is_some_and(|key| key.is_stream_continuation);

        let live_lines = lines(width);
        let live = live_lines
            .as_ref()
            .map(|lines| Arc::new(TextLayout::new(lines.clone(), width)));

        if self.live.as_ref().map(|layout| layout.text())
            != live.as_ref().map(|layout| layout.text())
        {
            if !self.is_following() && revision_changed && live.is_some() {
                self.unseen_activity = true;
            }
        }

        let changed = self.live.is_some() || live.is_some();
        self.live = live;
        self.live_lines = live_lines;
        changed
    }

    /// Paint the visible window of the transcript, plus a return-to-bottom hint when reading back.
    pub(crate) fn render(&mut self, area: Rect, buf: &mut Buffer, cells: &[Arc<dyn HistoryCell>]) {
        self.cache.begin_frame();
        self.cache.retain_live_sources();
        self.sync_history_tail(cells);

        // Frames are diffed against the previous buffer, so every cell this view owns must be
        // written even when it is unchanged. Clearing first also erases rows that scrolled away.
        Clear.render(area, buf);

        // The hint borrows the last row instead of overwriting transcript content, so the history
        // window shrinks by one row while the reader is away from the bottom or a status is shown.
        let status = self.status_row(area.width);
        let reserved = u16::from(
            area.height > 1
                && (status.is_some() || self.needs_return_to_bottom()),
        );
        let body = Rect {
            height: area.height.saturating_sub(reserved),
            ..area
        };
        self.area = body;

        if !body.is_empty() {
            self.visible.clear();
            let (mut index, mut row) = self.start(cells);
            let mut y = body.top();
            let mut last_visible: Option<(usize, usize)> = None;

            while y < body.bottom() {
                let Some(layout) = self.layout(cells, index) else {
                    break;
                };
                if row >= layout.row_count() {
                    index += 1;
                    row = 0;
                    // Empty entries are legal (hidden reasoning, empty notices).
                    let Some(next) = self.next_nonempty(cells, index) else {
                        break;
                    };
                    index = next;
                    continue;
                }
                layout.render(Rect::new(body.x, y, body.width, /*height*/ 1), buf, row);
                self.visible.push((index, row));
                last_visible = Some((index, row));
                y += 1;
                row += 1;
            }
            self.tail_visible = self.is_tail_row(cells, last_visible);
        } else {
            self.visible.clear();
            self.tail_visible = false;
        }

        // Interaction feedback paints over the rows it belongs to, after the content is in place.
        self.highlight_search(cells, buf);
        self.highlight_selection(cells, buf);

        if self.tail_visible {
            self.unseen_activity = false;
        }
        if reserved == 1 {
            let row = Rect {
                y: area.bottom().saturating_sub(/*rhs*/ 1),
                height: 1,
                ..area
            };
            match status {
                Some(label) => self.render_status_row(row, buf, &label),
                None => self.render_return_to_bottom(row, buf),
            }
        }
    }

    /// Label for the reserved row, if any.
    ///
    /// One row serves every transient affordance, so the order here decides what the reader sees:
    /// find status, then history paging state, then the last action's feedback, then the selection
    /// hint.
    fn status_row(&self, width: u16) -> Option<String> {
        if width == 0 {
            return None;
        }
        if let Some(status) = self.search_status() {
            return Some(status);
        }
        if let Some(status) = self.history_status() {
            return Some(status);
        }
        if let Some(notice) = &self.notice {
            return Some(format!(" {notice}"));
        }
        if self.has_selection() {
            return Some(" selection · enter copies · esc clears".to_string());
        }
        None
    }

    /// Reserved-row label for a page load in flight or a failed one.
    ///
    /// Only shown while the reader is actually looking at the boundary, so a background page never
    /// takes the row away from the transcript. Failures always surface, because nothing else will
    /// tell the reader that history stopped arriving.
    fn history_status(&self) -> Option<String> {
        match self.history {
            TranscriptHistoryState::LoadingOlder if !self.tail_visible => {
                Some(" loading earlier messages…".to_string())
            }
            TranscriptHistoryState::Failed => {
                Some(" earlier messages failed to load · home retries".to_string())
            }
            _ => None,
        }
    }

    /// Scroll by `rows` (negative scrolls toward older content).
    pub(crate) fn scroll(&mut self, cells: &[Arc<dyn HistoryCell>], rows: isize) {
        if rows == 0 || self.area.is_empty() {
            return;
        }
        // Deliberate navigation supersedes a pending jump-to-beginning, but keeps the page loads
        // that jump already started.
        self.cancel_beginning();
        let start = self.start(cells);
        let moved = self.move_rows(cells, start.0, start.1, rows);
        if moved == start {
            return;
        }
        if rows > 0 {
            let bottom = self.bottom_start(cells);
            if moved >= bottom {
                self.jump_to_latest();
                return;
            }
        }
        let Some(layout) = self.layout(cells, moved.0) else {
            return;
        };
        let offset = layout.position_at(moved.1, /*column*/ 0);
        self.position = Position::Reading(Anchor {
            key: self.entry_key(cells, moved.0),
            index: moved.0,
            offset,
            row_bias: layout.row_for_offset(offset) as isize - moved.1 as isize,
        });
        if rows < 0 {
            self.tail_visible = false;
        }
    }

    /// Apply one wheel notch; `direction` is negative to scroll up.
    pub(crate) fn scroll_by_wheel(
        &mut self,
        cells: &[Arc<dyn HistoryCell>],
        direction: f64,
    ) -> bool {
        if self.pending_mouse_scroll.signum() != direction.signum() {
            self.pending_mouse_scroll = 0.0;
        }
        self.pending_mouse_scroll += direction * self.mouse_scroll_speed;
        // Decimal speeds can land just short of a whole row after repeated addition.
        let rounded = self.pending_mouse_scroll.round();
        if rounded != 0.0 && (self.pending_mouse_scroll - rounded).abs() < 1e-9 {
            self.pending_mouse_scroll = rounded;
        }
        let rows = self.pending_mouse_scroll as isize;
        self.pending_mouse_scroll = self.pending_mouse_scroll.fract();
        if rows == 0 {
            return false;
        }
        self.scroll(cells, rows);
        true
    }

    pub(crate) fn jump_to_latest(&mut self) {
        self.position = Position::Latest;
        self.tail_visible = true;
        self.unseen_activity = false;
        self.pending_mouse_scroll = 0.0;
    }

    /// Move the window to the oldest loaded entry, aligned with its first row.
    ///
    /// Unlike scrolling up repeatedly, this leaves the first row of the oldest entry at the top
    /// instead of keeping the entry's separator in view, which is what "jump to the beginning"
    /// should show.
    ///
    /// When older pages still exist this only *aims* at the beginning: the position is held and
    /// `history` records that the reader is waiting, so the app chains pages and
    /// `land_on_beginning` finishes the jump once nothing older remains. Landing on the oldest
    /// *loaded* row immediately would look like the request was ignored — the top would keep moving
    /// away as each page arrives.
    pub(crate) fn jump_to_beginning(&mut self, cells: &[Arc<dyn HistoryCell>]) {
        if self.history == TranscriptHistoryState::LoadingBeginning {
            // A previous page is still in flight; let the chain finish rather than restarting it.
            return;
        }
        if self.history.has_unloaded_history() {
            self.hold_for_beginning(cells);
            return;
        }
        self.land_on_beginning(cells);
    }

    /// Start or continue a beginning jump, keeping the window where it is until pages arrive.
    ///
    /// The window is frozen into a content anchor first: while history is unloaded the position
    /// would otherwise stay `Latest` and re-derive a new window from the newest page on every
    /// `history_loaded`, which is exactly the crawl this state exists to avoid. Pinning the top row
    /// means each prepend shifts the anchor down by identity and the reader keeps looking at the
    /// same rows.
    fn hold_for_beginning(&mut self, cells: &[Arc<dyn HistoryCell>]) {
        let (index, row) = self.start(cells);
        if let Some(layout) = self.layout(cells, index) {
            let offset = layout.position_at(row, /*column*/ 0);
            self.position = Position::Reading(Anchor {
                key: self.entry_key(cells, index),
                index,
                offset,
                row_bias: layout.row_for_offset(offset) as isize - row as isize,
            });
        }
        // Only a reader who was already away from the bottom needs the hint back; someone who
        // jumped up from the tail should not be told to return to a bottom that is one screen down.
        self.tail_visible = self.is_tail_row(cells, self.visible.last().copied());
        self.history = TranscriptHistoryState::LoadingBeginning;
        self.unseen_activity = false;
        self.pending_mouse_scroll = 0.0;
    }

    /// Rows moved by one page-up/page-down step.
    ///
    /// One row of the previous page is kept on screen so the reader can see where the jump landed.
    /// The viewport is only known after the first render, so this falls back to a single row.
    pub(crate) fn page_rows(&self) -> isize {
        let rows = self.area.height.saturating_sub(/*rhs*/ 1);
        (rows as isize).max(/*default*/ 1)
    }

    fn start(&mut self, cells: &[Arc<dyn HistoryCell>]) -> (usize, usize) {
        match self.position {
            Position::Latest => self.bottom_start(cells),
            Position::Reading(anchor) => {
                let index = self.resolve(cells, anchor);
                let row = self.layout(cells, index).map_or(/*default*/ 0, |layout| {
                    layout
                        .row_for_offset(anchor.offset)
                        .saturating_add_signed(anchor.row_bias.saturating_neg())
                        .min(layout.row_count().saturating_sub(/*rhs*/ 1))
                });
                self.position = Position::Reading(Anchor {
                    key: self.entry_key(cells, index),
                    index,
                    ..anchor
                });
                (index, row)
            }
        }
    }

    /// First row of the window that ends at the bottom of the transcript.
    fn bottom_start(&mut self, cells: &[Arc<dyn HistoryCell>]) -> (usize, usize) {
        let count = cells.len() + usize::from(self.live.is_some());
        let Some(last) = count.checked_sub(/*rhs*/ 1) else {
            return (0, 0);
        };
        let height = self
            .layout(cells, last)
            .map_or(/*default*/ 0, |layout| layout.row_count());
        self.move_rows(cells, last, height, -(self.area.height as isize))
    }

    fn move_rows(
        &mut self,
        cells: &[Arc<dyn HistoryCell>],
        mut index: usize,
        mut row: usize,
        rows: isize,
    ) -> (usize, usize) {
        if rows < 0 {
            let mut remaining = rows.unsigned_abs();
            while remaining > row && index > 0 {
                remaining -= row;
                index -= 1;
                row = self
                    .layout(cells, index)
                    .map_or(/*default*/ 0, |layout| layout.row_count());
            }
            if index == 0
                && remaining >= row
                && let Some(first) = self.next_nonempty(cells, index)
            {
                return (first, usize::from(self.layout_separated(cells, first)));
            }
            return (index, row.saturating_sub(remaining));
        }
        let mut remaining = row.saturating_add(rows as usize);
        while let Some(layout) = self.layout(cells, index) {
            if remaining < layout.row_count() {
                break;
            }
            remaining -= layout.row_count();
            index += 1;
        }
        (index, remaining)
    }

    fn layout_separated(&mut self, cells: &[Arc<dyn HistoryCell>], index: usize) -> bool {
        self.layout(cells, index)
            .is_some_and(|layout| layout.separated)
    }

    fn next_nonempty(&mut self, cells: &[Arc<dyn HistoryCell>], mut index: usize) -> Option<usize> {
        while let Some(layout) = self.layout(cells, index) {
            if layout.row_count() > 0 {
                return Some(index);
            }
            index += 1;
        }
        None
    }

    fn is_tail_row(
        &mut self,
        cells: &[Arc<dyn HistoryCell>],
        last_visible: Option<(usize, usize)>,
    ) -> bool {
        let Some((index, row)) = last_visible else {
            return false;
        };
        let Some(final_index) = self.last_nonempty(cells) else {
            return false;
        };
        if index != final_index {
            return false;
        }
        self.layout(cells, index)
            .is_some_and(|layout| row + 1 >= layout.row_count())
    }

    fn last_nonempty(&mut self, cells: &[Arc<dyn HistoryCell>]) -> Option<usize> {
        let count = cells.len() + usize::from(self.live.is_some());
        for index in (0..count).rev() {
            if self
                .layout(cells, index)
                .is_some_and(|layout| layout.row_count() > 0)
            {
                return Some(index);
            }
        }
        None
    }

    fn resolve(&mut self, cells: &[Arc<dyn HistoryCell>], anchor: Anchor) -> usize {
        if anchor.key == EntryKey::Live {
            return cells.len();
        }
        if cells
            .get(anchor.index)
            .is_some_and(|cell| EntryKey::cell(cell) == anchor.key)
        {
            return anchor.index;
        }
        cells
            .iter()
            .position(|cell| EntryKey::cell(cell) == anchor.key)
            .unwrap_or_else(|| anchor.index.min(cells.len().saturating_sub(/*rhs*/ 1)))
    }

    fn entry_key(&self, cells: &[Arc<dyn HistoryCell>], index: usize) -> EntryKey {
        cells.get(index).map_or(EntryKey::Live, EntryKey::cell)
    }
}

#[cfg(test)]
mod tests;
