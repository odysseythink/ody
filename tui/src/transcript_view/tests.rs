//! Unit tests for the virtualized transcript viewport.

use super::*;
use crate::history_cell::PlainHistoryCell;
use crate::terminal_hyperlinks::plain_hyperlink_lines;
use pretty_assertions::assert_eq;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::text::Text;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Wrap;

fn cell(text: &str) -> Arc<dyn HistoryCell> {
    let lines = text
        .split('\n')
        .map(|line| Line::from(line.to_string()))
        .collect::<Vec<_>>();
    Arc::new(PlainHistoryCell::new(lines))
}

fn render(view: &mut TranscriptView, cells: &[Arc<dyn HistoryCell>], area: Rect) -> Buffer {
    let mut buf = Buffer::empty(area);
    view.render(area, &mut buf, cells);
    buf
}

fn row_text(buf: &Buffer, y: u16) -> String {
    let mut text = String::new();
    for x in 0..buf.area.width {
        text.push_str(buf[(x, y)].symbol());
    }
    text.trim_end().to_string()
}

fn screen_text(buf: &Buffer) -> String {
    (0..buf.area.height)
        .map(|y| row_text(buf, y))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The view must measure rows exactly like the terminal scrollback path does, or the two modes
/// would disagree about where content lands.
#[test]
fn wrapped_rows_match_paragraph_line_count() {
    let cases = [
        "short line",
        "a line that is definitely longer than twenty four columns",
        "indented  text with words and more",
        "",
        "   ",
        "one\ntwo\nthree",
    ];
    for case in cases {
        for width in [1u16, 8, 24, 80] {
            let lines = case
                .split('\n')
                .map(|line| Line::from(line.to_string()))
                .collect::<Vec<_>>();
            let expected = Paragraph::new(Text::from(lines.clone()))
                .wrap(Wrap { trim: false })
                .line_count(width);
            let layout = TextLayout::new(plain_hyperlink_lines(lines), width);
            assert_eq!(
                layout.row_count(),
                expected,
                "width {width} case {case:?} row count mismatch"
            );
        }
    }
}

/// A reading anchor stores a source offset, so re-wrapping at a new width must keep the reader on
/// the same content instead of an unrelated row.
#[test]
fn reading_anchor_survives_width_change() {
    let cells = vec![
        cell("first entry"),
        cell("second entry with enough words to wrap across several rows at a narrow width"),
        cell("third entry"),
    ];
    let mut view = TranscriptView::default();
    render(&mut view, &cells, Rect::new(0, 0, 40, 4));
    view.scroll(&cells, -1);
    let Position::Reading(anchor) = view.position else {
        panic!("scrolling up enters reading mode");
    };
    assert_eq!(anchor.key, EntryKey::cell(&cells[1]));

    // Re-render at a different width: the anchor must still resolve to the same entry.
    render(&mut view, &cells, Rect::new(0, 0, 20, 4));
    let Position::Reading(anchor) = view.position else {
        panic!("still reading after resize");
    };
    assert_eq!(anchor.key, EntryKey::cell(&cells[1]));
    assert_eq!(anchor.index, 1);
}

/// Prepending history must not renumber a reading position: the anchor is keyed by cell identity.
#[test]
fn prepended_history_keeps_reading_position() {
    let older = vec![cell("older a"), cell("older b")];
    let mut cells = vec![cell("current a"), cell("current b"), cell("current c")];
    let mut view = TranscriptView::default();
    render(&mut view, &cells, Rect::new(0, 0, 30, 4));
    view.scroll(&cells, -2);
    let Position::Reading(anchor) = view.position else {
        panic!("reading");
    };
    let anchored_key = anchor.key;

    let mut prepended = older.clone();
    prepended.extend(cells.clone());
    cells = prepended;
    render(&mut view, &cells, Rect::new(0, 0, 30, 4));

    let Position::Reading(anchor) = view.position else {
        panic!("reading after prepend");
    };
    assert_eq!(anchor.key, anchored_key);
    assert_eq!(anchor.index, older.len(), "anchor tracks the shifted index");
}

/// A thousand-turn transcript must only format the entries near the visible window.
#[test]
fn only_visible_entries_are_formatted() {
    let cells: Vec<Arc<dyn HistoryCell>> = (0..1000)
        .map(|index| cell(&format!("turn {index} body")))
        .collect();
    let mut view = TranscriptView::default();
    render(&mut view, &cells, Rect::new(0, 0, 40, 8));
    assert!(
        view.cached_entry_count() <= 16,
        "expected a small cache, got {}",
        view.cached_entry_count()
    );
    assert!(view.tail_visible(), "follow mode shows the last row");
}

#[test]
fn scrolling_up_shows_older_content_and_hint() {
    let cells: Vec<Arc<dyn HistoryCell>> = (0..40)
        .map(|index| cell(&format!("turn {index}")))
        .collect();
    let area = Rect::new(0, 0, 24, 6);
    let mut view = TranscriptView::default();
    let following = render(&mut view, &cells, area);
    assert!(screen_text(&following).contains("turn 39"));

    view.scroll(&cells, -10);
    let scrolled = render(&mut view, &cells, area);
    assert!(!view.tail_visible());
    assert!(view.needs_return_to_bottom());
    // The hint takes the last row, so the reader keeps most of the window for older turns.
    assert!(row_text(&scrolled, area.height - 1).contains("Back to bottom"));
    let body = (0..area.height - 1)
        .map(|y| row_text(&scrolled, y))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(body.contains("turn 3"), "older turns are visible: {body}");
    assert!(!body.contains("turn 39"), "the tail scrolled away: {body}");
}

#[test]
fn scrolling_back_to_bottom_resumes_following() {
    let cells: Vec<Arc<dyn HistoryCell>> = (0..40)
        .map(|index| cell(&format!("turn {index}")))
        .collect();
    let area = Rect::new(0, 0, 24, 6);
    let mut view = TranscriptView::default();
    render(&mut view, &cells, area);
    view.scroll(&cells, -10);
    render(&mut view, &cells, area);
    assert!(!view.is_following());
    // Scrolling down far enough reaches the bottom window and resumes following.
    view.scroll(&cells, 20);
    assert!(view.is_following(), "scrolling to the bottom follows again");
    let buf = render(&mut view, &cells, area);
    assert!(!row_text(&buf, area.height - 1).contains("Back to bottom"));
}

/// Replacing a streamed run with its consolidated cell must move a reader anchored inside it.
#[test]
fn replace_range_moves_anchor_to_replacement() {
    let cells = vec![
        cell("committed a"),
        cell("committed b"),
        cell("stream one"),
        cell("stream two"),
    ];
    let mut view = TranscriptView::default();
    render(&mut view, &cells, Rect::new(0, 0, 30, 3));
    view.scroll(&cells, -1);
    let Position::Reading(anchor) = view.position else {
        panic!("reading");
    };
    assert_eq!(anchor.key, EntryKey::cell(&cells[2]));

    let consolidated = cell("consolidated answer");
    view.replace_range(&cells, 2..4, &consolidated);
    let Position::Reading(anchor) = view.position else {
        panic!("still reading");
    };
    assert_eq!(anchor.key, EntryKey::cell(&consolidated));
}

#[test]
fn live_tail_is_cached_until_the_key_changes() {
    let mut view = TranscriptView::default();
    let key = ActiveCellTranscriptKey {
        revision: 1,
        is_stream_continuation: false,
        animation_tick: None,
    };
    let calls = std::cell::Cell::new(0usize);
    let mut lines = |width: u16| {
        calls.set(calls.get() + 1);
        Some(plain_hyperlink_lines(vec![Line::from(format!(
            "streaming at {width}"
        ))]))
    };
    assert!(view.sync_live_tail(40, Some(key), &mut lines));
    assert_eq!(calls.get(), 1);
    // Same key: no re-formatting.
    assert!(!view.sync_live_tail(40, Some(key), &mut lines));
    assert_eq!(calls.get(), 1);
    // New revision: reformat.
    let bumped = ActiveCellTranscriptKey { revision: 2, ..key };
    assert!(view.sync_live_tail(40, Some(bumped), &mut lines));
    assert_eq!(calls.get(), 2);
    // New width: reformat.
    assert!(view.sync_live_tail(41, Some(bumped), &mut lines));
    assert_eq!(calls.get(), 3);
}
