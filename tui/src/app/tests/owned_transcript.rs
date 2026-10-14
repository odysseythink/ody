//! Owned (full-screen) transcript acceptance tests.
//!
//! These cover the P2 contract: the main screen renders history and the input surface inside a
//! single frame, history stays in `transcript_cells` instead of terminal scrollback, and nothing
//! the Owned path emits purges the shell's scrollback.

use std::sync::Arc;

use super::*;
use crate::app::test_support::make_test_app;
use crate::custom_terminal::Terminal;
use crate::history_cell::PlainHistoryCell;
use crate::test_backend::VT100Backend;
use crate::tui::test_support::make_test_tui;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use crossterm::event::MouseButton;
use crossterm::event::MouseEvent;
use crossterm::event::MouseEventKind;
use pretty_assertions::assert_eq;
use ratatui::text::Line;

/// Byte sequence that purges the terminal's scrollback. Owned mode must never emit it.
pub(super) const SCROLLBACK_PURGE: &[u8] = b"\x1b[3J";

pub(super) fn history_cell(text: &str) -> Arc<dyn HistoryCell> {
    Arc::new(PlainHistoryCell::new(vec![Line::from(text.to_string())]))
}

pub(super) fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

pub(super) fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

pub(super) fn screen_contents(terminal: &Terminal<VT100Backend>) -> String {
    terminal.backend().vt100().screen().contents()
}

/// A frame big enough for a short transcript, drawn through the Owned render path.
///
/// Recorded output is scoped to the frame itself, so the no-purge assertion cannot be satisfied
/// by bytes written before the draw.
pub(super) fn draw_owned_frame(app: &mut App, width: u16, height: u16) -> Terminal<VT100Backend> {
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

/// The wheel and navigation keys drive the Owned viewport; Terminal mode is left untouched.
#[tokio::test]
async fn owned_transcript_scrolling_follows_and_detaches() -> Result<()> {
    let mut app = make_test_app().await;
    for index in 0..80 {
        app.transcript_cells
            .push(history_cell(&format!("turn {index}")));
    }
    let mut tui = make_test_tui().expect("test tui");
    let mut app_server = Box::pin(crate::start_embedded_app_server_for_picker(
        app.chat_widget.config_ref(),
    ))
    .await?;

    // Terminal mode never routes input to the owned view.
    assert!(!app.owned_transcript_accepts_input(&tui));

    tui.set_owned_screen(/*owned*/ true).expect("owned screen");
    assert!(app.owned_transcript_accepts_input(&tui));

    // The viewport size is only known after a frame, so paint one first.
    draw_owned_frame(&mut app, /*width*/ 40, /*height*/ 8);
    assert!(app.transcript_view.is_following());

    app.handle_mouse_event(
        &mut tui,
        &mut app_server,
        mouse(MouseEventKind::ScrollUp, /*column*/ 5, /*row*/ 3),
    );
    assert!(
        !app.transcript_view.is_following(),
        "scrolling up leaves the live tail"
    );

    // Scrolling back past the bottom reattaches to the live tail.
    let page = app.transcript_view.page_rows();
    for _ in 0..4 {
        assert!(app.route_owned_transcript_key(&mut tui, key(KeyCode::PageDown)));
    }
    assert!(page > 0);
    assert!(app.transcript_view.is_following());

    // Home detaches, End reattaches.
    assert!(app.route_owned_transcript_key(&mut tui, key(KeyCode::Home)));
    assert!(!app.transcript_view.is_following());
    assert!(app.route_owned_transcript_key(&mut tui, key(KeyCode::End)));
    assert!(app.transcript_view.is_following());

    // A wheel notch followed by Esc-in-reading returns to the newest content.
    app.handle_mouse_event(
        &mut tui,
        &mut app_server,
        mouse(MouseEventKind::ScrollUp, /*column*/ 5, /*row*/ 3),
    );
    assert!(!app.transcript_view.is_following());
    assert!(app.route_owned_transcript_key(&mut tui, key(KeyCode::Esc)));
    assert!(app.transcript_view.is_following());

    tui.set_owned_screen(/*owned*/ false).expect("release");
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// P3: interaction routing, priority, copy/link delivery and Ctrl+T.
// ---------------------------------------------------------------------------------------------

/// Draw the owned frame, then run one gesture through the app's routing entry points.
fn owned_terminal(app: &mut App) -> (crate::tui::Tui, Terminal<VT100Backend>) {
    let mut tui = make_test_tui().expect("test tui");
    tui.set_owned_screen(/*owned*/ true).expect("owned screen");
    let terminal = draw_owned_frame(app, /*width*/ 40, /*height*/ 10);
    (tui, terminal)
}

/// An owned screen plus an app server, for gestures that can trigger a history page request.
///
/// Mouse routing is passed through `App::handle_mouse_event`, which also asks the view whether it
/// wants older history; the server is only ever touched on that path, so these tests use a real
/// embedded one rather than a stub.
pub(super) async fn owned_terminal_with_server(
    app: &mut App,
) -> Result<(crate::tui::Tui, Terminal<VT100Backend>, AppServerSession)> {
    let app_server =
        Box::pin(crate::start_embedded_app_server_for_picker(app.chat_widget.config_ref())).await?;
    let (tui, terminal) = owned_terminal(app);
    Ok((tui, terminal, app_server))
}

/// A drag plus Enter copies the selected text through the app and reattaches to the tail.
#[tokio::test]
async fn owned_transcript_selection_copy_reaches_the_app() -> Result<()> {
    let mut app = make_test_app().await;
    app.transcript_cells.push(history_cell("alpha beta"));
    app.transcript_cells.push(history_cell("gamma delta"));
    let (mut tui, _terminal, mut app_server) = owned_terminal_with_server(&mut app).await?;

    // Rows: 0 "alpha beta", 1 separator, 2 "gamma delta".
    app.handle_mouse_event(
        &mut tui,
        &mut app_server,
        mouse(MouseEventKind::Down(MouseButton::Left), 0, 2),
    );
    app.handle_mouse_event(
        &mut tui,
        &mut app_server,
        mouse(MouseEventKind::Drag(MouseButton::Left), 5, 2),
    );
    app.handle_mouse_event(
        &mut tui,
        &mut app_server,
        mouse(MouseEventKind::Up(MouseButton::Left), 5, 2),
    );
    assert!(app.transcript_view.has_selection());

    assert!(app.route_owned_transcript_key(&mut tui, key(KeyCode::Enter)));
    assert_eq!(
        app.last_transcript_copy.as_deref(),
        Some("gamma"),
        "the app received exactly the selected characters"
    );
    assert!(!app.transcript_view.has_selection());
    assert!(
        app.transcript_view.is_following(),
        "Enter copies and returns to the newest content"
    );

    tui.set_owned_screen(/*owned*/ false).expect("release");
    Ok(())
}

/// A draft keeps every navigation key; a selection still gets the copy keys.
#[tokio::test]
async fn owned_transcript_priority_draft_then_selection() -> Result<()> {
    let mut app = make_test_app().await;
    for index in 0..30 {
        app.transcript_cells
            .push(history_cell(&format!("turn {index}")));
    }
    let (mut tui, _terminal, mut app_server) = owned_terminal_with_server(&mut app).await?;

    app.chat_widget
        .apply_external_edit("a draft that stays editable".to_string());
    assert!(
        !app.route_owned_transcript_key(&mut tui, key(KeyCode::PageDown)),
        "navigation stays with the composer while a draft is present"
    );
    assert!(app.transcript_view.is_following());

    // Selecting with the mouse still works while the draft is present, and so do its keys.
    app.handle_mouse_event(
        &mut tui,
        &mut app_server,
        mouse(MouseEventKind::Down(MouseButton::Left), 0, 0),
    );
    app.handle_mouse_event(
        &mut tui,
        &mut app_server,
        mouse(MouseEventKind::Drag(MouseButton::Left), 4, 0),
    );
    assert!(app.transcript_view.has_selection());
    assert!(
        app.route_owned_transcript_key(&mut tui, key(KeyCode::Esc)),
        "Esc clears the selection even with a draft on screen"
    );
    assert!(!app.transcript_view.has_selection());

    tui.set_owned_screen(/*owned*/ false).expect("release");
    Ok(())
}

/// A popup or approval modal keeps the transcript out of the input path.
#[tokio::test]
async fn owned_transcript_yields_to_popups() -> Result<()> {
    let mut app = make_test_app().await;
    for index in 0..30 {
        app.transcript_cells
            .push(history_cell(&format!("turn {index}")));
    }
    let (mut tui, _terminal) = owned_terminal(&mut app);
    assert!(app.owned_transcript_accepts_input(&tui));

    let keymap = app.keymap.clone();
    app.chat_widget.open_keymap_debug(&keymap);
    assert!(
        !app.owned_transcript_accepts_input(&tui),
        "an open popup owns the input"
    );
    assert!(!app.route_owned_transcript_key(&mut tui, key(KeyCode::PageDown)));

    // Mouse input is not routed either: a click must not select behind a popup.
    let mut app_server = Box::pin(crate::start_embedded_app_server_for_picker(
        app.chat_widget.config_ref(),
    ))
    .await?;
    app.handle_mouse_event(
        &mut tui,
        &mut app_server,
        mouse(MouseEventKind::Down(MouseButton::Left), 0, 0),
    );
    assert!(!app.transcript_view.has_selection());

    tui.set_owned_screen(/*owned*/ false).expect("release");
    Ok(())
}

/// A printable key is not a transcript gesture: it belongs to the composer.
#[tokio::test]
async fn owned_transcript_leaves_typing_to_the_composer() {
    let mut app = make_test_app().await;
    app.transcript_cells.push(history_cell("alpha beta"));
    let (mut tui, _terminal) = owned_terminal(&mut app);

    assert!(!app.route_owned_transcript_key(&mut tui, key(KeyCode::Char('x'))));

    tui.set_owned_screen(/*owned*/ false).expect("release");
}

/// Ctrl+T reuses the detailed overlay and keeps the reader where they were on exit.
#[tokio::test]
async fn owned_transcript_overlay_keeps_the_reading_position() {
    let mut app = make_test_app().await;
    for index in 0..40 {
        app.transcript_cells
            .push(history_cell(&format!("turn {index}")));
    }
    let (mut tui, _terminal) = owned_terminal(&mut app);

    // Read back into the history, then open and close the detailed overlay.
    assert!(app.route_owned_transcript_key(&mut tui, key(KeyCode::PageUp)));
    assert!(!app.transcript_view.is_following());

    app.open_transcript_overlay(&mut tui);
    assert!(app.overlay.is_some());
    app.close_transcript_overlay(&mut tui);

    assert!(
        app.overlay.is_none(),
        "closing the overlay returns to the owned view"
    );
    assert!(
        tui.is_owned_screen(),
        "closing the overlay must not release the owned screen"
    );
    assert!(
        !app.transcript_view.is_following(),
        "the reader keeps their place after Ctrl+T"
    );

    tui.set_owned_screen(/*owned*/ false).expect("release");
}

/// Find mode is entered from the owned view and owns typing until Esc.
#[tokio::test]
async fn owned_transcript_find_mode_owns_typing_then_closes() -> Result<()> {
    let (mut app, mut app_event_rx, _op_rx) = make_test_app_with_channels().await;
    while app_event_rx.try_recv().is_ok() {}
    let mut app_server = Box::pin(crate::start_embedded_app_server_for_picker(
        app.chat_widget.config_ref(),
    ))
    .await?;
    for index in 0..12 {
        app.transcript_cells
            .push(history_cell(&format!("turn {index} needle")));
    }
    let mut tui = make_test_tui()?;
    tui.set_owned_screen(/*owned*/ true)?;
    draw_owned_frame(&mut app, /*width*/ 40, /*height*/ 10);

    app.handle_key_event(&mut tui, &mut app_server, key(KeyCode::F(3)))
        .await;
    assert!(app.transcript_view.search_active());

    for ch in "needle".chars() {
        app.handle_key_event(&mut tui, &mut app_server, key(KeyCode::Char(ch)))
            .await;
    }
    assert_eq!(
        app.chat_widget.composer_text_with_pending(),
        "",
        "find owns typing"
    );

    // The query, its match count and the hint render on the reserved status row.
    let terminal = draw_owned_frame(&mut app, /*width*/ 40, /*height*/ 10);
    let screen = screen_contents(&terminal);
    assert!(screen.contains("Find: needle"), "{screen}");

    app.handle_key_event(&mut tui, &mut app_server, key(KeyCode::Esc))
        .await;
    assert!(!app.transcript_view.search_active());

    // Esc while merely reading still belongs to the transcript, not to backtracking.
    assert!(!app.backtrack.primed);
    tui.set_owned_screen(/*owned*/ false)?;
    Ok(())
}
