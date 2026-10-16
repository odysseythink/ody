//! Tests for Owned/Terminal transcript mode and alternate-screen lifecycle.

use std::io;
use std::sync::atomic::Ordering;

use super::test_support::make_test_tui;
#[cfg(unix)]
use crate::tui::job_control::PreparedResumeAction;
use ratatui::text::Line;

fn mouse_capture_active(tui: &super::Tui) -> bool {
    tui.mouse_capture_active.load(Ordering::Relaxed)
}

#[tokio::test(flavor = "current_thread")]
async fn enter_alt_screen_is_idempotent() -> io::Result<()> {
    let mut tui = make_test_tui()?;
    assert!(!tui.is_alt_screen_active());

    tui.enter_alt_screen()?;
    assert!(tui.is_alt_screen_active());
    let viewport = tui.terminal.viewport_area;

    tui.enter_alt_screen()?;
    assert!(tui.is_alt_screen_active());
    assert_eq!(tui.terminal.viewport_area, viewport);

    tui.leave_alt_screen_for_handoff()?;
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn leave_alt_screen_is_idempotent() -> io::Result<()> {
    let mut tui = make_test_tui()?;
    tui.enter_alt_screen()?;
    tui.leave_alt_screen()?;
    assert!(!tui.is_alt_screen_active());

    tui.leave_alt_screen()?;
    assert!(!tui.is_alt_screen_active());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn closing_overlay_retains_owned_screen() -> io::Result<()> {
    let mut tui = make_test_tui()?;
    tui.set_owned_screen(/*owned*/ true)?;
    assert!(tui.is_owned_screen());
    assert!(tui.is_alt_screen_active());

    // Opening an overlay on an owned screen is a no-op for screen state.
    tui.enter_alt_screen()?;
    assert!(tui.is_alt_screen_active());

    // Closing the overlay must not leave the alternate screen.
    tui.leave_alt_screen()?;
    assert!(tui.is_alt_screen_active());
    assert!(tui.is_owned_screen());

    tui.set_owned_screen(/*owned*/ false)?;
    assert!(!tui.is_alt_screen_active());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn releasing_ownership_leaves_alt_screen() -> io::Result<()> {
    let mut tui = make_test_tui()?;
    tui.set_owned_screen(/*owned*/ true)?;
    assert!(tui.is_alt_screen_active());

    tui.set_owned_screen(/*owned*/ false)?;
    assert!(!tui.is_owned_screen());
    assert!(!tui.is_alt_screen_active());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn disabling_alt_screen_releases_owned_screen() -> io::Result<()> {
    let mut tui = make_test_tui()?;
    tui.set_owned_screen(/*owned*/ true)?;
    assert!(tui.is_alt_screen_active());

    tui.set_alt_screen_enabled(/*enabled*/ false);
    assert!(!tui.is_owned_screen());
    assert!(!tui.is_alt_screen_active());

    // Ownership requests degrade to Terminal mode while alt screen is disabled.
    tui.prepare_owned_screen(/*owned*/ true)?;
    assert!(!tui.is_owned_screen());
    assert!(!tui.is_alt_screen_active());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn prepare_owned_screen_defers_entry_until_first_draw() -> io::Result<()> {
    let mut tui = make_test_tui()?;
    tui.prepare_owned_screen(/*owned*/ true)?;
    // Ownership is resolved at launch, but the screen is entered lazily on the
    // first synchronized frame.
    assert!(tui.is_owned_screen());
    assert!(!tui.is_alt_screen_active());

    tui.draw(24, |_| {})?;
    assert!(tui.is_alt_screen_active());
    assert!(tui.is_owned_screen());

    tui.set_owned_screen(/*owned*/ false)?;
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn owned_screen_reenters_after_external_handoff() -> io::Result<()> {
    let mut tui = make_test_tui()?;
    tui.set_owned_screen(/*owned*/ true)?;
    assert!(tui.is_alt_screen_active());

    // External program/suspend yields the screen without giving up ownership.
    tui.leave_alt_screen_for_handoff()?;
    assert!(!tui.is_alt_screen_active());
    assert!(tui.is_owned_screen());

    // The next draw re-enters the owned screen.
    tui.draw(24, |_| {})?;
    assert!(tui.is_alt_screen_active());

    tui.set_owned_screen(/*owned*/ false)?;
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn active_screen_holds_pointer_capture() -> io::Result<()> {
    let mut tui = make_test_tui()?;
    assert!(!mouse_capture_active(&tui));

    tui.enter_alt_screen()?;
    assert!(mouse_capture_active(&tui));

    tui.leave_alt_screen_for_handoff()?;
    assert!(!mouse_capture_active(&tui));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn owned_overlay_close_keeps_pointer_capture() -> io::Result<()> {
    let mut tui = make_test_tui()?;
    tui.set_owned_screen(/*owned*/ true)?;
    assert!(mouse_capture_active(&tui));

    // Closing an overlay on an owned screen keeps the screen and its input modes.
    tui.leave_alt_screen()?;
    assert!(tui.is_alt_screen_active());
    assert!(mouse_capture_active(&tui));

    tui.set_owned_screen(/*owned*/ false)?;
    assert!(!mouse_capture_active(&tui));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn handoff_releases_pointer_capture_without_losing_ownership() -> io::Result<()> {
    let mut tui = make_test_tui()?;
    tui.set_owned_screen(/*owned*/ true)?;
    assert!(mouse_capture_active(&tui));

    // External editor/suspend handoff releases both the screen and its input modes.
    tui.leave_alt_screen_for_handoff()?;
    assert!(!tui.is_alt_screen_active());
    assert!(!mouse_capture_active(&tui));
    assert!(tui.is_owned_screen());

    // Re-entry restores pointer capture.
    tui.enter_alt_screen()?;
    assert!(mouse_capture_active(&tui));

    tui.set_owned_screen(/*owned*/ false)?;
    Ok(())
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn resume_action_restores_alt_screen_with_pointer_capture() -> io::Result<()> {
    let mut tui = make_test_tui()?;
    // Simulate the state right after SIGTSTP: screen left, capture released,
    // resume intent recorded as RestoreAlt.
    tui.mouse_capture_active.store(false, Ordering::Relaxed);

    // The resume action sizes the viewport from the terminal's own size, so derive the
    // expectation from the terminal instead of assuming the fixture's 80x24.
    let size = tui.terminal.size()?;
    let resume = PreparedResumeAction::RestoreAltScreen;
    resume.apply(&mut tui.terminal, &tui.mouse_capture_active)?;

    assert!(mouse_capture_active(&tui));
    assert_eq!(
        tui.terminal.viewport_area,
        ratatui::layout::Rect::new(0, 0, size.width, size.height)
    );

    // Clean up the screen state the apply left behind.
    tui.alt_screen_active.store(true, Ordering::Relaxed);
    tui.leave_alt_screen_for_handoff()?;
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn nested_overlay_open_is_idempotent_and_close_leaves_once() -> io::Result<()> {
    let mut tui = make_test_tui()?;

    // Opening a second overlay while one is active does not re-enter the screen
    // or change input modes.
    tui.enter_alt_screen()?;
    tui.enter_alt_screen()?;
    assert!(tui.is_alt_screen_active());
    assert!(mouse_capture_active(&tui));

    // Terminal mode has a single-screen model: closing the overlay leaves the screen.
    tui.leave_alt_screen()?;
    assert!(!tui.is_alt_screen_active());
    assert!(!mouse_capture_active(&tui));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn terminal_mode_editor_handoff_and_reentry() -> io::Result<()> {
    let mut tui = make_test_tui()?;

    // Overlay open in Terminal mode.
    tui.enter_alt_screen()?;
    assert!(mouse_capture_active(&tui));

    // External editor handoff releases the screen and input modes.
    tui.leave_alt_screen_for_handoff()?;
    assert!(!tui.is_alt_screen_active());
    assert!(!mouse_capture_active(&tui));

    // Returning from the editor re-enters and re-applies pointer capture.
    tui.enter_alt_screen()?;
    assert!(tui.is_alt_screen_active());
    assert!(mouse_capture_active(&tui));

    tui.leave_alt_screen()?;
    assert!(!tui.is_alt_screen_active());
    Ok(())
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn resume_realign_viewport_keeps_pointer_capture_off() -> io::Result<()> {
    let mut tui = make_test_tui()?;

    // Inline (non-alt-screen) suspend resumes with RealignViewport: no screen
    // transition and no pointer capture.
    let resume = PreparedResumeAction::RealignViewport(ratatui::layout::Rect::new(
        0, 20, 80, 4,
    ));
    resume.apply(&mut tui.terminal, &tui.mouse_capture_active)?;

    assert!(!mouse_capture_active(&tui));
    assert!(!tui.is_alt_screen_active());
    assert_eq!(
        tui.terminal.viewport_area,
        ratatui::layout::Rect::new(0, 20, 80, 4)
    );
    Ok(())
}

/// Owned sessions paint history themselves, so nothing may be queued above the viewport.
#[tokio::test(flavor = "current_thread")]
async fn owned_screen_never_queues_history_rows() -> io::Result<()> {
    let mut tui = make_test_tui()?;
    tui.set_owned_screen(/*owned*/ true)?;
    assert!(tui.is_alt_screen_active());

    tui.insert_history_lines(vec![Line::from("first committed turn")]);
    assert_eq!(
        tui.pending_history_line_count(),
        0,
        "an owned screen has no viewport history to write above"
    );

    // Handing the screen to an external program puts the session back on the main screen,
    // where inserting history rows above an inline viewport is correct again.
    tui.leave_alt_screen_for_handoff()?;
    tui.insert_history_lines(vec![Line::from("editor handoff notice")]);
    assert_eq!(
        tui.pending_history_line_count(),
        1,
        "the main screen still accepts history rows"
    );

    // Releasing ownership keeps the terminal history path in use.
    tui.clear_pending_history_lines();
    tui.set_owned_screen(/*owned*/ false)?;
    tui.insert_history_lines(vec![Line::from("released session")]);
    assert_eq!(tui.pending_history_line_count(), 1);
    Ok(())
}
