//! Tests for reconciliation: idempotence and supersession.
//!
//! Two failure modes are locked down here. Storing the same observation twice
//! (a long conversation repeats itself), and losing an earlier belief when a
//! newer one replaces it — the earlier belief has to stay answerable, otherwise
//! "why did you think that in September" has no answer.

use chrono::TimeZone;
use chrono::Utc;

use super::*;
use crate::ClaimKind;
use crate::Confidence;
use crate::EvidenceKind;

fn at(day: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, day, 9, 0, 0).unwrap()
}

fn candidate(subject: &str, statement: &str) -> NewClaim {
    NewClaim {
        kind: ClaimKind::Entity,
        subject: subject.to_string(),
        statement: statement.to_string(),
        confidence: Confidence::High,
        scope: None,
        decision_implication: None,
        review_due: None,
        valid_from: Some(at(8)),
    }
}

fn evidence(thread: &str) -> NewEvidence {
    NewEvidence {
        kind: EvidenceKind::Fact,
        occurred_at: at(8),
        source_thread: thread.to_string(),
        source_locator: "item:2".to_string(),
        excerpt: "原文".to_string(),
    }
}

/// A candidate with no declared replacement: the judge decides.
fn pair(claim: NewClaim, evidence: Vec<NewEvidence>) -> ReconcileCandidate {
    ReconcileCandidate {
        claim,
        evidence,
        supersedes: None,
    }
}

/// Stands in for a judge that read both sides and found a contradiction.
struct AlwaysSupersedes(String);

impl ReconcileJudge for AlwaysSupersedes {
    fn decide<'a>(
        &'a self,
        _candidate: &'a NewClaim,
        _existing: &'a [Claim],
    ) -> ReconcileFuture<'a> {
        let replaced = self.0.clone();
        Box::pin(async move { ReconcileAction::Supersede { replaced } })
    }
}

#[tokio::test]
async fn an_identical_claim_is_not_stored_twice() {
    let db = MemoryDb::open_in_memory().await.unwrap();
    let judge = ConservativeJudge;

    let first = apply_claims(
        &db,
        &judge,
        vec![pair(
            candidate("manager", "does not hand out plans"),
            vec![evidence("t-1")],
        )],
    )
    .await
    .unwrap();
    let second = apply_claims(
        &db,
        &judge,
        vec![pair(
            candidate("manager", "does not hand out plans"),
            vec![evidence("t-2")],
        )],
    )
    .await
    .unwrap();

    assert_eq!(1, first.added);
    assert_eq!(1, second.skipped);
    assert_eq!(0, second.added);
    assert_eq!(1, db.active_claims(10).await.unwrap().len());
}

#[tokio::test]
async fn case_and_spacing_do_not_create_a_second_memory() {
    let db = MemoryDb::open_in_memory().await.unwrap();
    let judge = ConservativeJudge;

    apply_claims(
        &db,
        &judge,
        vec![pair(
            candidate("user", "Prefers   short replies"),
            vec![evidence("t-1")],
        )],
    )
    .await
    .unwrap();
    let second = apply_claims(
        &db,
        &judge,
        vec![pair(
            candidate("user", "prefers short replies"),
            vec![evidence("t-2")],
        )],
    )
    .await
    .unwrap();

    assert_eq!(1, second.skipped);
    assert_eq!(1, db.active_claims(10).await.unwrap().len());
}

#[tokio::test]
async fn a_different_subject_keeps_its_own_memory() {
    let db = MemoryDb::open_in_memory().await.unwrap();
    let judge = ConservativeJudge;

    apply_claims(
        &db,
        &judge,
        vec![pair(
            candidate("manager", "does not hand out plans"),
            vec![evidence("t-1")],
        )],
    )
    .await
    .unwrap();
    let second = apply_claims(
        &db,
        &judge,
        vec![pair(
            candidate("user", "does not hand out plans"),
            vec![evidence("t-2")],
        )],
    )
    .await
    .unwrap();

    assert_eq!(1, second.added);
    assert_eq!(2, db.active_claims(10).await.unwrap().len());
}

#[tokio::test]
async fn a_superseding_claim_closes_the_older_one_without_losing_it() {
    let db = MemoryDb::open_in_memory().await.unwrap();
    db.insert_claim(
        NewClaim {
            valid_from: Some(at(1)),
            ..candidate("manager", "is coachable")
        },
        vec![evidence("t-1")],
    )
    .await
    .unwrap();
    let old_id = db.active_claims(10).await.unwrap()[0].id.clone();
    let judge = AlwaysSupersedes(old_id.clone());

    let outcome = apply_claims(
        &db,
        &judge,
        vec![pair(
            candidate("manager", "is not coachable"),
            vec![evidence("t-2")],
        )],
    )
    .await
    .unwrap();

    assert_eq!(1, outcome.superseded);
    let active = db.active_claims(10).await.unwrap();
    assert_eq!(1, active.len());
    assert_eq!("is not coachable", active[0].statement);

    let back_then = db.claims_valid_at("manager", at(2)).await.unwrap();
    assert_eq!(1, back_then.len());
    assert_eq!("is coachable", back_then[0].statement);
    assert_eq!(Some(at(8)), back_then[0].valid_to);
    assert_eq!(
        Some(active[0].id.clone()),
        back_then[0].superseded_by,
        "the closed claim points at the claim that replaced it"
    );
    assert_ne!(Some(old_id), back_then[0].superseded_by);
}

#[tokio::test]
async fn stored_claims_keep_the_evidence_they_were_built_from() {
    let db = MemoryDb::open_in_memory().await.unwrap();

    apply_claims(
        &db,
        &ConservativeJudge,
        vec![pair(
            candidate("manager", "does not hand out plans"),
            vec![evidence("t-9")],
        )],
    )
    .await
    .unwrap();

    let stored = db.active_claims(10).await.unwrap().remove(0);
    let sources = db.evidence_for(&stored.id).await.unwrap();
    assert_eq!(1, sources.len());
    assert_eq!("t-9", sources[0].source_thread);
}

#[tokio::test]
async fn a_declared_supersede_closes_the_named_claim() {
    let db = MemoryDb::open_in_memory().await.unwrap();
    db.insert_claim(
        NewClaim {
            valid_from: Some(at(1)),
            ..candidate("manager", "is coachable")
        },
        vec![evidence("t-1")],
    )
    .await
    .unwrap();
    let old_id = db.active_claims(10).await.unwrap()[0].id.clone();

    let outcome = apply_claims(
        &db,
        &ConservativeJudge,
        vec![ReconcileCandidate {
            claim: candidate("manager", "is not coachable"),
            evidence: vec![evidence("t-2")],
            supersedes: Some(old_id.clone()),
        }],
    )
    .await
    .unwrap();

    assert_eq!(1, outcome.superseded);
    let active = db.active_claims(10).await.unwrap();
    assert_eq!(1, active.len());
    assert_eq!("is not coachable", active[0].statement);
    let back_then = db.claims_valid_at("manager", at(2)).await.unwrap();
    assert_eq!("is coachable", back_then[0].statement);
    assert_eq!(Some(active[0].id.clone()), back_then[0].superseded_by);
}

#[tokio::test]
async fn a_declared_supersede_naming_an_unknown_claim_falls_back_to_the_judge() {
    let db = MemoryDb::open_in_memory().await.unwrap();

    let outcome = apply_claims(
        &db,
        &ConservativeJudge,
        vec![ReconcileCandidate {
            claim: candidate("manager", "is not coachable"),
            evidence: vec![evidence("t-2")],
            supersedes: Some("claim-that-does-not-exist".to_string()),
        }],
    )
    .await
    .unwrap();

    assert_eq!(
        1, outcome.added,
        "an unknown target cannot silently drop the claim"
    );
    assert_eq!(0, outcome.superseded);
}
