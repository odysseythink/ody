//! Text selection over the owned transcript viewport.
//!
//! A selection is anchored to content exactly like the reading position: each end is a stable entry
//! identity plus a byte offset into that entry's source text. Scrolling, prepending older history
//! and terminal resizes therefore all keep the selection on the text the pointer dragged over,
//! while the highlight is recomputed from the current wrap on every frame.
//!
//! The overlap rules match the pager overlay, which is the established Ody behavior: a drag selects
//! character-precisely, a stationary click leaves no zero-width selection behind, and the extracted
//! text is the visible rows joined with newlines and trimmed at the ends.

use super::*;
use ratatui::layout::Position as ScreenPosition;
use ratatui::style::Modifier;
use ratatui::style::Style;

/// One end of a selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct SelectionPoint {
    key: EntryKey,
    /// Last known entry index, used when the key no longer resolves.
    index: usize,
    /// Byte offset inside the entry's source text.
    offset: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Selection {
    anchor: SelectionPoint,
    head: SelectionPoint,
    /// Whether the pointer is still held down; drags extend the selection while this is set.
    dragging: bool,
    /// Whether the head ever moved off the anchor. A stationary click selects nothing.
    moved: bool,
}

/// Reverse video, matching the pager overlay's selection highlight.
pub(super) fn selection_style() -> Style {
    Style::default().add_modifier(Modifier::REVERSED)
}

impl TranscriptView {
    pub(crate) fn has_selection(&self) -> bool {
        self.selection.is_some()
    }

    pub(super) fn is_selecting(&self) -> bool {
        self.selection.is_some_and(|selection| selection.dragging)
    }

    /// Clear the active selection; returns whether there was one.
    pub(crate) fn clear_selection(&mut self) -> bool {
        self.selection.take().is_some()
    }

    /// Resolve a screen position to an entry row.
    ///
    /// `visible` is recorded by the last frame, so this is exact for whatever the reader is
    /// actually looking at rather than re-deriving the window.
    pub(super) fn visible_row(&self, row: u16) -> Option<(usize, usize)> {
        let offset = usize::from(row.checked_sub(self.area.y)?);
        self.visible.get(offset).copied()
    }

    fn point_at(
        &mut self,
        cells: &[Arc<dyn HistoryCell>],
        column: u16,
        row: u16,
    ) -> Option<SelectionPoint> {
        let (index, row_in_entry) = self.visible_row(row)?;
        let layout = self.layout(cells, index)?;
        let offset = layout.position_at(row_in_entry, column.saturating_sub(self.area.x));
        Some(SelectionPoint {
            key: self.entry_key(cells, index),
            index,
            offset,
        })
    }

    /// Resolve a screen position to a selection point, clamping outside positions to the viewport.
    ///
    /// Dragging above or below the window keeps extending the selection instead of stalling at the
    /// edge, which is what makes "drag to the top" reach the first visible row.
    fn point_at_clamped(
        &mut self,
        cells: &[Arc<dyn HistoryCell>],
        column: u16,
        row: u16,
    ) -> Option<SelectionPoint> {
        let area = self.area;
        if area.is_empty() {
            return None;
        }
        let column = column.clamp(area.x, area.right().saturating_sub(/*rhs*/ 1));
        let row = row.clamp(area.y, area.bottom().saturating_sub(/*rhs*/ 1));
        self.point_at(cells, column, row)
    }

    /// Start a selection at a screen position.
    ///
    /// `clicks >= 2` selects the whole display row under the pointer; a single click starts a drag.
    pub(super) fn begin_selection(
        &mut self,
        cells: &[Arc<dyn HistoryCell>],
        column: u16,
        row: u16,
        clicks: u8,
    ) -> bool {
        if clicks >= 2
            && let Some((anchor, head)) = self.row_bounds_at(cells, column, row)
        {
            self.selection = Some(Selection {
                anchor,
                head,
                dragging: false,
                moved: true,
            });
            return true;
        }
        let Some(point) = self.point_at(cells, column, row) else {
            return false;
        };
        self.selection = Some(Selection {
            anchor: point,
            head: point,
            dragging: true,
            moved: false,
        });
        true
    }

    /// Display-row bounds of the row under a screen position.
    fn row_bounds_at(
        &mut self,
        cells: &[Arc<dyn HistoryCell>],
        _column: u16,
        row: u16,
    ) -> Option<(SelectionPoint, SelectionPoint)> {
        let (index, row_in_entry) = self.visible_row(row)?;
        let layout = self.layout(cells, index)?;
        let source = layout.row_source(row_in_entry)?;
        if source.is_empty() {
            return None;
        }
        let key = self.entry_key(cells, index);
        Some((
            SelectionPoint {
                key,
                index,
                offset: source.start,
            },
            SelectionPoint {
                key,
                index,
                offset: source.end,
            },
        ))
    }

    /// Move the head of the active selection.
    pub(super) fn extend_selection(
        &mut self,
        cells: &[Arc<dyn HistoryCell>],
        column: u16,
        row: u16,
    ) -> bool {
        let Some(mut selection) = self.selection else {
            return false;
        };
        let Some(point) = self.point_at_clamped(cells, column, row) else {
            return false;
        };
        if point != selection.anchor {
            selection.moved = true;
        }
        selection.head = point;
        self.selection = Some(selection);
        true
    }

    /// Move the head of the active selection by whole rows, keeping the column.
    pub(super) fn extend_selection_rows(
        &mut self,
        cells: &[Arc<dyn HistoryCell>],
        rows: isize,
    ) -> bool {
        let Some(mut selection) = self.selection else {
            return false;
        };
        let (mut index, _) = self.resolve_point(cells, selection.head);
        let row = self
            .layout(cells, index)
            .map_or(/*default*/ 0, |layout| layout.row_for_offset(selection.head.offset));
        let target = row as isize + rows;
        if target < 0 {
            return false;
        }
        let last_index = cells.len() + usize::from(self.live.is_some());
        let mut row = target as usize;
        loop {
            let Some(layout) = self.layout(cells, index) else {
                return false;
            };
            if row < layout.row_count() {
                break;
            }
            if index + 1 >= last_index {
                return false;
            }
            row -= layout.row_count();
            index += 1;
        }
        let Some(layout) = self.layout(cells, index) else {
            return false;
        };
        let offset = layout.position_at(row, u16::MAX);
        let point = SelectionPoint {
            key: self.entry_key(cells, index),
            index,
            offset,
        };
        if point != selection.anchor {
            selection.moved = true;
        }
        selection.head = point;
        self.selection = Some(selection);
        true
    }

    /// Release the pointer. A gesture that never moved leaves no selection behind.
    pub(super) fn finish_selection_drag(&mut self) {
        let Some(mut selection) = self.selection else {
            return;
        };
        selection.dragging = false;
        self.selection = if selection.moved {
            Some(selection)
        } else {
            None
        };
    }

    /// Whether a screen position is inside the painted history window.
    pub(super) fn contains(&self, column: u16, row: u16) -> bool {
        self.area.contains(ScreenPosition::new(column, row))
    }

    /// Whether a screen position is inside the whole transcript frame, including the reserved
    /// gap/status row. Wheel input is routed by this: the gap row is visually part of the
    /// transcript even though no content paints there.
    pub(super) fn frame_contains(&self, column: u16, row: u16) -> bool {
        self.frame_area.contains(ScreenPosition::new(column, row))
    }

    /// Destination of the link under a screen position.
    pub(super) fn link_at(
        &mut self,
        cells: &[Arc<dyn HistoryCell>],
        column: u16,
        row: u16,
    ) -> Option<String> {
        let (index, row_in_entry) = self.visible_row(row)?;
        let layout = self.layout(cells, index)?;
        layout
            .link_at(row_in_entry, column.saturating_sub(self.area.x))
            .map(str::to_string)
    }

    /// Resolve a selection point against the current cell list.
    ///
    /// Falls back to the stored index when the identity no longer resolves, so a selection that
    /// outlives a spliced-away cell still lands somewhere sensible instead of panicking.
    fn resolve_point(
        &self,
        cells: &[Arc<dyn HistoryCell>],
        point: SelectionPoint,
    ) -> (usize, usize) {
        if point.key == EntryKey::Live {
            return (cells.len(), point.offset);
        }
        if cells
            .get(point.index)
            .is_some_and(|cell| EntryKey::cell(cell) == point.key)
        {
            return (point.index, point.offset);
        }
        let index = cells
            .iter()
            .position(|cell| EntryKey::cell(cell) == point.key)
            .unwrap_or_else(|| point.index.min(cells.len().saturating_sub(/*rhs*/ 1)));
        (index, point.offset)
    }

    /// Ordered (earlier, later) bounds of the active selection in current cell indices.
    pub(super) fn selection_bounds(
        &self,
        cells: &[Arc<dyn HistoryCell>],
    ) -> Option<((usize, usize), (usize, usize))> {
        let selection = self.selection?;
        let anchor = self.resolve_point(cells, selection.anchor);
        let head = self.resolve_point(cells, selection.head);
        if anchor <= head {
            Some((anchor, head))
        } else {
            Some((head, anchor))
        }
    }

    /// Extract the selected text, or `None` when nothing (or only blank rows) is selected.
    pub(super) fn selected_text(&mut self, cells: &[Arc<dyn HistoryCell>]) -> Option<String> {
        let ((start_index, start_offset), (end_index, end_offset)) =
            self.selection_bounds(cells)?;
        if (start_index, start_offset) == (end_index, end_offset) {
            return None;
        }
        let last_index = cells.len() + usize::from(self.live.is_some());
        let mut lines: Vec<String> = Vec::new();
        for index in start_index..=end_index.min(last_index.saturating_sub(/*rhs*/ 1)) {
            let Some(layout) = self.layout(cells, index) else {
                continue;
            };
            if layout.row_count() == 0 {
                continue;
            }
            let first_row = if index == start_index {
                layout.row_for_offset(start_offset)
            } else {
                0
            };
            let last_row = if index == end_index {
                layout.row_for_offset(end_offset)
            } else {
                layout.row_count().saturating_sub(/*rhs*/ 1)
            };
            for row in first_row..=last_row {
                let Some(source) = layout.row_source(row) else {
                    continue;
                };
                let low = if index == start_index && row == first_row {
                    source.start.max(start_offset)
                } else {
                    source.start
                };
                let high = if index == end_index && row == last_row {
                    source.end.min(end_offset)
                } else {
                    source.end
                };
                if low >= high {
                    continue;
                }
                let Some(text) = layout.text().get(low..high) else {
                    continue;
                };
                lines.push(text.trim_end().to_string());
            }
        }
        while lines.first().is_some_and(String::is_empty) {
            lines.remove(/*index*/ 0);
        }
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        if lines.is_empty() {
            return None;
        }
        Some(lines.join("\n"))
    }

    /// Paint the active selection over the rows the last frame recorded.
    pub(super) fn highlight_selection(
        &mut self,
        cells: &[Arc<dyn HistoryCell>],
        buf: &mut Buffer,
    ) {
        let Some(((start_index, start_offset), (end_index, end_offset))) =
            self.selection_bounds(cells)
        else {
            return;
        };
        let area = self.area;
        if area.is_empty() {
            return;
        }
        let style = selection_style();
        for offset in 0..usize::from(area.height) {
            let Some((index, row)) = self.visible.get(offset).copied() else {
                continue;
            };
            if index < start_index || index > end_index {
                continue;
            }
            let Some(layout) = self.layout(cells, index) else {
                continue;
            };
            let content_width = layout.row_content_width(row).unwrap_or(0);
            let low = if index == start_index {
                layout.column_for_offset(row, start_offset)
            } else {
                0
            };
            let high = if index == end_index {
                layout.column_for_offset(row, end_offset)
            } else {
                content_width
            };
            let high = high.min(content_width);
            if low >= high {
                continue;
            }
            let y = area.y.saturating_add(u16::try_from(offset).unwrap_or(0));
            for column in low..high {
                let x = area.x.saturating_add(column);
                if x < area.right() {
                    buf[(x, y)].set_style(style);
                }
            }
        }
    }
}
