//! Owned-mode (P4) pagination acceptance tests.
//!
//! The view's own rules are covered in `transcript_view::tests` and `transcript_history`; this
//! module drives the *app* against a real embedded app server so the wiring is exercised end to
//! end: resume hydrates a bounded window, reaching the top chains real `thread/turns/list` pages,
//! turns dedupe by id, a failed page stays retryable, and a foreign thread's page is dropped.
//!
//! The synthetic rollout fixture is shared with `paginated_resume` so a fixture change cannot
//! silently desynchronise the two suites.

use std::collections::HashSet;
use std::time::Duration;

use super::paginated_resume::{turn_user_texts, write_multi_turn_rollout};
use super::*;
use crate::app::test_support::make_test_app;
use crate::app::test_support::run_async_test_on_big_stack;
use crate::custom_terminal::Terminal;
use crate::test_backend::VT100Backend;
use crate::transcript_history::TranscriptHistoryState;
use crate::tui::test_support::make_test_tui;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use ody_protocol::ThreadId;
use pretty_assertions::assert_eq;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// Draw the owned frame into a throwaway terminal so the view learns its viewport.
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

/// Resume a synthetic rollout into the app's owned view.
///
/// The Owned row budget is `terminal_height * 3`; the test terminal crossterm reports is roughly a
/// real window, so the rollout must be far larger than that to leave anything pageable. Callers
/// pass a turn count well past the budget.
///
/// Returns the thread id and the live server so the caller can drive the page round-trip through
/// the real dispatch loop.
async fn resume_owned_thread(
    app: &mut App,
    app_event_rx: &mut tokio::sync::mpsc::UnboundedReceiver<AppEvent>,
    turn_count: usize,
) -> Result<(ThreadId, AppServerSession)> {
    while app_event_rx.try_recv().is_ok() {}
    let mut config = app.chat_widget.config_ref().clone();
    // Owned mode is selected by the config pair, not by the Tui: the resume path reads
    // `config.tui_fullscreen_transcript` and the alt-screen setting, so future tests need the flag.
    config.tui_fullscreen_transcript = true;
    let thread_id = ThreadId::new();
    write_multi_turn_rollout(config.ody_home.as_path(), thread_id, turn_count).await;
    let mut app_server = Box::pin(crate::start_embedded_app_server_for_picker(&config)).await?;

    let started = Box::pin(app_server.resume_thread(config, thread_id)).await?;
    assert!(
        started.turns.len() < turn_count,
        "fixture must exceed the Owned hydration budget for paging to happen: \
         hydrated {} of {turn_count}",
        started.turns.len()
    );
    Box::pin(app.enqueue_primary_thread_session(
        started.session,
        started.turns,
        /*initial_collaboration_mask*/ None,
    ))
    .await?;

    // `enqueue_primary_thread_session` replays the initial window behind a buffer: the replay only
    // reaches `transcript_cells` once the app dispatches the buffered events. A plain drain would
    // discard them and leave the view empty, so drive every queued event through `handle_event`.
    let mut tui = make_test_tui()?;
    tui.set_owned_screen(/*owned*/ true)?;
    while let Ok(event) = app_event_rx.try_recv() {
        Box::pin(app.handle_event(&mut tui, &mut app_server, event)).await?;
    }
    tui.set_owned_screen(/*owned*/ false)?;
    Ok((thread_id, app_server))
}

/// Drive the owned view to the top of what is loaded, then apply queued page events.
///
/// Mirrors the production loop: the reader navigates, the Draw branch asks the view whether it
/// wants older history, and a landed page is dispatched. Navigating with Home is what a reader
/// does; the 8-row viewport is small enough that each landing page brings the top back into range.
///
/// Returns the number of pages applied. Panics if paging does not terminate, which is exactly the
/// regression the loading state machine exists to prevent.
async fn drain_until_exhausted(
    app: &mut App,
    app_event_rx: &mut tokio::sync::mpsc::UnboundedReceiver<AppEvent>,
    tui: &mut crate::tui::Tui,
    app_server: &mut AppServerSession,
    thread_id: ThreadId,
) -> Result<usize> {
    let mut applied = 0usize;
    let mut rounds = 0usize;
    while app_server.has_older_history(thread_id) {
        // A reader reaches for the oldest content; the view decides whether that needs a page.
        app.route_owned_transcript_key(tui, key(KeyCode::Home));
        app.request_owned_history(tui, app_server);

        let Ok(event) = tokio::time::timeout(Duration::from_secs(5), app_event_rx.recv()).await
        else {
            break;
        };
        let Some(event) = event else {
            break;
        };
        if matches!(event, AppEvent::OlderThreadHistoryLoaded { .. }) {
            Box::pin(app.handle_event(tui, app_server, event)).await?;
            applied += 1;
        }
        rounds += 1;
        assert!(rounds <= 200, "owned paging must terminate");
    }
    Ok(applied)
}

/// Reaching the top chains real pages; the loaded window grows in stable order with no repeats.
#[test]
fn owned_paging_reaches_the_oldest_turn_in_stable_order() {
    run_async_test_on_big_stack(owned_paging_reaches_the_oldest_turn_in_stable_order_async);
}

async fn owned_paging_reaches_the_oldest_turn_in_stable_order_async() -> Result<()> {
    let (mut app, mut app_event_rx, _op_rx) = make_test_app_with_channels().await;
    let (thread_id, mut app_server) =
        resume_owned_thread(&mut app, &mut app_event_rx, /*turn_count*/ 200).await?;
    let mut tui = make_test_tui()?;
    tui.set_owned_screen(/*owned*/ true)?;

    // A short frame keeps the initial window small, so history genuinely has to page.
    draw_owned_frame(&mut app, /*width*/ 40, /*height*/ 8);

    let applied = drain_until_exhausted(
        &mut app,
        &mut app_event_rx,
        &mut tui,
        &mut app_server,
        thread_id,
    )
    .await?;
    assert!(applied > 0, "at least one older page must have landed");

    // Every turn reached the source store exactly once, oldest-first.
    let channel = app
        .thread_event_channels
        .get(&thread_id)
        .expect("replay channel");
    let store = channel.store.lock().await;
    assert_eq!(store.turns.len(), 200, "the full thread is loaded");
    let ids: Vec<&str> = store.turns.iter().map(|turn| turn.id.as_str()).collect();
    let unique: HashSet<&str> = ids.iter().copied().collect();
    assert_eq!(unique.len(), ids.len(), "turn ids must not repeat");
    let texts = turn_user_texts(&store.turns);
    drop(store);
    let expected: Vec<String> = (0..200).map(|i| format!("user message {i}")).collect();
    assert_eq!(texts, expected, "older pages prepend in ascending order");

    assert!(!app.scrollback_has_older_history);
    assert_eq!(
        app.transcript_view.history,
        TranscriptHistoryState::Complete,
        "the view stops asking once the server is exhausted"
    );

    tui.set_owned_screen(/*owned*/ false)?;
    Ok(())
}

/// A page that fails surfaces as `Failed`, stops auto-paging, and retries from Home.
#[test]
fn owned_failed_page_is_retryable_and_unwedges_loading() {
    run_async_test_on_big_stack(owned_failed_page_is_retryable_and_unwedges_loading_async);
}

async fn owned_failed_page_is_retryable_and_unwedges_loading_async() -> Result<()> {
    let (mut app, mut app_event_rx, _op_rx) = make_test_app_with_channels().await;
    let (thread_id, mut app_server) =
        resume_owned_thread(&mut app, &mut app_event_rx, /*turn_count*/ 200).await?;
    // Pretend a request is in flight so a failure has a loading guard to clear.
    app.scrollback_has_older_history = true;
    app.transcript_view.set_history_complete(/*complete*/ false);

    let mut tui = make_test_tui()?;
    tui.set_owned_screen(/*owned*/ true)?;
    draw_owned_frame(&mut app, /*width*/ 40, /*height*/ 8);

    Box::pin(app.handle_older_history_page(
        &mut tui,
        &mut app_server,
        thread_id,
        "stale-cursor",
        Err("network down".to_string()),
    ))
    .await?;

    assert_eq!(
        app.transcript_view.history,
        TranscriptHistoryState::Failed,
        "a failed page is visible in the view state"
    );
    assert!(
        !app.transcript_view.wants_older_history(),
        "a failed page must not be re-requested automatically"
    );

    // The frame advertises retryability rather than sitting on a loading spinner.
    let terminal = draw_owned_frame(&mut app, /*width*/ 40, /*height*/ 8);
    let screen = terminal.backend().vt100().screen().contents();
    assert!(
        screen.contains("failed to load"),
        "a failed page tells the reader: {screen}"
    );

    // Home retries: the state returns to a pageable one.
    assert!(app.route_owned_transcript_key(&mut tui, key(KeyCode::Home)));
    assert_ne!(
        app.transcript_view.history,
        TranscriptHistoryState::Failed,
        "retrying via Home clears the failed state"
    );

    tui.set_owned_screen(/*owned*/ false)?;
    Ok(())
}

/// A page for a thread the app is no longer on is dropped and does not disturb the view.
#[test]
fn owned_page_for_another_thread_is_ignored() {
    run_async_test_on_big_stack(owned_page_for_another_thread_is_ignored_async);
}

async fn owned_page_for_another_thread_is_ignored_async() -> Result<()> {
    let (mut app, mut app_event_rx, _op_rx) = make_test_app_with_channels().await;
    let (thread_id, mut app_server) =
        resume_owned_thread(&mut app, &mut app_event_rx, /*turn_count*/ 200).await?;
    let mut tui = make_test_tui()?;
    tui.set_owned_screen(/*owned*/ true)?;
    draw_owned_frame(&mut app, /*width*/ 40, /*height*/ 8);

    let before_len = app.transcript_cells.len();
    let other_thread = ThreadId::new();
    Box::pin(app.handle_older_history_page(
        &mut tui,
        &mut app_server,
        other_thread,
        "irrelevant",
        Err("should never be applied".to_string()),
    ))
    .await?;

    assert_eq!(
        app.transcript_cells.len(),
        before_len,
        "a foreign thread's page never touches the cells"
    );
    assert_ne!(
        app.transcript_view.history,
        TranscriptHistoryState::Failed,
        "and it never marks the current view failed"
    );
    assert!(
        app_server.has_older_history(thread_id),
        "the real thread's pagination is untouched"
    );

    tui.set_owned_screen(/*owned*/ false)?;
    Ok(())
}

/// The Owned budget bounds the first page even when the *terminal* row cap is disabled.
#[test]
fn owned_resume_ignores_a_disabled_terminal_cap() {
    run_async_test_on_big_stack(owned_resume_ignores_a_disabled_terminal_cap_async);
}

async fn owned_resume_ignores_a_disabled_terminal_cap_async() -> Result<()> {
    use crate::legacy_core::config::TerminalResizeReflowMaxRows;

    let app = make_test_app().await;
    let mut config = app.chat_widget.config_ref().clone();
    // Terminal mode treats this as "replay everything"; Owned must not.
    config.tui_fullscreen_transcript = true;
    config.terminal_resize_reflow.max_rows = TerminalResizeReflowMaxRows::Disabled;
    let thread_id = ThreadId::new();
    write_multi_turn_rollout(config.ody_home.as_path(), thread_id, /*turn_count*/ 40).await;
    let mut app_server = Box::pin(crate::start_embedded_app_server_for_picker(&config)).await?;

    let started = Box::pin(app_server.resume_thread(config, thread_id)).await?;
    assert!(
        started.turns.len() < 40,
        "owned hydration must stop short of the whole thread: got {}",
        started.turns.len()
    );
    assert!(
        app_server.has_older_history(thread_id),
        "the rest stays pageable"
    );
    Ok(())
}
