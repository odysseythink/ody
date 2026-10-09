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

// ---------------------------------------------------------------------------
// Full-text index invariants
// ---------------------------------------------------------------------------

async fn fts_row_count(db: &MemoryDb) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM claims_fts")
        .fetch_one(&db.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn the_full_text_index_keeps_one_row_per_claim() {
    let db = db().await;
    let first = db
        .insert_claim(claim("one"), vec![evidence("t-1", "item:1")])
        .await
        .unwrap();
    let second = db
        .insert_claim(claim("two"), vec![evidence("t-1", "item:2")])
        .await
        .unwrap();

    assert_eq!(2, fts_row_count(&db).await);

    // Closing a validity window must not add a second index row, and must not
    // drop the historical one either: search filters by validity, not by index.
    db.mark_superseded_by(&first, &second, at(9)).await.unwrap();
    assert_eq!(2, fts_row_count(&db).await);
    assert_eq!(
        0,
        db.search("one", 10).await.unwrap().len(),
        "a closed claim leaves retrieval even though its index row stays"
    );
    assert_eq!(1, db.search("two", 10).await.unwrap().len());
}

#[tokio::test]
async fn a_drifted_index_is_rebuilt_when_the_store_is_reopened() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let db = MemoryDb::open(root).await.unwrap();
    db.insert_claim(claim("alpha"), vec![evidence("t-1", "item:1")])
        .await
        .unwrap();
    assert_eq!(1, fts_row_count(&db).await);

    // Simulate a store written before the triggers existed: the claim row is
    // there, the index row is gone.
    sqlx::query("DELETE FROM claims_fts")
        .execute(&db.pool)
        .await
        .unwrap();
    assert_eq!(0, fts_row_count(&db).await);
    drop(db);

    let reopened = MemoryDb::open(root).await.unwrap();
    assert_eq!(
        1,
        fts_row_count(&reopened).await,
        "opening the store must restore the index"
    );
    assert_eq!(1, reopened.search("alpha", 10).await.unwrap().len());
}

// ---------------------------------------------------------------------------
// Recall terms
// ---------------------------------------------------------------------------

#[test]
fn recall_terms_split_chinese_into_bigrams_and_keep_latin_words_whole() {
    let terms = recall_terms("偏好 concise replies");

    assert!(terms.contains(&"偏好".to_string()));
    assert!(terms.contains(&"concise".to_string()));
    assert!(terms.contains(&"replies".to_string()));
    assert!(
        !terms.contains(&"c".to_string()),
        "one letter would match nearly everything"
    );
}

#[test]
fn recall_terms_cover_a_long_message_instead_of_only_its_opening() {
    // 200 distinct characters: 199 distinct bigrams, which is more than the cap.
    let message: String = (0x4E00u32..0x4E00 + 200)
        .filter_map(char::from_u32)
        .collect();
    let terms = recall_terms(&message);

    assert_eq!(MAX_RECALL_TERMS, terms.len());
    let first = terms.first().unwrap().chars().next().unwrap();
    let last = terms.last().unwrap().chars().next().unwrap();
    assert_eq!(
        0x4E00, first as u32,
        "the first term comes from the opening"
    );
    assert!(
        (last as u32) > 0x4E00 + 150,
        "sampling must reach the end of a long message, not stop at the start"
    );
}

#[test]
fn recall_terms_of_punctuation_only_are_empty() {
    assert!(recall_terms("   ,.!?  ").is_empty());
}

// ---------------------------------------------------------------------------
// Subject canonicalization
// ---------------------------------------------------------------------------

#[tokio::test]
async fn subjects_are_stored_in_one_canonical_spelling() {
    let db = db().await;
    let spaced = db
        .insert_claim(
            NewClaim {
                subject: "  User ".to_string(),
                ..claim("prefers short replies")
            },
            vec![evidence("t-1", "item:1")],
        )
        .await
        .unwrap();
    let plain = db
        .insert_claim(
            NewClaim {
                subject: "user".to_string(),
                ..claim("prefers no section headings")
            },
            vec![evidence("t-2", "item:2")],
        )
        .await
        .unwrap();

    assert_eq!("user", db.load_claim(&spaced).await.unwrap().subject);
    assert_eq!(
        2,
        db.active_claims_for_subject("user").await.unwrap().len(),
        "both claims must be found under one spelling"
    );
    assert_eq!(2, db.active_claims_for_subject("USER").await.unwrap().len());
    let _ = plain;
}

#[tokio::test]
async fn a_store_written_before_normalization_is_rewritten_on_open() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    let db = MemoryDb::open(root).await.unwrap();
    let id = db
        .insert_claim(
            claim("prefers short replies"),
            vec![evidence("t-1", "item:1")],
        )
        .await
        .unwrap();
    // Simulate a row written by an older build.
    sqlx::query("UPDATE claims SET subject = 'User  Name' WHERE id = ?")
        .bind(&id)
        .execute(&db.pool)
        .await
        .unwrap();
    drop(db);

    let reopened = MemoryDb::open(root).await.unwrap();
    assert_eq!(
        "user name",
        reopened.load_claim(&id).await.unwrap().subject,
        "opening the store must canonicalize existing subjects"
    );
}

// ---------------------------------------------------------------------------
// Health check
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_doctor_reports_a_healthy_store() {
    let db = db().await;
    db.insert_claim(claim("first"), vec![evidence("t-1", "item:1")])
        .await
        .unwrap();
    db.insert_claim(claim("second"), vec![evidence("t-1", "item:2")])
        .await
        .unwrap();

    let report = db.doctor().await.unwrap();

    assert_eq!(2, report.claims_total);
    assert_eq!(2, report.active);
    assert_eq!(0, report.superseded);
    assert_eq!(0, report.orphan_evidence);
    assert_eq!(0, report.invalid_windows);
    assert!(report.index_in_sync);
    assert!(report.integrity_ok);
    assert!(!report.needs_attention());
}

#[tokio::test]
async fn the_doctor_notices_an_index_that_lost_a_row() {
    let db = db().await;
    db.insert_claim(claim("first"), vec![evidence("t-1", "item:1")])
        .await
        .unwrap();
    sqlx::query("DELETE FROM claims_fts")
        .execute(&db.pool)
        .await
        .unwrap();

    let report = db.doctor().await.unwrap();

    assert!(!report.index_in_sync);
    assert!(report.needs_attention());
}

#[tokio::test]
async fn the_doctor_treats_an_overdue_claim_as_attention_not_damage() {
    let db = db().await;
    db.insert_claim(
        NewClaim {
            review_due: Some(at(1)),
            ..claim("prefers short replies")
        },
        vec![evidence("t-1", "item:1")],
    )
    .await
    .unwrap();

    let report = db.doctor().await.unwrap();

    assert_eq!(1, report.overdue);
    assert!(
        !report.needs_attention(),
        "a claim awaiting review is a prompt, not a broken row"
    );
}

#[tokio::test]
async fn the_doctor_notices_evidence_whose_claim_is_gone() {
    let db = db().await;
    let id = db
        .insert_claim(claim("first"), vec![evidence("t-1", "item:1")])
        .await
        .unwrap();
    sqlx::query("DELETE FROM claims WHERE id = ?")
        .bind(&id)
        .execute(&db.pool)
        .await
        .unwrap();

    let report = db.doctor().await.unwrap();

    assert_eq!(1, report.orphan_evidence);
    assert!(report.needs_attention());
}

// ---------------------------------------------------------------------------
// Claims awaiting the user's decision
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_claim_under_review_is_stored_but_kept_out_of_retrieval() {
    let db = db().await;
    let held = db
        .insert_claim(claim("回复要简短"), vec![evidence("t-1", "item:1")])
        .await
        .unwrap();
    let disputed = db
        .insert_conflicting(
            claim("回复不要简短"),
            vec![evidence("t-2", "item:2")],
            &held,
        )
        .await
        .unwrap();

    // Stored: the newest thing the user said is never silently dropped.
    let stored = db.load_claim(&disputed).await.unwrap();
    assert_eq!(Some(held.clone()), stored.conflict_with);

    // But not retrieved, so the model is never handed both statements at once.
    let injected = db.injection_candidates(10, at(22)).await.unwrap();
    assert_eq!(1, injected.len());
    assert_eq!(held, injected[0].id);

    let recalled = db.recall_candidates("回复", 10, at(22)).await.unwrap();
    assert_eq!(1, recalled.hits.len());
    assert_eq!(held, recalled.hits[0].claim.id);
}

#[tokio::test]
async fn keeping_a_claim_closes_the_statement_it_contradicted() {
    let db = db().await;
    let held = db
        .insert_claim(claim("回复要简短"), vec![evidence("t-1", "item:1")])
        .await
        .unwrap();
    let disputed = db
        .insert_conflicting(
            claim("回复不要简短"),
            vec![evidence("t-2", "item:2")],
            &held,
        )
        .await
        .unwrap();

    db.keep_claim(&disputed).await.unwrap();

    let confirmed = db.load_claim(&disputed).await.unwrap();
    assert_eq!(None, confirmed.conflict_with);
    let closed = db.load_claim(&held).await.unwrap();
    assert!(closed.valid_to.is_some(), "the older belief is closed");
    assert_eq!(Some(disputed.clone()), closed.superseded_by);
    assert_eq!(
        1,
        db.active_claims_for_subject("user").await.unwrap().len(),
        "one belief per subject survives a ruling"
    );
}

#[tokio::test]
async fn keeping_a_settled_claim_leaves_other_claims_alone() {
    let db = db().await;
    let first = db
        .insert_claim(claim("first"), vec![evidence("t-1", "item:1")])
        .await
        .unwrap();
    let second = db
        .insert_claim(claim("second"), vec![evidence("t-1", "item:2")])
        .await
        .unwrap();

    db.keep_claim(&first).await.unwrap();

    assert!(
        db.load_claim(&second).await.unwrap().valid_to.is_none(),
        "confirming one memory must not close an unrelated one"
    );
}

#[tokio::test]
async fn the_doctor_reports_claims_awaiting_review() {
    let db = db().await;
    let held = db
        .insert_claim(claim("回复要简短"), vec![evidence("t-1", "item:1")])
        .await
        .unwrap();
    db.insert_conflicting(
        claim("回复不要简短"),
        vec![evidence("t-2", "item:2")],
        &held,
    )
    .await
    .unwrap();

    let report = db.doctor().await.unwrap();

    assert_eq!(1, report.awaiting_review);
    assert_eq!(2, report.active, "both claims are still on disk");
    assert!(
        !report.needs_attention(),
        "a pending decision is a prompt for the user, not damage"
    );
}

#[tokio::test]
async fn past_due_claims_are_listed_for_reconsideration() {
    let db = db().await;
    let due = db
        .insert_claim(
            NewClaim {
                review_due: Some(at(1)),
                ..claim("试用账号下月到期")
            },
            vec![evidence("t-1", "item:1")],
        )
        .await
        .unwrap();
    db.insert_claim(claim("回复要简短"), vec![evidence("t-1", "item:2")])
        .await
        .unwrap();

    let listed = db.review_due_claims(10, at(22)).await.unwrap();

    assert_eq!(1, listed.len());
    assert_eq!(due, listed[0].id);
}
