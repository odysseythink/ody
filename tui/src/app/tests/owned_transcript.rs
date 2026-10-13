//! Owned (full-screen) transcript acceptance tests.
//!
//! These cover the P2 contract: the main screen renders history and the input surface inside a
//! single frame, history stays in `transcript_cells` instead of terminal scrollback, and nothing
//! the Owned path emits purges the shell's scrollback.

use std::sync::Arc;

use super::*;
use crate::app::input::OwnedTranscriptScroll;
use crate::app::test_support::make_test_app;
use crate::custom_terminal::Terminal;
use crate::history_cell::PlainHistoryCell;
use crate::test_backend::VT100Backend;
use crate::tui::test_support::make_test_tui;
use pretty_assertions::assert_eq;
use ratatui::text::Line;

/// Byte sequence that purges the terminal's scrollback. Owned mode must never emit it.
const SCROLLBACK_PURGE: &[u8] = b"\x1b[3J";

fn history_cell(text: &str) -> Arc<dyn HistoryCell> {
    Arc::new(PlainHistoryCell::new(vec![Line::from(text.to_string())]))
}

fn screen_contents(terminal: &Terminal<VT100Backend>) -> String {
    terminal.backend().vt100().screen().contents()
}

/// A frame big enough for a short transcript, drawn through the Owned render path.
///
/// Recorded output is scoped to the frame itself, so the no-purge assertion cannot be satisfied
/// by bytes written before the draw.
fn draw_owned_frame(app: &mut App, width: u16, height: u16) -> Terminal<VT100Backend> {
    let mut terminal =
        Terminal::with_options(VT100Backend::new(width, height)).expect("test terminal");
    terminal.set_viewport_area(Rect::new(0, 0, width, height));
    terminal.backend_mut().clear_output();
    terminal
        .draw(|frame| {
            let area = frame.area();
            app.render_owned_transcript_frame(area, frame);
        })
        .expect("draw owned frame");
    terminal
}

/// Only the tail is formatted and painted, and no scrollback purge escapes the frame.
#[tokio::test]
async fn owned_frame_paints_history_without_replaying_scrollback() {
    let mut app = make_test_app().await;
    for index in 0..1000 {
        app.transcript_cells
            .push(history_cell(&format!("turn {index} body")));
    }

    let width = 40;
    let height = 10;
    let terminal = draw_owned_frame(&mut app, width, height);
    let screen = screen_contents(&terminal);

    assert!(
        screen.contains("turn 999 body"),
        "the newest turn follows the tail: {screen}"
    );
    assert!(
        !screen.contains("turn 0 body"),
        "older turns are not replayed into the window: {screen}"
    );
    assert!(
        !terminal
            .backend()
            .output()
            .windows(SCROLLBACK_PURGE.len())
            .any(|window| window == SCROLLBACK_PURGE),
        "an owned frame must not purge the shell's scrollback"
    );
}

/// A small terminal with a multi-line draft still leaves the composer usable.
#[tokio::test]
async fn owned_small_terminal_keeps_a_multi_line_composer() {
    let mut app = make_test_app().await;
    app.chat_widget
        .apply_external_edit("first draft line\nsecond draft line".to_string());
    app.transcript_cells
        .push(history_cell("older transcript row"));

    // Six rows total: the composer needs most of the screen, so the history window shrinks
    // instead of the input surface being pushed off screen.
    let terminal = draw_owned_frame(&mut app, /*width*/ 40, /*height*/ 6);
    let screen = screen_contents(&terminal);

    assert!(
        screen.contains("first draft line"),
        "the draft stays visible on a small terminal: {screen}"
    );
    assert!(
        screen.contains("second draft line"),
        "every draft line stays visible: {screen}"
    );
}

/// Owned mode keeps history in source cells and never queues terminal rows.
#[tokio::test]
async fn owned_mode_keeps_history_in_source_cells() {
    let mut app = make_test_app().await;
    let mut tui = make_test_tui().expect("test tui");
    tui.set_owned_screen(/*owned*/ true).expect("owned screen");

    app.insert_history_cell(
        &mut tui,
        Box::new(PlainHistoryCell::new(vec![Line::from("committed turn")])),
    );
    assert_eq!(app.transcript_cells.len(), 1, "the row is kept as source");
    assert_eq!(
        tui.pending_history_line_count(),
        0,
        "nothing is written above the viewport"
    );

    // The `/clear` banner takes the same path: a source cell, not a scrollback write.
    app.queue_clear_ui_header(&mut tui);
    assert_eq!(app.transcript_cells.len(), 2);
    assert_eq!(tui.pending_history_line_count(), 0);

    // Ctrl-L and `/clear` reset the view without purging scrollback.
    app.clear_terminal_ui(&mut tui, /*redraw_header*/ false)
        .expect("clear owned ui");
    assert_eq!(tui.pending_history_line_count(), 0);
    assert!(app.transcript_view.is_following());

    tui.set_owned_screen(/*owned*/ false).expect("release");
}

/// The wheel and page keys drive the Owned viewport; Terminal mode is left untouched.
#[tokio::test]
async fn owned_transcript_scrolling_follows_and_detaches() {
    let mut app = make_test_app().await;
    for index in 0..80 {
        app.transcript_cells
            .push(history_cell(&format!("turn {index}")));
    }
    let mut tui = make_test_tui().expect("test tui");

    // Terminal mode never routes scroll input to the owned view.
    assert!(!app.owned_transcript_accepts_scroll(&tui));

    tui.set_owned_screen(/*owned*/ true).expect("owned screen");
    assert!(app.owned_transcript_accepts_scroll(&tui));

    // The viewport size is only known after a frame, so paint one first.
    draw_owned_frame(&mut app, /*width*/ 40, /*height*/ 8);
    assert!(app.transcript_view.is_following());

    app.scroll_owned_transcript(&mut tui, OwnedTranscriptScroll::Wheel(/*direction*/ -1.0));
    assert!(
        !app.transcript_view.is_following(),
        "scrolling up leaves the live tail"
    );

    // Scrolling back past the bottom reattaches to the live tail.
    app.scroll_owned_transcript(
        &mut tui,
        OwnedTranscriptScroll::Rows(app.transcript_view.page_rows() * 4),
    );
    assert!(app.transcript_view.is_following());

    app.scroll_owned_transcript(&mut tui, OwnedTranscriptScroll::Wheel(/*direction*/ -1.0));
    app.scroll_owned_transcript(&mut tui, OwnedTranscriptScroll::Latest);
    assert!(app.transcript_view.is_following());

    tui.set_owned_screen(/*owned*/ false).expect("release");
}
