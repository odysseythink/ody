//! Tests for consolidation: folding, and — more importantly — not folding.
//!
//! The failure mode worth guarding against is a memory that quietly disappears
//! or gets rewritten. So: a claim is only folded when another current claim
//! covers it word for word, and the folded claim stays answerable afterwards.

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

fn claim(kind: ClaimKind, subject: &str, statement: &str) -> NewClaim {
    NewClaim {
        kind,
        subject: subject.to_string(),
        statement: statement.to_string(),
        confidence: Confidence::High,
        scope: None,
        decision_implication: None,
        review_due: None,
        valid_from: Some(at(1)),
    }
}

fn evidence() -> NewEvidence {
    NewEvidence {
        kind: EvidenceKind::Fact,
        occurred_at: at(1),
        source_thread: "t-1".to_string(),
        source_locator: "item:1".to_string(),
        excerpt: "原文".to_string(),
    }
}

#[tokio::test]
async fn a_shorter_claim_is_folded_into_one_that_covers_it() {
    let db = MemoryDb::open_in_memory().await.unwrap();
    let short = db
        .insert_claim(
            claim(ClaimKind::Entity, "manager", "does not hand out plans"),
            vec![evidence()],
        )
        .await
        .unwrap();
    let covering = db
        .insert_claim(
            claim(
                ClaimKind::Entity,
                "manager",
                "does not hand out plans, even when asked directly",
            ),
            vec![evidence()],
        )
        .await
        .unwrap();

    let folded = consolidate_subject(&db, "manager").await.unwrap();

    assert_eq!(1, folded);
    let active = db.active_claims(10).await.unwrap();
    assert_eq!(1, active.len());
    assert_eq!(covering, active[0].id);

    let closed = db.load_claim(&short).await.unwrap();
    assert_eq!(Some(covering), closed.superseded_by);
    let back_then = db.claims_valid_at("manager", at(2)).await.unwrap();
    assert_eq!(
        2,
        back_then.len(),
        "both beliefs stay answerable for that day"
    );
}

#[tokio::test]
async fn claims_that_differ_are_both_kept() {
    let db = MemoryDb::open_in_memory().await.unwrap();
    db.insert_claim(
        claim(ClaimKind::Entity, "manager", "does not hand out plans"),
        vec![evidence()],
    )
    .await
    .unwrap();
    db.insert_claim(
        claim(ClaimKind::Entity, "manager", "avoids answering in writing"),
        vec![evidence()],
    )
    .await
    .unwrap();

    let folded = consolidate_subject(&db, "manager").await.unwrap();

    assert_eq!(0, folded);
    assert_eq!(2, db.active_claims(10).await.unwrap().len());
}

#[tokio::test]
async fn another_subject_is_left_alone() {
    let db = MemoryDb::open_in_memory().await.unwrap();
    db.insert_claim(
        claim(ClaimKind::Entity, "manager", "does not hand out plans"),
        vec![evidence()],
    )
    .await
    .unwrap();
    db.insert_claim(
        claim(
            ClaimKind::Entity,
            "user",
            "does not hand out plans, even when asked directly",
        ),
        vec![evidence()],
    )
    .await
    .unwrap();

    let folded = consolidate_subject(&db, "manager").await.unwrap();

    assert_eq!(0, folded);
    assert_eq!(2, db.active_claims(10).await.unwrap().len());
}

#[tokio::test]
async fn a_claim_of_another_kind_is_never_folded() {
    let db = MemoryDb::open_in_memory().await.unwrap();
    db.insert_claim(
        claim(ClaimKind::Entity, "manager", "does not hand out plans"),
        vec![evidence()],
    )
    .await
    .unwrap();
    db.insert_claim(
        claim(
            ClaimKind::Preference,
            "manager",
            "does not hand out plans, even when asked directly",
        ),
        vec![evidence()],
    )
    .await
    .unwrap();

    let folded = consolidate_subject(&db, "manager").await.unwrap();

    assert_eq!(
        0, folded,
        "a preference and an observation are different things"
    );
}
