//! Draining active-thread events must respect a frame deadline so a long backlog
//! cannot monopolize a single frame during resume.

use std::time::{Duration, Instant};

use ody_app_server_protocol::WarningNotification;
use pretty_assertions::assert_eq;

use super::*;

fn warning(message: &str) -> ThreadBufferedEvent {
    ThreadBufferedEvent::Notification(ServerNotification::Warning(WarningNotification {
        thread_id: None,
        message: message.to_string(),
    }))
}

async fn app_with_pending_warnings(warnings: usize) -> (App, tokio::sync::mpsc::Sender<ThreadBufferedEvent>) {
    let (mut app, mut app_event_rx, _op_rx) = make_test_app_with_channels().await;
    while app_event_rx.try_recv().is_ok() {}
    let (tx, rx) = tokio::sync::mpsc::channel::<ThreadBufferedEvent>(16);
    for i in 0..warnings {
        tx.try_send(warning(&format!("warning-{i}"))).expect("send warning");
    }
    app.active_thread_rx = Some(rx);
    (app, tx)
}

#[tokio::test]
async fn drain_with_past_deadline_processes_one_event_then_stops() -> Result<()> {
    let (mut app, _tx) = app_with_pending_warnings(/*warnings*/ 3).await;
    let mut tui = crate::tui::test_support::make_test_tui().expect("test tui");
    let past = Instant::now() - Duration::from_millis(1);

    app.drain_active_thread_events_until(&mut tui, past).await?;

    let remaining = app
        .active_thread_rx
        .as_ref()
        .expect("channel stays installed")
        .len();
    assert_eq!(remaining, 2);
    Ok(())
}

#[tokio::test]
async fn drain_with_future_deadline_processes_everything() -> Result<()> {
    let (mut app, _tx) = app_with_pending_warnings(/*warnings*/ 3).await;
    let mut tui = crate::tui::test_support::make_test_tui().expect("test tui");
    let future = Instant::now() + Duration::from_secs(60);

    app.drain_active_thread_events_until(&mut tui, future).await?;

    let remaining = app
        .active_thread_rx
        .as_ref()
        .expect("channel stays installed")
        .len();
    assert_eq!(remaining, 0);
    Ok(())
}
