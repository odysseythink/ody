//! Tests for the user-operation inbox: forget, keep, unpin.
//!
//! The panel's requests arrive as files, so these tests are about what survives
//! that hand-off: the request has to land, it has to be applied exactly once,
//! and a request that cannot be honoured must not sit there forever blocking the
//! queue.

use std::path::Path;

use chrono::TimeZone;
use chrono::Utc;

use super::*;
use crate::ClaimKind;
use crate::Confidence;
use crate::EvidenceKind;
use crate::NewClaim;
use crate::NewEvidence;

fn at(day: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, day, 9, 0, 0).unwrap()
}

async fn insert_claim(db: &MemoryDb, subject: &str, statement: &str) -> String {
    db.insert_claim(
        NewClaim {
            kind: ClaimKind::Entity,
            subject: subject.to_string(),
            statement: statement.to_string(),
            confidence: Confidence::High,
            scope: None,
            decision_implication: None,
            review_due: None,
            valid_from: Some(at(8)),
        },
        vec![NewEvidence {
            kind: EvidenceKind::Fact,
            occurred_at: at(8),
            source_thread: "t-1".to_string(),
            source_locator: "item:1".to_string(),
            excerpt: "原话".to_string(),
        }],
    )
    .await
    .expect("claim")
}

async fn write_op(root: &Path, name: &str, body: &str) {
    let dir = user_ops_dir(root);
    tokio::fs::create_dir_all(&dir).await.expect("ops dir");
    tokio::fs::write(dir.join(format!("{name}.json")), body)
        .await
        .expect("op file");
}

fn op_body(op: &str, claim_id: &str) -> String {
    format!("{{\"op\":\"{op}\",\"claim_id\":\"{claim_id}\"}}")
}

async fn pending_files(root: &Path) -> usize {
    let mut reader = tokio::fs::read_dir(user_ops_dir(root)).await.expect("dir");
    let mut count = 0;
    while reader.next_entry().await.expect("entry").is_some() {
        count += 1;
    }
    count
}

#[tokio::test]
async fn a_missing_inbox_is_a_noop() {
    let db = MemoryDb::open_in_memory().await.unwrap();
    let dir = tempfile::tempdir().unwrap();

    let outcome = apply_pending(&db, dir.path()).await.unwrap();

    assert_eq!(UserOpsOutcome::default(), outcome);
}

#[tokio::test]
async fn a_forget_request_removes_the_claim_from_retrieval() {
    let db = MemoryDb::open_in_memory().await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let id = insert_claim(&db, "manager", "does not hand out plans").await;
    write_op(dir.path(), "001", &op_body("delete", &id)).await;

    let outcome = apply_pending(&db, dir.path()).await.unwrap();

    assert_eq!(1, outcome.applied);
    assert!(db.active_claims(10).await.unwrap().is_empty());
    assert_eq!(
        0,
        pending_files(dir.path()).await,
        "an applied request is cleared"
    );
}

#[tokio::test]
async fn a_keep_request_puts_the_claim_first() {
    let db = MemoryDb::open_in_memory().await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    insert_claim(&db, "user", "prefers short replies").await;
    let kept = insert_claim(&db, "manager", "does not hand out plans").await;
    write_op(dir.path(), "001", &op_body("pin", &kept)).await;

    let outcome = apply_pending(&db, dir.path()).await.unwrap();

    assert_eq!(1, outcome.applied);
    let picked = db.injection_candidates(1, at(22)).await.unwrap();
    assert_eq!(kept, picked[0].id, "a kept claim wins the budget");
}

#[tokio::test]
async fn an_unpin_request_gives_the_budget_back() {
    let db = MemoryDb::open_in_memory().await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let id = insert_claim(&db, "user", "prefers short replies").await;
    db.set_pinned(&id, true).await.unwrap();
    write_op(dir.path(), "001", &op_body("unpin", &id)).await;

    apply_pending(&db, dir.path()).await.unwrap();

    assert!(!db.load_claim(&id).await.unwrap().pinned);
}

#[tokio::test]
async fn a_request_for_an_unknown_claim_is_reported_and_not_retried() {
    let db = MemoryDb::open_in_memory().await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    write_op(
        dir.path(),
        "001",
        &op_body("delete", "claim-does-not-exist"),
    )
    .await;

    let outcome = apply_pending(&db, dir.path()).await.unwrap();

    assert_eq!(0, outcome.applied);
    assert_eq!(1, outcome.failed);
    assert_eq!(
        0,
        pending_files(dir.path()).await,
        "a request that cannot be honoured must not block the queue forever"
    );
}

#[tokio::test]
async fn an_unreadable_request_is_reported_and_cleared() {
    let db = MemoryDb::open_in_memory().await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    write_op(dir.path(), "001", "{ not json").await;

    let outcome = apply_pending(&db, dir.path()).await.unwrap();

    assert_eq!(1, outcome.failed);
    assert_eq!(0, pending_files(dir.path()).await);
}
