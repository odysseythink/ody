//! Behavioral tests for query-aware recall.
//!
//! These lock the properties the prompt depends on: recall is relevant to the
//! message being answered (not merely recent), it fits a token budget, it never
//! repeats one subject's belief five ways, and it works for Chinese queries —
//! which is the language most of this product's users write in.

use chrono::TimeZone;
use chrono::Utc;

use super::*;
use crate::EvidenceKind;
use crate::NewClaim;
use crate::NewEvidence;

fn at(day: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, day, 9, 0, 0).unwrap()
}

fn claim(id: &str, kind: ClaimKind, subject: &str, statement: &str) -> Claim {
    Claim {
        id: id.to_string(),
        kind,
        subject: subject.to_string(),
        statement: statement.to_string(),
        confidence: Confidence::High,
        scope: None,
        decision_implication: None,
        valid_from: at(1),
        valid_to: None,
        superseded_by: None,
        created_at: at(1),
        review_due: None,
        usage_count: 0,
        last_used: None,
        pinned: false,
        conflict_with: None,
    }
}

fn hit(claim: Claim, matched_terms: usize, subject_match: bool) -> crate::memory_db::ClaimHit {
    crate::memory_db::ClaimHit {
        claim,
        matched_terms,
        subject_match,
    }
}

fn matches(hits: Vec<crate::memory_db::ClaimHit>, term_count: usize) -> RecallMatches {
    RecallMatches { hits, term_count }
}

fn scored(claim: Claim, score: f64) -> ScoredClaim {
    ScoredClaim {
        claim,
        score,
        factors: ClaimFactors {
            term_match: score,
            subject: 0.0,
            confidence: 1.0,
            recency: 1.0,
            kind: 1.0,
        },
    }
}

async fn db() -> MemoryDb {
    MemoryDb::open_in_memory().await.expect("in-memory db")
}

async fn store(db: &MemoryDb, kind: ClaimKind, subject: &str, statement: &str) -> String {
    db.insert_claim(
        NewClaim {
            kind,
            subject: subject.to_string(),
            statement: statement.to_string(),
            confidence: Confidence::High,
            scope: None,
            decision_implication: None,
            review_due: None,
            valid_from: None,
        },
        vec![NewEvidence {
            kind: EvidenceKind::Fact,
            occurred_at: at(1),
            source_thread: "t-1".to_string(),
            source_locator: "item:1".to_string(),
            excerpt: "原话".to_string(),
        }],
    )
    .await
    .expect("claim is stored")
}

// ---------------------------------------------------------------------------
// Budget and estimate
// ---------------------------------------------------------------------------

#[test]
fn the_budget_follows_the_window_and_stays_between_its_bounds() {
    assert_eq!(budget_tokens_for_window(None), FALLBACK_BUDGET_TOKENS);
    assert_eq!(budget_tokens_for_window(Some(0)), FALLBACK_BUDGET_TOKENS);
    assert_eq!(budget_tokens_for_window(Some(4_000)), MIN_BUDGET_TOKENS);
    assert_eq!(budget_tokens_for_window(Some(1_000_000)), MAX_BUDGET_TOKENS);
    assert_eq!(budget_tokens_for_window(Some(20_000)), 400);
}

#[test]
fn chinese_characters_cost_more_than_latin_ones() {
    assert_eq!(estimate_tokens(""), 0);
    // Four Chinese characters: four tokens, not one.
    assert_eq!(estimate_tokens("简短回复"), 4);
    // Eight ASCII characters: two tokens.
    assert_eq!(estimate_tokens("abcdefgh"), 2);
}

// ---------------------------------------------------------------------------
// Scoring
// ---------------------------------------------------------------------------

#[test]
fn matching_more_of_the_query_scores_higher() {
    let scored = score_matches(
        matches(
            vec![
                hit(
                    claim("c-1", ClaimKind::Preference, "user", "one of two"),
                    1,
                    false,
                ),
                hit(
                    claim("c-2", ClaimKind::Preference, "user", "both of two"),
                    2,
                    false,
                ),
            ],
            2,
        ),
        at(2),
    );

    assert_eq!(scored.len(), 2);
    assert_eq!(scored[0].claim.id, "c-2");
}

#[test]
fn a_subject_match_outranks_a_statement_only_match() {
    let scored = score_matches(
        matches(
            vec![
                hit(
                    claim("c-1", ClaimKind::Preference, "user", "expo is nice"),
                    1,
                    false,
                ),
                hit(
                    claim("c-2", ClaimKind::Preference, "expo", "is nice"),
                    1,
                    true,
                ),
            ],
            1,
        ),
        at(2),
    );

    assert_eq!(scored[0].claim.id, "c-2");
    assert_eq!(scored[0].factors.subject, 1.0);
}

#[test]
fn a_correction_outranks_a_task_when_nothing_else_differs() {
    let scored = score_matches(
        matches(
            vec![
                hit(
                    claim("c-1", ClaimKind::Task, "user", "same words"),
                    1,
                    false,
                ),
                hit(
                    claim("c-2", ClaimKind::Correction, "user", "same words"),
                    1,
                    false,
                ),
            ],
            1,
        ),
        at(2),
    );

    assert_eq!(scored[0].claim.id, "c-2");
    assert!(scored[0].score > scored[1].score);
}

#[test]
fn an_older_claim_scores_lower_than_a_fresh_one() {
    let old = score_matches(
        matches(
            vec![hit(
                claim("c-1", ClaimKind::Preference, "user", "same"),
                1,
                false,
            )],
            1,
        ),
        at(1) + chrono::Duration::days((HALF_LIFE_DAYS * 3.0) as i64),
    );
    let fresh = score_matches(
        matches(
            vec![hit(
                claim("c-1", ClaimKind::Preference, "user", "same"),
                1,
                false,
            )],
            1,
        ),
        at(1),
    );

    assert!(
        fresh[0].score > old[0].score,
        "recency must decay: fresh={} old={}",
        fresh[0].score,
        old[0].score
    );
    // Three half-lives: an eighth of the recency factor.
    let expected = 2f64.powf(-3.0);
    assert!((old[0].factors.recency - expected).abs() < 0.01);
}

// ---------------------------------------------------------------------------
// Selection
// ---------------------------------------------------------------------------

#[test]
fn one_claim_per_subject_survives_selection() {
    let chosen = select_for_injection(
        vec![
            scored(
                claim("c-1", ClaimKind::Preference, "user", "first wording"),
                0.9,
            ),
            scored(
                claim("c-2", ClaimKind::Preference, "user", "second wording"),
                0.8,
            ),
            scored(
                claim("c-3", ClaimKind::Preference, "expo", "other subject"),
                0.7,
            ),
        ],
        400,
        8,
    );

    assert_eq!(
        chosen
            .iter()
            .map(|claim| claim.id.as_str())
            .collect::<Vec<_>>(),
        vec!["c-1", "c-3"],
        "the second wording of one subject must not take a slot"
    );
}

#[test]
fn pinned_claims_come_first_regardless_of_score() {
    let mut pinned = claim("c-1", ClaimKind::Preference, "user", "keep me");
    pinned.pinned = true;
    let chosen = select_for_injection(
        vec![
            scored(
                claim("c-2", ClaimKind::Correction, "expo", "higher score"),
                0.99,
            ),
            scored(pinned, 0.10),
        ],
        400,
        8,
    );

    assert_eq!(chosen[0].id, "c-1");
}

#[test]
fn the_token_budget_stops_selection() {
    let long = "字".repeat(200);
    let chosen = select_for_injection(
        vec![
            scored(claim("c-1", ClaimKind::Preference, "user", &long), 0.9),
            scored(claim("c-2", ClaimKind::Preference, "expo", &long), 0.8),
        ],
        260,
        8,
    );

    assert_eq!(chosen.len(), 1, "the second claim does not fit the budget");
}

#[test]
fn selection_keeps_one_claim_even_when_it_alone_exceeds_the_budget() {
    let long = "字".repeat(500);
    let chosen = select_for_injection(
        vec![scored(
            claim("c-1", ClaimKind::Preference, "user", &long),
            0.9,
        )],
        MIN_BUDGET_TOKENS,
        8,
    );

    assert_eq!(
        chosen.len(),
        1,
        "an empty recall is worse than an overspend"
    );
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

#[test]
fn an_empty_selection_renders_nothing_at_all() {
    assert!(render_recall_block(&[]).is_none());
}

#[test]
fn the_block_carries_kind_date_and_scope() {
    let mut pinned = claim(
        "c-1",
        ClaimKind::Correction,
        "user",
        "never use canned headers",
    );
    pinned.scope = Some("docs".to_string());
    pinned.pinned = true;

    let block = render_recall_block(&[pinned]).expect("a block");

    assert!(block.contains("[correction]"));
    assert!(block.contains("never use canned headers"));
    assert!(block.contains("since 2026-09-01"));
    assert!(block.contains("scope: docs"));
    assert!(block.contains("pinned"));
    assert!(
        block.contains("not instructions"),
        "the block must say it is context, not orders"
    );
}

// ---------------------------------------------------------------------------
// Recall against the store
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_chinese_message_recalls_a_chinese_claim() {
    let db = db().await;
    let id = store(
        &db,
        ClaimKind::Preference,
        "user",
        "用户偏好简短回复，不要用套话",
    )
    .await;

    let outcome = recall_for_turn(&db, "帮我写个介绍，回复简短一点", 400)
        .await
        .unwrap();

    assert_eq!(outcome.claims.len(), 1);
    assert_eq!(outcome.claims[0].id, id);
    assert!(outcome.rendered.contains("简短回复"));
}

#[tokio::test]
async fn an_english_message_recalls_an_english_claim() {
    let db = db().await;
    store(
        &db,
        ClaimKind::Preference,
        "user",
        "prefers concise replies without filler",
    )
    .await;

    let outcome = recall_for_turn(&db, "please keep it concise", 400)
        .await
        .unwrap();

    assert_eq!(outcome.claims.len(), 1);
}

#[tokio::test]
async fn an_unrelated_message_recalls_nothing() {
    let db = db().await;
    store(&db, ClaimKind::Preference, "user", "用户偏好简短回复").await;

    let outcome = recall_for_turn(&db, "计算 3 加到 5", 400).await.unwrap();

    assert!(outcome.is_empty());
    assert!(outcome.rendered.is_empty());
    assert_eq!(outcome.candidates, 0);
}

#[tokio::test]
async fn a_blank_message_costs_nothing() {
    let db = db().await;
    store(&db, ClaimKind::Preference, "user", "用户偏好简短回复").await;

    let outcome = recall_for_turn(&db, "   ", 400).await.unwrap();

    assert!(
        outcome.is_empty(),
        "a blank message must not recall the store"
    );
}

#[tokio::test]
async fn overdue_and_superseded_claims_stay_out_of_recall() {
    let db = db().await;
    let overdue = db
        .insert_claim(
            NewClaim {
                kind: ClaimKind::Preference,
                subject: "user".to_string(),
                statement: "用户偏好简短回复".to_string(),
                confidence: Confidence::High,
                scope: None,
                decision_implication: None,
                review_due: Some(at(1)),
                valid_from: None,
            },
            vec![NewEvidence {
                kind: EvidenceKind::Fact,
                occurred_at: at(1),
                source_thread: "t-1".to_string(),
                source_locator: "item:1".to_string(),
                excerpt: "原话".to_string(),
            }],
        )
        .await
        .unwrap();
    let superseded = store(&db, ClaimKind::Preference, "user", "用户偏好简短回复").await;
    let covering = store(
        &db,
        ClaimKind::Preference,
        "user",
        "用户偏好简短回复，且不要标题",
    )
    .await;
    db.mark_superseded_by(&superseded, &covering, Utc::now())
        .await
        .unwrap();

    let outcome = recall_for_turn(&db, "回复风格", 400).await.unwrap();
    let ids = outcome
        .claims
        .iter()
        .map(|claim| claim.id.clone())
        .collect::<Vec<_>>();

    assert!(
        !ids.contains(&overdue),
        "an overdue claim must not be recalled"
    );
    assert!(
        !ids.contains(&superseded),
        "a superseded claim must not be recalled"
    );
    assert_eq!(ids, vec![covering]);
}

#[tokio::test]
async fn recall_records_that_the_claims_were_shown() {
    let db = db().await;
    let id = store(&db, ClaimKind::Preference, "user", "用户偏好简短回复").await;

    let outcome = recall_for_turn(&db, "回复风格", 400).await.unwrap();
    assert_eq!(outcome.claims.len(), 1);

    let stored = db.load_claim(&id).await.unwrap();
    assert_eq!(
        stored.usage_count, 1,
        "exposure is recorded without reranking"
    );
    assert!(stored.last_used.is_some());
}
