//! Owned-mode (P5) lifecycle and dual-mode acceptance tests.
//!
//! P5 is about what happens *around* the transcript rather than inside it: switching threads,
//! clearing, handing the screen to an external program, and giving the terminal back unchanged.
//! The roadmap names four invariants, and this module asserts each of them at the app level:
//!
//! * both modes keep the same history, in the same order;
//! * a thread switch or a clear drops the previous view state *and* its paging gate, so the old
//!   thread's content cannot appear in the new thread's first frame;
//! * returning from an external program repaints the owned window from `transcript_cells`;
//! * pointer input never crosses the mode boundary, and releasing ownership leaves no owned residue.
//!
//! Resize, prepend and streaming anchors are asserted at the view level in
//! `transcript_view::tests` (`reading_anchor_survives_width_change`,
//! `prepended_history_keeps_reading_position`, `streaming_keeps_the_reading_anchor`); this module
//! covers the app wiring those depend on.

use super::owned_transcript::owned_terminal_with_server;
use super::owned_transcript::draw_owned_frame;
use super::owned_transcript::history_cell;
use super::owned_transcript::key;
use super::owned_transcript::mouse;
use super::owned_transcript::screen_contents;
use super::*;
use crate::app::test_support::make_test_app;
use crate::transcript_history::TranscriptHistoryState;
use crate::tui::test_support::make_test_tui;
use crossterm::event::KeyCode;
use crossterm::event::MouseEventKind;
use pretty_assertions::assert_eq;
use ratatui::text::Line;

/// Both modes render one ordered history; only *where* it goes differs.
///
/// Terminal mode hands the rows to the terminal scrollback path, Owned mode keeps them as source
/// and paints a window. The content and its chronological order must be identical either way.
#[tokio::test]
async fn both_modes_render_one_ordered_history() {
    let mut app = make_test_app().await;
    for index in 0..5 {
        app.transcript_cells
            .push(history_cell(&format!("turn {index}")));
    }

    let mut tui = make_test_tui().expect("test tui");

    // Terminal mode: the rows go to the terminal history path.
    tui.insert_history_lines(
        (0..5)
            .map(|index| Line::from(format!("turn {index}")))
            .collect(),
    );
    assert_eq!(
        tui.pending_history_line_count(),
        1,
        "terminal mode queues history rows"
    );
    tui.clear_pending_history_lines();

    // Owned mode: the *same* rows are refused — they stay in `transcript_cells` and are painted in
    // one frame instead. `set_owned_screen` takes the alternate screen immediately, so this really
    // exercises the drop path rather than a not-yet-owned session.
    tui.set_owned_screen(/*owned*/ true).expect("owned screen");
    tui.insert_history_lines(
        (0..5)
            .map(|index| Line::from(format!("turn {index}")))
            .collect(),
    );
    assert_eq!(
        tui.pending_history_line_count(),
        0,
        "owned mode writes no history rows"
    );

    // The frame has to be tall enough for all five turns (a row plus a separator each) or the
    // oldest one scrolls out of the tail-following window.
    let terminal = draw_owned_frame(&mut app, /*width*/ 40, /*height*/ 24);
    let screen = screen_contents(&terminal);
    for index in 0..5 {
        assert!(
            screen.contains(&format!("turn {index}")),
            "turn {index} is painted: {screen}"
        );
    }
    let first = screen.find("turn 0").expect("first turn on screen");
    let last = screen.find("turn 4").expect("last turn on screen");
    assert!(
        first < last,
        "history keeps its chronological order in owned mode: {screen}"
    );

    tui.set_owned_screen(/*owned*/ false).expect("release");
}

/// A thread switch drops the previous thread's cells, reading position and paging gate.
///
/// The paging gate matters most: if it survived the switch, the new thread's first frame could ask
/// the server for pages it is not missing.
#[tokio::test]
async fn thread_switch_drops_the_previous_view_and_paging_state() -> Result<()> {
    let mut app = make_test_app().await;
    // Enough turns that `PageUp` can actually leave the tail; on a transcript shorter than the
    // window there is nothing to scroll and the view stays attached.
    for index in 0..40 {
        app.transcript_cells
            .push(history_cell(&format!("previous thread turn {index}")));
    }
    app.scrollback_has_older_history = true;

    let mut tui = make_test_tui()?;
    tui.set_owned_screen(/*owned*/ true)?;
    draw_owned_frame(&mut app, /*width*/ 40, /*height*/ 10);

    // Read back and fake an in-flight page so both the position and the state have to be dropped.
    assert!(app.route_owned_transcript_key(&mut tui, key(KeyCode::PageUp)));
    assert!(!app.transcript_view.is_following());
    app.transcript_view.history = TranscriptHistoryState::Partial;
    app.deferred_history_lines = vec![Line::from("stale buffered line").into()];
    app.has_emitted_history_lines = true;

    app.reset_for_thread_switch(&mut tui)?;

    assert!(
        app.transcript_cells.is_empty(),
        "the previous thread's cells must not reach the new thread's first frame"
    );
    assert!(
        app.transcript_view.is_following(),
        "the new thread starts at its own tail"
    );
    assert_eq!(
        app.transcript_view.history,
        TranscriptHistoryState::Idle,
        "paging state is dropped with the thread"
    );
    assert!(
        !app.scrollback_has_older_history,
        "the paging gate is re-seeded by the next replay, not inherited"
    );
    assert!(app.deferred_history_lines.is_empty());
    assert!(!app.has_emitted_history_lines);
    assert!(app.overlay.is_none());

    tui.set_owned_screen(/*owned*/ false)?;
    Ok(())
}

/// The same switch in Terminal mode also discards queued terminal rows.
#[tokio::test]
async fn thread_switch_discards_queued_terminal_rows() -> Result<()> {
    let mut app = make_test_app().await;
    app.transcript_cells.push(history_cell("previous thread turn"));

    let mut tui = make_test_tui()?;
    tui.insert_history_lines(vec![Line::from("stale queued row")]);
    assert_eq!(tui.pending_history_line_count(), 1);

    app.reset_for_thread_switch(&mut tui)?;

    assert_eq!(
        tui.pending_history_line_count(),
        0,
        "a queued row must not be flushed into the new thread"
    );
    assert!(app.transcript_cells.is_empty());
    Ok(())
}

/// Clearing (`/clear` and Ctrl-L share this reset) drops the owned view and its paging gate.
#[tokio::test]
async fn clear_drops_the_owned_paging_gate_and_view_state() {
    let mut app = make_test_app().await;
    app.transcript_cells.push(history_cell("turn before clear"));
    app.scrollback_has_older_history = true;

    let mut tui = make_test_tui().expect("test tui");
    tui.set_owned_screen(/*owned*/ true).expect("owned screen");
    draw_owned_frame(&mut app, /*width*/ 40, /*height*/ 10);
    assert!(app.route_owned_transcript_key(&mut tui, key(KeyCode::PageUp)));
    app.transcript_view.history = TranscriptHistoryState::LoadingOlder;

    app.reset_app_ui_state_after_clear();

    assert!(app.transcript_cells.is_empty());
    assert!(app.transcript_view.is_following());
    assert_eq!(
        app.transcript_view.history,
        TranscriptHistoryState::Idle,
        "a clear must not leave a loading affordance behind"
    );
    assert!(
        !app.scrollback_has_older_history,
        "the next replay re-seeds the gate from the new transcript"
    );

    tui.set_owned_screen(/*owned*/ false).expect("release");
}

/// Handing the screen to an external program and coming back repaints the same owned window.
#[tokio::test]
async fn owned_window_repaints_from_cells_after_a_handoff() -> Result<()> {
    let mut app = make_test_app().await;
    for index in 0..40 {
        app.transcript_cells
            .push(history_cell(&format!("turn {index}")));
    }

    let mut tui = make_test_tui()?;
    tui.set_owned_screen(/*owned*/ true)?;
    draw_owned_frame(&mut app, /*width*/ 40, /*height*/ 10);

    // Read back so the window is anchored somewhere other than the tail.
    assert!(app.route_owned_transcript_key(&mut tui, key(KeyCode::PageUp)));
    let before = screen_contents(&draw_owned_frame(&mut app, /*width*/ 40, /*height*/ 10));

    // An external program borrows the terminal; the session keeps ownership of the mode.
    tui.leave_alt_screen_for_handoff()?;
    assert!(!tui.is_alt_screen_active());
    assert!(tui.is_owned_screen());

    // Re-entry re-establishes the screen, and the next frame repaints from the same anchor.
    tui.enter_alt_screen()?;
    let after = screen_contents(&draw_owned_frame(&mut app, /*width*/ 40, /*height*/ 10));
    assert_eq!(
        after, before,
        "the owned window is redrawn from the same reading anchor"
    );
    assert_eq!(
        tui.pending_history_line_count(),
        0,
        "no history rows are written on re-entry"
    );

    tui.set_owned_screen(/*owned*/ false)?;
    Ok(())
}

/// Releasing ownership returns the terminal and stops the owned view from answering input.
#[tokio::test]
async fn releasing_ownership_leaves_no_owned_residue() -> Result<()> {
    let mut app = make_test_app().await;
    app.transcript_cells.push(history_cell("turn"));

    let mut tui = make_test_tui()?;
    tui.set_owned_screen(/*owned*/ true)?;
    assert!(tui.is_alt_screen_active());
    assert!(app.owned_transcript_accepts_input(&tui));

    tui.set_owned_screen(/*owned*/ false)?;

    assert!(
        !tui.is_alt_screen_active(),
        "the alternate screen is returned to the shell"
    );
    assert!(!tui.is_owned_screen());
    assert!(
        !app.owned_transcript_accepts_input(&tui),
        "the owned view must stop answering input after release"
    );
    // Terminal mode accepts history rows again: that is what "no residue" means here.
    tui.insert_history_lines(vec![Line::from("terminal row")]);
    assert_eq!(tui.pending_history_line_count(), 1);
    tui.clear_pending_history_lines();
    Ok(())
}

/// The wheel scrolls the owned window and never edits the draft; in Terminal mode it is inert.
#[tokio::test]
async fn pointer_input_does_not_cross_the_mode_boundary() -> Result<()> {
    let mut app = make_test_app().await;
    for index in 0..40 {
        app.transcript_cells
            .push(history_cell(&format!("turn {index}")));
    }
    app.chat_widget
        .apply_external_edit("draft stays mine".to_string());
    let (mut tui, _terminal, mut app_server) = owned_terminal_with_server(&mut app).await?;

    // Owned: the wheel moves the reading window and leaves the composer untouched.
    app.handle_mouse_event(
        &mut tui,
        &mut app_server,
        mouse(MouseEventKind::ScrollUp, /*column*/ 5, /*row*/ 1),
    );
    assert!(
        !app.transcript_view.is_following(),
        "the wheel scrolls the owned window"
    );
    assert_eq!(
        app.chat_widget.composer_text_with_pending(),
        "draft stays mine",
        "a wheel event must never reach the composer"
    );

    // Terminal mode: the same wheel belongs to the terminal, not to the owned view. Return to the
    // tail first — with a draft present `End` goes to the composer, and the point here is the wheel,
    // not the keymap.
    app.transcript_view.jump_to_latest();
    assert!(app.transcript_view.is_following());
    tui.set_owned_screen(/*owned*/ false)?;
    app.handle_mouse_event(
        &mut tui,
        &mut app_server,
        mouse(MouseEventKind::ScrollUp, /*column*/ 5, /*row*/ 1),
    );
    assert!(
        app.transcript_view.is_following(),
        "terminal mode leaves the owned view alone"
    );
    assert_eq!(app.chat_widget.composer_text_with_pending(), "draft stays mine");
    Ok(())
}
