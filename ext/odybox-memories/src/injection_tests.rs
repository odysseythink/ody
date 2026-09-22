//! Tests for the always-on memory block injected into odyBox prompts.

use chrono::TimeZone;
use chrono::Utc;

use super::*;
use crate::memory_db::ClaimKind;
use crate::memory_db::Confidence;
use crate::memory_db::EvidenceKind;
use crate::memory_db::NewClaim;
use crate::memory_db::NewEvidence;

fn at(day: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, day, 9, 0, 0).unwrap()
}

fn claim(statement: &str, valid_from: chrono::DateTime<Utc>) -> NewClaim {
    NewClaim {
        kind: ClaimKind::Preference,
        subject: "user".to_string(),
        statement: statement.to_string(),
        confidence: Confidence::High,
        scope: None,
        decision_implication: None,
        review_due: None,
        valid_from: Some(valid_from),
    }
}

fn evidence(thread: &str) -> NewEvidence {
    NewEvidence {
        kind: EvidenceKind::Fact,
        occurred_at: at(8),
        source_thread: thread.to_string(),
        source_locator: "item:1".to_string(),
        excerpt: "原话".to_string(),
    }
}

async fn db_with_claim(statement: &str, valid_from: chrono::DateTime<Utc>) -> MemoryDb {
    let db = MemoryDb::open_in_memory().await.unwrap();
    db.insert_claim(claim(statement, valid_from), vec![evidence("t-1")])
        .await
        .unwrap();
    db
}

#[tokio::test]
async fn memory_block_is_absent_when_nothing_is_stored() {
    let db = MemoryDb::open_in_memory().await.unwrap();

    assert!(memory_block_for(&db, at(22)).await.unwrap().is_none());
}

#[tokio::test]
async fn memory_block_carries_the_claim_with_its_kind_and_date() {
    let db = db_with_claim("user prefers short replies", at(8)).await;

    let block = memory_block_for(&db, at(22))
        .await
        .unwrap()
        .expect("a block");

    assert!(block.contains("user prefers short replies"), "{block}");
    assert!(block.contains("preference"), "{block}");
    assert!(block.contains("2026-09-08"), "{block}");
}

#[tokio::test]
async fn memory_block_says_the_notes_are_context_rather_than_instructions() {
    let db = db_with_claim("user prefers short replies", at(8)).await;

    let block = memory_block_for(&db, at(22)).await.unwrap().unwrap();

    assert!(block.contains("not instructions"), "{block}");
}

#[tokio::test]
async fn memory_block_omits_claims_the_user_removed() {
    let db = db_with_claim("user prefers short replies", at(8)).await;
    let id = db.active_claims(10).await.unwrap()[0].id.clone();

    db.delete_claim(&id).await.unwrap();

    assert!(memory_block_for(&db, at(22)).await.unwrap().is_none());
}

#[tokio::test]
async fn memory_block_omits_claims_whose_review_is_overdue() {
    let db = MemoryDb::open_in_memory().await.unwrap();
    db.insert_claim(
        NewClaim {
            review_due: Some(at(10)),
            ..claim("user prefers short replies", at(8))
        },
        vec![evidence("t-1")],
    )
    .await
    .unwrap();

    assert!(memory_block_for(&db, at(22)).await.unwrap().is_none());
}
