//! Tests for the run counters: they have to survive a restart, a damaged file,
//! and an empty machine — otherwise the numbers cannot be trusted later.

use super::*;

fn event(added: usize, superseded: usize) -> PassEvent {
    PassEvent {
        at: "2026-09-22T10:00:00+00:00".to_string(),
        added,
        superseded,
        ..PassEvent::default()
    }
}

#[tokio::test]
async fn each_pass_appends_one_parseable_line() {
    let dir = tempfile::tempdir().unwrap();

    append_event(dir.path(), &event(2, 0)).await.unwrap();
    append_event(dir.path(), &event(1, 1)).await.unwrap();

    let raw = tokio::fs::read_to_string(events_path(dir.path()))
        .await
        .unwrap();
    let lines = raw.lines().collect::<Vec<_>>();
    assert_eq!(2, lines.len());
    let first: PassEvent = serde_json::from_str(lines[0]).expect("first line parses");
    let second: PassEvent = serde_json::from_str(lines[1]).expect("second line parses");
    assert_eq!(2, first.added);
    assert_eq!(1, second.superseded);
}

#[tokio::test]
async fn the_summary_accumulates_across_passes() {
    let dir = tempfile::tempdir().unwrap();

    update_summary(dir.path(), &event(2, 0), 2).await.unwrap();
    let summary = update_summary(dir.path(), &event(1, 3), 4).await.unwrap();

    assert_eq!(2, summary.passes);
    assert_eq!(3, summary.claims_added);
    assert_eq!(3, summary.claims_superseded);
    assert_eq!(4, summary.claims_in_store);
    assert_eq!(
        Some("2026-09-22T10:00:00+00:00".to_string()),
        summary.last_pass_at
    );
    let reread = read_summary(dir.path()).await;
    assert_eq!(summary, reread, "the summary survives a restart");
}

#[tokio::test]
async fn a_missing_summary_reads_as_empty_counters() {
    let dir = tempfile::tempdir().unwrap();

    let summary = read_summary(dir.path()).await;

    assert_eq!(MemorySummary::default(), summary);
}

#[tokio::test]
async fn a_damaged_summary_does_not_break_the_next_pass() {
    let dir = tempfile::tempdir().unwrap();
    tokio::fs::write(summary_path(dir.path()), "{ not json")
        .await
        .unwrap();

    let summary = update_summary(dir.path(), &event(1, 0), 1).await.unwrap();

    assert_eq!(
        1, summary.passes,
        "a damaged file restarts the counters instead of failing the pass"
    );
    assert_eq!(1, summary.claims_added);
}

#[tokio::test]
async fn dropped_candidates_and_store_failures_are_counted() {
    let dir = tempfile::tempdir().unwrap();
    let mut pass = event(1, 0);
    pass.dropped_ungrounded = 5;
    pass.store_failed = 2;
    pass.failed = 1;
    pass.injected_claims = 3;

    let summary = update_summary(dir.path(), &pass, 7).await.unwrap();

    assert_eq!(5, summary.claims_dropped_ungrounded);
    assert_eq!(3, summary.store_failures);
    assert_eq!(3, summary.last_injected_claims);
}
