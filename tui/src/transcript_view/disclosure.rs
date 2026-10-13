//! Disclosure handling for the owned transcript.
//!
//! History cells already own their expanded/collapsed state (`is_collapsible`/`is_expanded`/
//! `toggle_expanded`), and the collapsed form ends in a hint row such as
//! `"... (12 more lines, alt+o to expand)"`. The keyboard path (`alt+o`) is handled by the app
//! event loop; this module adds the pointer path, which needs the viewport geometry and therefore
//! lives here.
//!
//! Toggling goes through the cell itself, so the layout cache notices because `CellPresentation`
//! includes the expansion state: the next frame re-wraps that entry and nothing else.

use super::*;

/// Marker the collapsed form of a history cell ends with.
///
/// Matching the rendered hint instead of a per-cell API keeps this working for every cell type that
/// truncates through `history_cell::preview` without adding a new trait method.
const DISCLOSURE_HINT_MARKER: &str = " to expand)";

impl TranscriptView {
    /// Toggle the disclosure row under a screen position.
    ///
    /// Returns whether a cell was toggled, so the caller can request a frame.
    pub(super) fn toggle_disclosure_at(
        &mut self,
        cells: &[Arc<dyn HistoryCell>],
        _column: u16,
        row: u16,
    ) -> bool {
        let Some((index, row_in_entry)) = self.visible_row(row) else {
            return false;
        };
        let Some(cell) = cells.get(index) else {
            return false;
        };
        if !cell.is_collapsible() {
            return false;
        }
        if !self.row_is_disclosure(cells, index, row_in_entry) {
            return false;
        }
        cell.toggle_expanded();
        true
    }

    /// Whether one display row of an entry belongs to the collapsed-form hint.
    ///
    /// The hint can wrap, so the marker is located in the entry text first and then translated to
    /// the rows it covers rather than compared against a single row.
    fn row_is_disclosure(
        &mut self,
        cells: &[Arc<dyn HistoryCell>],
        index: usize,
        row: usize,
    ) -> bool {
        let Some(layout) = self.layout(cells, index) else {
            return false;
        };
        let Some(start) = layout.text().find(DISCLOSURE_HINT_MARKER) else {
            return false;
        };
        let end = start + DISCLOSURE_HINT_MARKER.len();
        let first = layout.row_for_offset(start);
        let last = layout.row_for_offset(end.saturating_sub(/*rhs*/ 1));
        (first..=last).contains(&row)
    }
}
