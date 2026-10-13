//! Carry reading anchors through transcript mutations.
//!
//! The app owns the cell list and splices it directly (`transcript_cells`), so the view must treat
//! every frame as potentially changed. Two mutations need help to stay honest:
//!
//! - the tail growing while the reader is away from the bottom, which drives the return-to-bottom
//!   hint and the unseen-activity label;
//! - a streamed run being replaced by a single consolidated cell, which would otherwise leave the
//!   reader anchored to a cell that no longer exists.
//!
//! Prepends need no special handling: anchors address content by cell identity, so inserting older
//! history above the window never moves the reader.

use std::ops::Range;

use super::*;

impl TranscriptView {
    /// Note whether new transcript entries appeared while the reader is away from the bottom.
    pub(super) fn sync_history_tail(&mut self, cells: &[Arc<dyn HistoryCell>]) {
        let tail = cells.last().map(EntryKey::cell);
        if !self.is_following()
            && self.last_tail.is_some()
            && self.last_tail != tail
            && let Some(previous) = cells
                .iter()
                .rposition(|cell| Some(EntryKey::cell(cell)) == self.last_tail)
        {
            self.unseen_activity |= (previous + 1..cells.len())
                .any(|index| self.layout(cells, index).is_some_and(|l| l.row_count() > 0));
        }
        self.last_tail = tail;
        if self.is_following() {
            self.unseen_activity = false;
        }
    }

    /// Move a reading anchor onto `replacement` when the cells it addressed were replaced.
    ///
    /// Call this with the cell list *before* the splice. The replacement is a different cell with
    /// different content (for example a consolidated markdown cell), so the anchor restarts at its
    /// first row instead of pretending the old offset still means the same thing.
    pub(crate) fn replace_range(
        &mut self,
        cells: &[Arc<dyn HistoryCell>],
        range: Range<usize>,
        replacement: &Arc<dyn HistoryCell>,
    ) {
        if range.is_empty() {
            self.cache.retain_live_sources();
            return;
        }
        let replacement_key = EntryKey::cell(replacement);
        if range.end == cells.len() && self.last_tail == cells.last().map(EntryKey::cell) {
            self.last_tail = Some(replacement_key);
        }
        let Position::Reading(anchor) = self.position else {
            self.cache.retain_live_sources();
            return;
        };
        let index = self.resolve(cells, anchor);
        if !range.contains(&index) {
            self.cache.retain_live_sources();
            return;
        }
        self.position = Position::Reading(Anchor {
            key: replacement_key,
            index: range.start,
            offset: 0,
            row_bias: 0,
        });
        self.cache.retain_live_sources();
    }
}
