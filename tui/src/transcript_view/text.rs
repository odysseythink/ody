//! Width-dependent display rows for one transcript entry.
//!
//! The owned viewport measures and paints the same rows it lays out, so this module reimplements
//! ratatui's word wrapping state machine (`WordWrapper` with `trim = false`, the mode every other
//! transcript path in this crate uses) while additionally recording where each row came from in the
//! entry's source text.
//!
//! That source range is what makes a reading anchor survive re-wrapping: an anchor stores a byte
//! offset into the entry text, and `row_for_offset`/`position_at` translate between offsets and
//! display rows for whatever width the terminal currently has. Display wrapping and the synthetic
//! separator row never become part of the source text.

use std::collections::VecDeque;
use std::ops::Range;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Widget;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::terminal_hyperlinks::HyperlinkLine;
use crate::terminal_hyperlinks::TerminalHyperlink;
use crate::terminal_hyperlinks::osc8_hyperlink;

/// One entry's wrapped display rows plus the source offsets they came from.
pub(super) struct TextLayout {
    text: String,
    rows: Vec<TextRow>,
    /// Whether a blank separator row precedes this entry's first source row.
    pub(super) separated: bool,
}

struct TextRow {
    line: HyperlinkLine,
    /// Byte range of this row's content inside `TextLayout::text`.
    ///
    /// Wrapping drops trailing whitespace at a break, so consecutive rows can leave gaps in the
    /// source text; the ranges themselves are always contiguous.
    source: Range<usize>,
    content_width: u16,
}

impl TextRow {
    fn empty() -> Self {
        Self {
            line: HyperlinkLine::default(),
            source: 0..0,
            content_width: 0,
        }
    }
}

/// One grapheme of a source line, with the display column it occupied before wrapping.
struct Grapheme {
    symbol: String,
    style: Style,
    /// Byte offset of the grapheme inside `TextLayout::text`.
    start: usize,
    /// Display column of the grapheme in the unwrapped source line.
    column: usize,
}

impl TextLayout {
    pub(super) fn new(lines: Vec<HyperlinkLine>, width: u16) -> Self {
        let mut text = String::new();
        let mut rows = Vec::new();

        for (index, line) in lines.iter().enumerate() {
            if index > 0 {
                text.push('\n');
            }
            let base = text.len();
            for span in &line.line.spans {
                text.push_str(&span.content);
            }
            let graphemes = line_graphemes(line, base);
            wrap_source_line(&mut rows, graphemes, width, &line.hyperlinks);
        }

        Self {
            text,
            rows,
            separated: false,
        }
    }

    /// Add a blank row before the entry so adjacent blocks do not run together.
    ///
    /// This mirrors the separator the terminal scrollback path inserts between non-continuation
    /// cells, without touching any source offset.
    pub(super) fn with_leading_separator(mut self) -> Self {
        if !self.rows.is_empty() {
            self.rows.insert(/*index*/ 0, TextRow::empty());
        }
        self.separated = !self.rows.is_empty();
        self
    }

    pub(super) fn text(&self) -> &str {
        &self.text
    }

    pub(super) fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// Paint `area.height` rows starting at `start_row`.
    pub(super) fn render(&self, area: Rect, buf: &mut Buffer, start_row: usize) {
        if area.is_empty() {
            return;
        }
        for (offset, row) in self
            .rows
            .iter()
            .enumerate()
            .skip(start_row)
            .take(usize::from(area.height))
        {
            let Ok(screen_row) = u16::try_from(offset - start_row) else {
                break;
            };
            let y = area.y + screen_row;
            let row_area = Rect::new(area.x, y, area.width, /*height*/ 1);
            buf.set_style(row_area, row.line.line.style);
            let content_area = Rect::new(
                area.x,
                y,
                row.content_width.min(area.width),
                /*height*/ 1,
            );
            if !content_area.is_empty() {
                row.line.line.clone().render(content_area, buf);
            }
            mark_row_hyperlinks(buf, row_area, row);
        }
    }

    /// Resolve a display column back to a byte offset in the entry text.
    pub(super) fn position_at(&self, row: usize, column: u16) -> usize {
        let Some(row) = self.rows.get(row) else {
            return self.text.len();
        };
        let mut current_column = 0usize;
        for (offset, grapheme) in
            self.text[row.source.clone()].grapheme_indices(/*extended*/ true)
        {
            current_column += grapheme.width();
            if usize::from(column) < current_column {
                return row.source.start + offset;
            }
        }
        row.source.end
    }

    /// Find the display row that contains a source offset.
    pub(super) fn row_for_offset(&self, offset: usize) -> usize {
        self.rows
            .partition_point(|row| row.source.start <= offset)
            .saturating_sub(/*rhs*/ 1)
    }

    /// Byte range of one display row's content inside `TextLayout::text`.
    pub(super) fn row_source(&self, row: usize) -> Option<Range<usize>> {
        self.rows.get(row).map(|row| row.source.clone())
    }

    /// Width of one row's content, excluding the blanks a wrapped row left unpainted.
    pub(super) fn row_content_width(&self, row: usize) -> Option<u16> {
        self.rows.get(row).map(|row| row.content_width)
    }

    /// Display column of a source offset inside one row.
    ///
    /// The inverse of `position_at`, used to turn a stored selection offset back into the column
    /// range to highlight after a re-wrap.
    pub(super) fn column_for_offset(&self, row: usize, offset: usize) -> u16 {
        let Some(row) = self.rows.get(row) else {
            return 0;
        };
        let mut column = 0u16;
        for (index, grapheme) in self.text[row.source.clone()].grapheme_indices(/*extended*/ true) {
            if row.source.start + index >= offset {
                break;
            }
            column = column.saturating_add(u16::try_from(grapheme.width()).unwrap_or(0));
        }
        column
    }

    /// Destination of the link covering `column` in `row`, if any.
    pub(super) fn link_at(&self, row: usize, column: u16) -> Option<&str> {
        let row = self.rows.get(row)?;
        let column = usize::from(column);
        row.line
            .hyperlinks
            .iter()
            .find(|link| link.columns.contains(&column))
            .map(|link| link.destination.as_str())
    }
}

/// Collect the graphemes of one source line, recording columns over the full unwrapped line.
///
/// Columns are accumulated before the width filter inside `wrap_source_line`, because hyperlink
/// ranges are expressed in the columns the source line would occupy unfolded.
fn line_graphemes(line: &HyperlinkLine, base: usize) -> Vec<Grapheme> {
    let mut graphemes = Vec::new();
    let mut offset = base;
    let mut column = 0usize;
    for span in &line.line.spans {
        for grapheme in span.content.graphemes(/*extended*/ true) {
            graphemes.push(Grapheme {
                symbol: grapheme.to_string(),
                style: span.style,
                start: offset,
                column,
            });
            offset += grapheme.len();
            column += grapheme.width();
        }
    }
    graphemes
}

/// Wrap one source line into rows, mirroring ratatui's `WordWrapper` with `trim = false`.
///
/// Every call emits at least one row, which is what keeps row counts aligned with
/// `HistoryCell::desired_height_for_mode` for the same source lines.
fn wrap_source_line(
    rows: &mut Vec<TextRow>,
    graphemes: Vec<Grapheme>,
    width: u16,
    links: &[TerminalHyperlink],
) {
    if width == 0 {
        return;
    }

    let max = width;
    let first_row = rows.len();
    let mut pending_line: Vec<Grapheme> = Vec::new();
    let mut pending_word: Vec<Grapheme> = Vec::new();
    let mut pending_whitespace: VecDeque<Grapheme> = VecDeque::new();
    let mut line_width: u16 = 0;
    let mut word_width: u16 = 0;
    let mut whitespace_width: u16 = 0;
    let mut non_whitespace_previous = false;

    for grapheme in graphemes {
        let is_whitespace = grapheme.symbol.chars().all(char::is_whitespace);
        let Ok(symbol_width) = u16::try_from(grapheme.symbol.width()) else {
            continue;
        };

        // Symbols wider than the viewport can never be displayed; skip them exactly like ratatui.
        if symbol_width > max {
            continue;
        }

        let word_found = non_whitespace_previous && is_whitespace;
        // `trim = false` keeps leading whitespace, so only the untrimmed overflow case applies.
        let untrimmed_overflow =
            pending_line.is_empty() && word_width + whitespace_width + symbol_width > max;

        if word_found || untrimmed_overflow {
            // `trim = false` always keeps the pending whitespace with the word it precedes.
            pending_line.extend(pending_whitespace.drain(..));
            line_width += whitespace_width;
            pending_line.append(&mut pending_word);
            line_width += word_width;
            pending_whitespace.clear();
            whitespace_width = 0;
            word_width = 0;
        }

        let line_full = line_width >= max;
        let pending_word_overflow =
            symbol_width > 0 && line_width + whitespace_width + word_width >= max;

        if line_full || pending_word_overflow {
            let mut remaining_width = max.saturating_sub(line_width);
            push_row(rows, std::mem::take(&mut pending_line), links);
            line_width = 0;

            // Whitespace that no longer fits at the end of the emitted row is dropped.
            while let Some(whitespace) = pending_whitespace.front() {
                let Ok(trimmed_width) = u16::try_from(whitespace.symbol.width()) else {
                    break;
                };
                if trimmed_width > remaining_width {
                    break;
                }
                whitespace_width -= trimmed_width;
                remaining_width -= trimmed_width;
                pending_whitespace.pop_front();
            }

            if is_whitespace && pending_whitespace.is_empty() {
                continue;
            }
        }

        if is_whitespace {
            whitespace_width += symbol_width;
            pending_whitespace.push_back(grapheme);
        } else {
            word_width += symbol_width;
            pending_word.push(grapheme);
        }
        non_whitespace_previous = !is_whitespace;
    }

    if pending_line.is_empty() && pending_word.is_empty() && !pending_whitespace.is_empty() {
        rows.push(TextRow::empty());
    }
    pending_line.extend(pending_whitespace.drain(..));
    pending_line.append(&mut pending_word);
    if !pending_line.is_empty() {
        push_row(rows, pending_line, links);
    }
    if rows.len() == first_row {
        rows.push(TextRow::empty());
    }
}

fn push_row(rows: &mut Vec<TextRow>, graphemes: Vec<Grapheme>, links: &[TerminalHyperlink]) {
    let source = match (graphemes.first(), graphemes.last()) {
        (Some(first), Some(last)) => first.start..last.start + last.symbol.len(),
        _ => 0..0,
    };
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut content_width = 0u16;
    for grapheme in &graphemes {
        let Ok(width) = u16::try_from(grapheme.symbol.width()) else {
            continue;
        };
        content_width += width;
        match spans.last_mut() {
            Some(span) if span.style == grapheme.style => {
                span.content.to_mut().push_str(&grapheme.symbol);
            }
            _ => spans.push(Span::styled(grapheme.symbol.clone(), grapheme.style)),
        }
    }
    rows.push(TextRow {
        line: HyperlinkLine {
            line: Line::from(spans),
            hyperlinks: row_hyperlinks(&graphemes, links),
        },
        source,
        content_width,
    });
}

/// Map link columns from the unwrapped source line onto this row's columns.
fn row_hyperlinks(graphemes: &[Grapheme], links: &[TerminalHyperlink]) -> Vec<TerminalHyperlink> {
    if links.is_empty() {
        return Vec::new();
    }
    let mut mapped: Vec<TerminalHyperlink> = Vec::new();
    let mut row_column = 0usize;
    for grapheme in graphemes {
        let width = grapheme.symbol.width();
        if width > 0 {
            for link in links {
                let overlaps = link.columns.start < grapheme.column + width
                    && link.columns.end > grapheme.column;
                if !overlaps {
                    continue;
                }
                match mapped.last_mut() {
                    Some(last)
                        if last.destination == link.destination
                            && last.columns.end == row_column =>
                    {
                        last.columns.end = row_column + width;
                    }
                    _ => mapped.push(TerminalHyperlink {
                        columns: row_column..row_column + width,
                        destination: link.destination.clone(),
                    }),
                }
            }
        }
        row_column += width;
    }
    mapped
}

/// Attach OSC 8 hyperlink bytes to the painted cells of one display row.
fn mark_row_hyperlinks(buf: &mut Buffer, area: Rect, row: &TextRow) {
    for link in &row.line.hyperlinks {
        for column in link.columns.clone() {
            if column >= usize::from(area.width) {
                break;
            }
            let cell = &mut buf[(area.x + column as u16, area.y)];
            if cell.skip || cell.symbol().trim().is_empty() {
                continue;
            }
            let symbol = osc8_hyperlink(&link.destination, cell.symbol());
            cell.set_symbol(&symbol);
        }
    }
}
