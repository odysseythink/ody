//! Behavioral tests for the structured memory store.
//!
//! These lock the four properties the product promises: every claim carries
//! evidence, superseded claims stay queryable as-of their validity window,
//! retrieval only sees what is still valid, and the user can always remove
//! something they did not want remembered.

use chrono::TimeZone;
use chrono::Utc;

use super::*;

fn at(day: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, day, 9, 0, 0).unwrap()
}

fn claim(statement: &str) -> NewClaim {
    NewClaim {
        kind: ClaimKind::Preference,
        subject: "user".to_string(),
        statement: statement.to_string(),
        confidence: Confidence::High,
        scope: None,
        decision_implication: None,
        review_due: None,
        valid_from: None,
    }
}

fn evidence(thread: &str, locator: &str) -> NewEvidence {
    NewEvidence {
        kind: EvidenceKind::Fact,
        occurred_at: at(8),
        source_thread: thread.to_string(),
        source_locator: locator.to_string(),
        excerpt: "原话".to_string(),
    }
}

async fn db() -> MemoryDb {
    MemoryDb::open_in_memory().await.expect("in-memory db")
}

#[tokio::test]
async fn a_claim_without_evidence_is_rejected() {
    let db = db().await;

    let error = db
        .insert_claim(claim("user prefers short replies"), Vec::new())
        .await
        .expect_err("evidence is mandatory");

    assert!(
        matches!(error, MemoryDbError::MissingEvidence),
        "expected MissingEvidence, got {error:?}"
    );
    assert!(db.active_claims(10).await.unwrap().is_empty());
}

#[tokio::test]
async fn inserted_claim_is_retrievable_with_its_evidence() {
    let db = db().await;

    let id = db
        .insert_claim(
            claim("user prefers short replies"),
            vec![evidence("t-1", "item:4")],
        )
        .await
        .unwrap();

    let active = db.active_claims(10).await.unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].id, id);
    assert_eq!(active[0].statement, "user prefers short replies");

    let sources = db.evidence_for(&id).await.unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].source_thread, "t-1");
    assert_eq!(sources[0].source_locator, "item:4");
}

#[tokio::test]
async fn superseded_claim_stays_queryable_as_of_its_window() {
    let db = db().await;

    let old = db
        .insert_claim(
            NewClaim {
                valid_from: Some(at(1)),
                ..claim("manager is coachable")
            },
            vec![evidence("t-1", "item:1")],
        )
        .await
        .unwrap();
    let new = db
        .supersede(
            &old,
            NewClaim {
                confidence: Confidence::Medium,
                decision_implication: Some("switch to independent output".to_string()),
                valid_from: Some(at(8)),
                ..claim("manager is not coachable")
            },
            vec![evidence("t-2", "item:9")],
            at(8),
        )
        .await
        .unwrap();

    let active = db.active_claims(10).await.unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].id, new);

    let back_then = db.claims_valid_at("user", at(1)).await.unwrap();
    assert_eq!(back_then.len(), 1);
    assert_eq!(back_then[0].id, old);
    assert_eq!(back_then[0].statement, "manager is coachable");
    assert_eq!(back_then[0].valid_to, Some(at(8)));
    assert_eq!(back_then[0].superseded_by.as_deref(), Some(new.as_str()));
}

#[tokio::test]
async fn search_finds_claims_by_keyword() {
    let db = db().await;
    db.insert_claim(
        claim("user prefers short replies"),
        vec![evidence("t-1", "item:1")],
    )
    .await
    .unwrap();
    db.insert_claim(
        NewClaim {
            subject: "manager".to_string(),
            ..claim("manager does not hand out task plans")
        },
        vec![evidence("t-2", "item:2")],
    )
    .await
    .unwrap();

    let hits = db.search("task plans", 10).await.unwrap();

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].subject, "manager");
}

#[tokio::test]
async fn deleted_claim_leaves_retrieval_and_the_rendered_view() {
    let db = db().await;
    let id = db
        .insert_claim(
            claim("user prefers short replies"),
            vec![evidence("t-1", "item:1")],
        )
        .await
        .unwrap();

    db.delete_claim(&id).await.unwrap();

    assert!(db.active_claims(10).await.unwrap().is_empty());
    assert!(db.search("short", 10).await.unwrap().is_empty());

    let dir = tempfile::tempdir().unwrap();
    let view = dir.path().join("memory_view.json");
    db.render_view(&view).await.unwrap();
    let rendered: serde_json::Value =
        serde_json::from_str(&tokio::fs::read_to_string(&view).await.unwrap()).unwrap();
    assert_eq!(rendered["claims"].as_array().unwrap().len(), 0);
    assert_eq!(rendered["stats"]["active"].as_u64().unwrap(), 0);
}

#[tokio::test]
async fn render_view_carries_claims_the_frontend_has_to_show() {
    let db = db().await;
    db.insert_claim(
        claim("user prefers short replies"),
        vec![evidence("t-1", "item:1")],
    )
    .await
    .unwrap();

    let dir = tempfile::tempdir().unwrap();
    let view = dir.path().join("memory_view.json");
    db.render_view(&view).await.unwrap();

    let rendered: serde_json::Value =
        serde_json::from_str(&tokio::fs::read_to_string(&view).await.unwrap()).unwrap();
    let claim = &rendered["claims"][0];
    assert_eq!(claim["statement"], "user prefers short replies");
    assert_eq!(claim["kind"], "preference");
    assert_eq!(claim["subject"], "user");
    assert_eq!(claim["confidence"], "high");
    assert_eq!(claim["evidence"][0]["source_thread"], "t-1");
    assert_eq!(rendered["stats"]["active"].as_u64().unwrap(), 1);
}

#[tokio::test]
async fn pinned_claims_win_the_injection_budget() {
    let db = db().await;
    let used = db
        .insert_claim(
            claim("user prefers short replies"),
            vec![evidence("t-1", "item:1")],
        )
        .await
        .unwrap();
    let pinned = db
        .insert_claim(
            NewClaim {
                kind: ClaimKind::Correction,
                ..claim("never use canned section titles")
            },
            vec![evidence("t-2", "item:2")],
        )
        .await
        .unwrap();
    db.set_pinned(&pinned, true).await.unwrap();
    db.record_usage(std::slice::from_ref(&used)).await.unwrap();

    let picked = db.injection_candidates(1, at(22)).await.unwrap();

    assert_eq!(picked.len(), 1);
    assert_eq!(picked[0].id, pinned);
}

#[tokio::test]
async fn overdue_claims_drop_out_of_injection_but_stay_stored() {
    let db = db().await;
    let stale = db
        .insert_claim(
            NewClaim {
                review_due: Some(at(10)),
                ..claim("user prefers short replies")
            },
            vec![evidence("t-1", "item:1")],
        )
        .await
        .unwrap();

    let picked = db.injection_candidates(10, at(22)).await.unwrap();
    assert!(picked.is_empty(), "overdue claim must not be injected");

    let loaded = db.load_claim(&stale).await.unwrap();
    assert_eq!(loaded.id, stale, "but it is still stored and viewable");
}

#[tokio::test]
async fn a_subsumed_claim_can_be_closed_without_storing_a_new_one() {
    let db = db().await;
    let short = db
        .insert_claim(
            NewClaim {
                valid_from: Some(at(1)),
                ..claim("prefers short replies")
            },
            vec![evidence("t-1", "item:1")],
        )
        .await
        .unwrap();
    let covering = db
        .insert_claim(
            NewClaim {
                valid_from: Some(at(1)),
                ..claim("prefers short replies, without section headings")
            },
            vec![evidence("t-2", "item:2")],
        )
        .await
        .unwrap();

    db.mark_superseded_by(&short, &covering, at(9))
        .await
        .unwrap();

    let active = db.active_claims(10).await.unwrap();
    assert_eq!(1, active.len());
    assert_eq!(covering, active[0].id);
    let closed = db.load_claim(&short).await.unwrap();
    assert_eq!(Some(at(9)), closed.valid_to);
    assert_eq!(Some(covering), closed.superseded_by);
}
