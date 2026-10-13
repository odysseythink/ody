//! Unit tests for the virtualized transcript viewport.

use super::*;
use crate::history_cell::PlainHistoryCell;
use crate::terminal_hyperlinks::annotate_web_urls;
use crate::terminal_hyperlinks::plain_hyperlink_lines;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use crossterm::event::MouseButton;
use crossterm::event::MouseEvent;
use crossterm::event::MouseEventKind;
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

/// A cell whose text carries an OSC 8 destination, for link hit tests.
#[derive(Debug)]
struct LinkedCell(String);

impl HistoryCell for LinkedCell {
    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        vec![Line::from(self.0.clone())]
    }

    fn raw_lines(&self) -> Vec<Line<'static>> {
        vec![Line::from(self.0.clone())]
    }

    fn display_hyperlink_lines(&self, width: u16) -> Vec<HyperlinkLine> {
        annotate_web_urls(self.display_lines(width))
    }
}

/// A collapsible cell that renders the same "alt+o to expand" hint real cells do.
#[derive(Debug, Default)]
struct CollapsibleCell {
    expanded: std::sync::atomic::AtomicBool,
}

impl HistoryCell for CollapsibleCell {
    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        let mut lines = vec![Line::from("collapsed body".to_string())];
        if !self.is_expanded() {
            lines.push(Line::from(
                "... (3 more lines, alt+o to expand)".to_string(),
            ));
        }
        lines
    }

    fn raw_lines(&self) -> Vec<Line<'static>> {
        self.display_lines(/*width*/ 80)
    }

    fn is_collapsible(&self) -> bool {
        true
    }

    fn is_expanded(&self) -> bool {
        self.expanded
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    fn toggle_expanded(&self) -> bool {
        let next = !self.is_expanded();
        self.expanded
            .store(next, std::sync::atomic::Ordering::Relaxed);
        next
    }
}

fn pager() -> crate::keymap::PagerKeymap {
    crate::keymap::RuntimeKeymap::defaults().pager
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn key_with(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, modifiers)
}

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
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

// ---------------------------------------------------------------------------------------------
// P3: selection, copy, links, disclosure, find and their routing.
// ---------------------------------------------------------------------------------------------

/// A drag selects exactly the dragged characters, and the selection is stored by content so it
/// survives a re-wrap and a prepended page.
#[test]
fn drag_selection_extracts_text_and_survives_rewrap() {
    let cells = vec![cell("alpha beta"), cell("gamma delta")];
    let area = Rect::new(0, 0, 20, 6);
    let mut view = TranscriptView::default();
    render(&mut view, &cells, area);

    // Rows: 0 "alpha beta", 1 separator, 2 "gamma delta".
    assert!(
        view.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 0, 2), &cells)
            .is_some()
    );
    assert!(
        view.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 5, 2), &cells)
            .is_some()
    );
    view.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 5, 2), &cells);
    assert!(view.has_selection());
    assert_eq!(view.selected_text(&cells).as_deref(), Some("gamma"));

    // Extending across the separator keeps the row text and drops the blank separator row.
    view.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 0, 0), &cells);
    view.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 5, 2), &cells);
    view.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 5, 2), &cells);
    assert_eq!(view.selected_text(&cells).as_deref(), Some("alpha beta\ngamma"));
    assert_eq!(view.selection_bounds(&cells), Some(((0, 0), (1, 5))));

    // The anchor is content based: a resize moves the rows, not the selected characters.
    render(&mut view, &cells, Rect::new(0, 0, 8, 6));
    assert_eq!(view.selection_bounds(&cells), Some(((0, 0), (1, 5))));
    assert!(
        view.selected_text(&cells)
            .is_some_and(|text| text.starts_with("alpha")),
        "the same characters stay selected at a new width"
    );

    // Prepending older history shifts the indices but not the selected characters.
    let mut prepended = vec![cell("older a"), cell("older b")];
    prepended.extend(cells.clone());
    render(&mut view, &prepended, Rect::new(0, 0, 20, 6));
    assert_eq!(view.selection_bounds(&prepended), Some(((2, 0), (3, 5))));
    assert_eq!(
        view.selected_text(&prepended).as_deref(),
        Some("alpha beta\ngamma")
    );
}

/// A click that never moves leaves no zero-width selection behind (pager overlay behavior).
#[test]
fn stationary_click_leaves_no_selection() {
    let cells = vec![cell("alpha beta")];
    let mut view = TranscriptView::default();
    render(&mut view, &cells, Rect::new(0, 0, 20, 4));

    view.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 3, 0), &cells);
    view.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 3, 0), &cells);
    assert!(!view.has_selection());
}

#[test]
fn double_click_selects_the_whole_row() {
    let cells = vec![cell("alpha beta"), cell("gamma delta")];
    let mut view = TranscriptView::default();
    render(&mut view, &cells, Rect::new(0, 0, 20, 6));

    // Two clicks on the same cell become a whole-row selection on the second press.
    view.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 2, 0), &cells);
    view.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 2, 0), &cells);
    view.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 2, 0), &cells);
    assert_eq!(view.selected_text(&cells).as_deref(), Some("alpha beta"));
}

/// A stationary click on a link opens it; Ctrl-click does the same without a selection.
#[test]
fn click_on_a_link_queues_it_to_open() {
    let cells: Vec<Arc<dyn HistoryCell>> =
        vec![Arc::new(LinkedCell("see https://example.com/docs here".to_string()))];
    let mut view = TranscriptView::default();
    render(&mut view, &cells, Rect::new(0, 0, 40, 4));

    view.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 6, 0), &cells);
    let action = view.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 6, 0), &cells);
    let Some(ViewAction::OpenLink(destination)) = action else {
        panic!("expected a link activation, got {action:?}");
    };
    assert!(destination.contains("example.com/docs"), "{destination}");
    assert!(!view.has_selection());

    // Ctrl-click activates immediately, on the press.
    let action = view.handle_mouse(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 6,
            row: 0,
            modifiers: KeyModifiers::CONTROL,
        },
        &cells,
    );
    assert!(matches!(action, Some(ViewAction::OpenLink(_))));
}

/// Right-click copies an existing selection, matching the pager overlay.
#[test]
fn right_click_copies_the_selection() {
    let cells = vec![cell("alpha beta")];
    let mut view = TranscriptView::default();
    render(&mut view, &cells, Rect::new(0, 0, 20, 4));

    view.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 0, 0), &cells);
    view.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 5, 0), &cells);
    let action = view.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Right), 5, 0), &cells);
    assert_eq!(action, Some(ViewAction::Copy("alpha".to_string())));
}

/// Clicking the collapsed-form hint toggles that cell and only that cell.
#[test]
fn click_on_the_collapse_hint_toggles_that_cell() {
    let collapsible = Arc::new(CollapsibleCell::default());
    let cells: Vec<Arc<dyn HistoryCell>> = vec![cell("plain"), Arc::clone(&collapsible) as _];
    let area = Rect::new(0, 0, 40, 8);
    let mut view = TranscriptView::default();
    let buf = render(&mut view, &cells, area);
    assert!(screen_text(&buf).contains("alt+o to expand"));

    // Rows: 0 "plain", 1 separator, 2 "collapsed body", 3 the hint.
    assert!(
        view.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 4, 3), &cells)
            .is_some()
    );
    assert!(collapsible.is_expanded(), "the hint expands its own cell");
    let buf = render(&mut view, &cells, area);
    assert!(!screen_text(&buf).contains("alt+o to expand"));
    assert!(!view.has_selection(), "the toggle does not leave a selection");

    // Clicking the body starts a selection instead of toggling.
    view.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 4, 2), &cells);
    assert!(collapsible.is_expanded());
    assert!(view.has_selection());
}

/// Navigation only claims keys that cannot be draft text.
#[test]
fn navigation_keys_leave_draft_text_alone() {
    let view = TranscriptView::default();
    let pager = pager();
    for code in [
        KeyCode::Up,
        KeyCode::Down,
        KeyCode::PageUp,
        KeyCode::PageDown,
        KeyCode::Home,
        KeyCode::End,
        KeyCode::Esc,
    ] {
        assert!(view.owns_navigation_key(key(code), &pager), "{code:?}");
    }
    assert!(view.owns_navigation_key(key_with(KeyCode::Char('u'), KeyModifiers::CONTROL), &pager));
    assert!(view.owns_navigation_key(key_with(KeyCode::Char('f'), KeyModifiers::CONTROL), &pager));
    // Typing keys are never claimed, even though the pager binds them.
    for code in [KeyCode::Char('j'), KeyCode::Char('k'), KeyCode::Char(' ')] {
        assert!(!view.owns_navigation_key(key(code), &pager), "{code:?}");
    }
}

/// Find mode owns typing, but leaves the app's own shortcuts alone.
#[test]
fn find_mode_owns_typing_but_not_app_shortcuts() {
    let mut view = TranscriptView::default();
    view.begin_search();
    assert!(view.owns_interaction_key(key(KeyCode::Char('n'))));
    assert!(view.owns_interaction_key(key(KeyCode::Backspace)));
    assert!(view.owns_interaction_key(key(KeyCode::Esc)));
    assert!(view.owns_interaction_key(key(KeyCode::Enter)));
    assert!(view.owns_interaction_key(key_with(KeyCode::Char('n'), KeyModifiers::CONTROL)));
    assert!(
        !view.owns_interaction_key(key_with(KeyCode::Char('t'), KeyModifiers::CONTROL)),
        "Ctrl+T still opens the transcript overlay"
    );

    // Accepted query: typing is released, navigation still belongs to the view.
    view.cancel_search();
    assert!(!view.owns_interaction_key(key(KeyCode::Char('n'))));
}

/// A selection keeps the copy keys and motion keys even with a draft on screen.
#[test]
fn selection_claims_its_keys() {
    let mut view = TranscriptView::default();
    assert!(!view.owns_interaction_key(key(KeyCode::Enter)));

    let cells = vec![cell("alpha beta")];
    render(&mut view, &cells, Rect::new(0, 0, 20, 4));
    view.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 0, 0), &cells);
    view.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 5, 0), &cells);

    assert!(view.owns_interaction_key(key(KeyCode::Enter)));
    assert!(view.owns_interaction_key(key(KeyCode::Esc)));
    assert!(view.owns_interaction_key(key(KeyCode::Down)));
    assert!(view.owns_interaction_key(key_with(KeyCode::Down, KeyModifiers::SHIFT)));
    assert!(!view.owns_interaction_key(key(KeyCode::Char('a'))));
}

/// Find walks the loaded history in bounded batches and jumps the viewport to a match.
#[test]
fn find_jumps_to_a_match_and_reports_progress() {
    let cells: Vec<Arc<dyn HistoryCell>> = vec![
        cell("alpha"),
        cell("needle one"),
        cell("beta"),
        cell("needle two"),
        cell("gamma"),
    ];
    let area = Rect::new(0, 0, 20, 6);
    let mut view = TranscriptView::default();
    render(&mut view, &cells, area);

    view.begin_search();
    for ch in "needle".chars() {
        view.push_search_char(ch);
    }
    // One step covers the whole loaded history.
    assert!(!view.search_step(&cells));
    let status = view.search_status().expect("find status");
    assert!(status.contains("2/2"), "{status}");

    // The cursor starts on the newest match.
    let Position::Reading(anchor) = view.position else {
        panic!("find moves the reader to the match");
    };
    assert_eq!(anchor.key, EntryKey::cell(&cells[3]));

    assert!(view.search_advance(&cells, /*newer*/ false));
    let Position::Reading(anchor) = view.position else {
        panic!("still reading");
    };
    assert_eq!(anchor.key, EntryKey::cell(&cells[1]));

    // Esc closes find and leaves the reader where the last match was.
    view.handle_key(key(KeyCode::Esc), &cells, &pager());
    assert!(!view.search_active());
    assert!(!view.is_following());
}

/// One find step never wraps the whole loaded history.
#[test]
fn find_scan_is_bounded_per_step() {
    let cells: Vec<Arc<dyn HistoryCell>> = (0..300)
        .map(|index| cell(&format!("turn {index} needle")))
        .collect();
    let mut view = TranscriptView::default();
    render(&mut view, &cells, Rect::new(0, 0, 40, 8));

    view.begin_search();
    for ch in "needle".chars() {
        view.push_search_char(ch);
    }
    view.search_step(&cells);
    assert!(
        view.search_match_count() <= 24,
        "one step formats a bounded number of entries, saw {}",
        view.search_match_count()
    );

    let mut steps = 1;
    while view.search_step(&cells) {
        steps += 1;
        assert!(steps < 100, "the scan must terminate");
    }
    assert_eq!(
        view.search_match_count(),
        300,
        "every loaded entry is covered"
    );
}

/// Esc unwinds the interaction layers in order: find, then selection, then the reading position.
#[test]
fn esc_ladder_unwinds_selection_then_find_then_reading() {
    let cells: Vec<Arc<dyn HistoryCell>> = (0..6)
        .map(|index| cell(&format!("row {index}")))
        .collect();
    let area = Rect::new(0, 0, 20, 5);
    let mut view = TranscriptView::default();
    render(&mut view, &cells, area);
    let pager = pager();

    view.scroll(&cells, -2);
    assert!(!view.is_following());

    // Find: Esc closes it and keeps the reading position.
    view.begin_search();
    assert!(view.handle_key(key(KeyCode::Esc), &cells, &pager).is_some());
    assert!(!view.search_active());
    assert!(!view.is_following());

    // Selection: Esc clears it and keeps the reading position.
    view.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 0, 0), &cells);
    view.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 2, 0), &cells);
    assert!(view.has_selection());
    assert!(view.handle_key(key(KeyCode::Esc), &cells, &pager).is_some());
    assert!(!view.has_selection());
    assert!(!view.is_following());

    // Reading: Esc returns to the newest content.
    assert!(view.handle_key(key(KeyCode::Esc), &cells, &pager).is_some());
    assert!(view.is_following());

    // A printable key is never a transcript gesture while the view is only reading.
    view.scroll(&cells, -2);
    assert!(view.handle_key(key(KeyCode::Char('x')), &cells, &pager).is_none());
}

/// Streaming output must not drag a reader who is back in the history.
#[test]
fn streaming_keeps_the_reading_anchor() {
    let cells = vec![cell("turn one"), cell("turn two"), cell("turn three")];
    let area = Rect::new(0, 0, 20, 4);
    let mut view = TranscriptView::default();
    render(&mut view, &cells, area);
    view.scroll(&cells, -2);
    let Position::Reading(before) = view.position else {
        panic!("reading");
    };

    let tail = |text: &str| Some(plain_hyperlink_lines(vec![Line::from(text.to_string())]));
    view.sync_live_tail(
        20,
        Some(ActiveCellTranscriptKey {
            revision: 1,
            is_stream_continuation: false,
            animation_tick: None,
        }),
        |_| tail("streaming part one"),
    );
    render(&mut view, &cells, area);
    view.sync_live_tail(
        20,
        Some(ActiveCellTranscriptKey {
            revision: 2,
            is_stream_continuation: false,
            animation_tick: None,
        }),
        |_| tail("streaming part one and more text"),
    );
    render(&mut view, &cells, area);

    let Position::Reading(after) = view.position else {
        panic!("the reader stays in the history while the tail streams");
    };
    assert_eq!(after.key, before.key);
    assert!(view.has_unseen_activity());
    assert!(view.needs_return_to_bottom());
}

// ---------------------------------------------------------------------------------------------
// P4: viewport-driven paging and cross-page search.
// ---------------------------------------------------------------------------------------------

/// Scrolling to the top of the loaded history exposes a page request; the bottom never does.
#[test]
fn paging_is_requested_only_near_the_loaded_top() {
    let area = Rect::new(0, 0, 20, 4);
    let cells: Vec<Arc<dyn HistoryCell>> = (0..12)
        .map(|index| cell(&format!("row {index}")))
        .collect();
    let mut view = TranscriptView::default();
    view.history = TranscriptHistoryState::Partial;

    render(&mut view, &cells, area);
    assert!(
        !view.needs_history(&cells),
        "the bottom of a loaded window must not page"
    );

    view.jump_to_beginning(&cells);
    // Older pages still exist, so the jump is only *aimed*; it must ask for history.
    assert!(view.is_loading_beginning());
    assert!(view.needs_history(&cells));
}

/// A pending beginning jump holds the reading position while pages arrive, then lands on row 0.
#[test]
fn beginning_jump_holds_position_until_history_is_exhausted() {
    let area = Rect::new(0, 0, 20, 4);
    let mut cells: Vec<Arc<dyn HistoryCell>> = (0..8)
        .map(|index| cell(&format!("row {index}")))
        .collect();
    let mut view = TranscriptView::default();
    view.history = TranscriptHistoryState::Partial;
    render(&mut view, &cells, area);

    view.jump_to_beginning(&cells);
    let Position::Reading(held) = view.position else {
        panic!("a beginning jump while history is unloaded keeps the current position");
    };
    // The anchor must not be re-aimed at the oldest loaded row: that is what makes the top crawl.
    assert_eq!(held.index, view.resolve(&cells, held));

    // An older page lands.
    let older: Vec<Arc<dyn HistoryCell>> = (0..4)
        .map(|index| cell(&format!("old {index}")))
        .collect();
    cells.splice(0..0, older);
    view.set_history_complete(/*complete*/ false);
    view.history_loaded(&cells, 0..4);
    assert!(
        view.is_loading_beginning(),
        "still chaining until the server is exhausted"
    );

    // No more pages: land on the real beginning.
    view.set_history_complete(/*complete*/ true);
    view.land_on_beginning(&cells);
    let Position::Reading(landed) = view.position else {
        panic!("after the jump completes the view is reading");
    };
    assert_eq!(landed.index, 0);
    assert!(!view.needs_history(&cells));
    assert!(!view.needs_return_to_bottom());
}

/// Scrolling away abandons a pending beginning jump but keeps loading older pages.
#[test]
fn scrolling_cancels_a_beginning_jump_without_stopping_paging() {
    let area = Rect::new(0, 0, 20, 6);
    let cells: Vec<Arc<dyn HistoryCell>> = (0..20)
        .map(|index| cell(&format!("row {index}")))
        .collect();
    let mut view = TranscriptView::default();
    view.history = TranscriptHistoryState::Partial;
    render(&mut view, &cells, area);

    view.jump_to_beginning(&cells);
    assert!(view.is_loading_beginning());

    view.scroll(&cells, /*rows*/ -1);
    assert!(!view.is_loading_beginning());
    assert_eq!(view.history, TranscriptHistoryState::LoadingOlder);
    assert!(view.wants_older_history());
}

/// A prepend keeps the reader on the same content and can resume a parked scan.
#[test]
fn prepended_page_keeps_the_anchor_and_resumes_search() {
    let area = Rect::new(0, 0, 40, 6);
    let mut cells: Vec<Arc<dyn HistoryCell>> = (0..6)
        .map(|index| cell(&format!("row {index}")))
        .collect();
    let mut view = TranscriptView::default();
    view.history = TranscriptHistoryState::Partial;
    render(&mut view, &cells, area);

    // Read an older row, then park a scan at the top of what is loaded.
    view.scroll(&cells, /*rows*/ -2);
    let Position::Reading(before) = view.position else {
        panic!("scrolling up leaves the live tail");
    };
    let anchored = view.resolve(&cells, before);
    view.begin_search();
    view.push_search_char('z');
    view.search_step(&cells);
    // `z` matches nothing in the loaded window; older pages remain, so it parks.
    assert!(view.search_awaiting_history());

    let older: Vec<Arc<dyn HistoryCell>> = (0..4)
        .map(|index| cell(&format!("oldz {index}")))
        .collect();
    cells.splice(0..0, older);
    view.history_loaded(&cells, 0..4);
    // The anchor moved down by the page, but still names the same cell.
    let Position::Reading(after) = view.position else {
        panic!("still reading after the prepend");
    };
    assert_eq!(view.resolve(&cells, after), anchored + 4);

    // The scan resumes over the inserted page and finds the older hit.
    view.search_step(&cells);
    assert!(view.search_match_count() > 0, "the older page was searched");
}

/// A scan that reaches the oldest loaded entry with no pages left reports exhaustion.
#[test]
fn search_exhausts_once_history_is_complete() {
    let area = Rect::new(0, 0, 40, 6);
    let cells: Vec<Arc<dyn HistoryCell>> = (0..4)
        .map(|index| cell(&format!("row {index}")))
        .collect();
    let mut view = TranscriptView::default();
    view.history = TranscriptHistoryState::Complete;
    render(&mut view, &cells, area);

    view.begin_search();
    view.push_search_char('r');
    while view.search_step(&cells) {}
    assert!(view.search_exhausted());
    assert!(!view.search_awaiting_history());
    assert!(view.search_match_count() > 0);
}

/// A retry gesture clears a failed page so paging resumes instead of stalling.
#[test]
fn failed_page_is_retryable_from_the_jump_key() {
    let area = Rect::new(0, 0, 20, 6);
    let cells: Vec<Arc<dyn HistoryCell>> = (0..20)
        .map(|index| cell(&format!("row {index}")))
        .collect();
    let mut view = TranscriptView::default();
    view.history = TranscriptHistoryState::Partial;
    render(&mut view, &cells, area);

    view.mark_history_failed();
    assert!(!view.wants_older_history(), "nothing re-requests on its own");

    let action = view.handle_key(key(KeyCode::Home), &cells, &pager());
    assert_eq!(action, Some(ViewAction::Changed));
    assert!(
        view.is_loading_beginning(),
        "Home after a failure restarts the jump instead of refusing"
    );
    assert!(view.needs_history(&cells));
}

