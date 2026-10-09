//! Recall quality harness: does query-aware recall actually beat the standing
//! block?
//!
//! The standing block (`injection_candidates`) is ranked by pinned/confidence/
//! recency and knows nothing about the message being answered. That is the
//! baseline this change is supposed to improve on, so the comparison is a test
//! rather than a claim: fixtures are seeded, both paths run over the same store,
//! and the metrics are asserted.
//!
//! The fixtures are small and hand-built. They do not establish production recall
//! quality; they catch a regression in the ranking, which is what they are for.
//! Add a case by adding a row here — no new machinery needed.

use chrono::TimeZone;
use chrono::Utc;

use super::*;
use crate::EvidenceKind;
use crate::INJECTION_LIMIT;
use crate::NewClaim;
use crate::NewEvidence;

/// One evaluation query and the statement it should surface.
struct EvalQuery {
    text: &'static str,
    expected: &'static str,
}

/// Claims that fill the standing block without answering any query.
///
/// They are inserted last so their `created_at` is newest: the baseline then
/// fills all twelve slots with them, which is exactly the failure mode being
/// measured.
const NOISE: &[&str] = &[
    "用户偏好深色主题",
    "用户使用 Windows 开发",
    "项目构建时长约三分钟",
    "团队每周同步一次进度",
    "用户不喜欢在周五安排会议",
    "报表导出用 CSV 格式",
    "用户有两只猫",
    "默认语言是简体中文",
    "迁移脚本需要手动回滚",
    "用户偏好小步提交",
    "日志保留三十天",
    "试用账号下月到期",
];

/// Claims that do answer a query, inserted first so they are the oldest.
const TARGETS: &[(&str, &str)] = &[
    ("user", "回复时不要用套话"),
    ("expo", "项目使用 Expo Router 做导航"),
    ("release", "部署只在周五进行"),
];

const QUERIES: &[EvalQuery] = &[
    EvalQuery {
        text: "帮我写个回复，注意风格",
        expected: "回复时不要用套话",
    },
    EvalQuery {
        text: "项目里导航是怎么配的",
        expected: "项目使用 Expo Router 做导航",
    },
    EvalQuery {
        text: "周五有空发版吗",
        expected: "部署只在周五进行",
    },
];

const BUDGET_TOKENS: usize = 400;

async fn seeded_store() -> MemoryDb {
    let db = MemoryDb::open_in_memory().await.expect("in-memory db");
    for (subject, statement) in TARGETS {
        store(&db, subject, statement).await;
    }
    for statement in NOISE {
        store(&db, "user-profile", statement).await;
    }
    db
}

async fn store(db: &MemoryDb, subject: &str, statement: &str) {
    db.insert_claim(
        NewClaim {
            kind: ClaimKind::DomainFact,
            subject: subject.to_string(),
            statement: statement.to_string(),
            confidence: Confidence::High,
            scope: None,
            decision_implication: None,
            review_due: None,
            valid_from: Some(Utc.with_ymd_and_hms(2026, 9, 1, 9, 0, 0).unwrap()),
        },
        vec![NewEvidence {
            kind: EvidenceKind::Fact,
            occurred_at: Utc::now(),
            source_thread: "t-1".to_string(),
            source_locator: "item:1".to_string(),
            excerpt: statement.to_string(),
        }],
    )
    .await
    .expect("claim");
}

/// Precision over the returned top `k`, counting hits rather than value.
fn precision_at(results: &[String], expected: &str, k: usize) -> f64 {
    let hits = results
        .iter()
        .take(k)
        .filter(|statement| statement.as_str() == expected)
        .count();
    hits as f64 / k as f64
}

/// Reciprocal rank of the single expected result: `1.0` when it is first.
fn reciprocal_rank(results: &[String], expected: &str) -> f64 {
    results
        .iter()
        .position(|statement| statement.as_str() == expected)
        .map(|index| 1.0 / (index as f64 + 1.0))
        .unwrap_or(0.0)
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

async fn baseline_ranking(db: &MemoryDb) -> Vec<String> {
    db.injection_candidates(INJECTION_LIMIT, Utc::now())
        .await
        .expect("standing block")
        .into_iter()
        .map(|claim| claim.statement)
        .collect()
}

async fn candidate_ranking(db: &MemoryDb, query: &str) -> Vec<String> {
    recall_for_turn(db, query, BUDGET_TOKENS)
        .await
        .expect("recall")
        .claims
        .into_iter()
        .map(|claim| claim.statement)
        .collect()
}

#[tokio::test]
async fn query_aware_recall_beats_the_standing_block() {
    let db = seeded_store().await;
    let baseline = baseline_ranking(&db).await;
    assert_eq!(
        INJECTION_LIMIT,
        baseline.len(),
        "the baseline is a full block"
    );

    let mut baseline_precision = Vec::new();
    let mut baseline_reciprocal = Vec::new();
    let mut candidate_precision = Vec::new();
    let mut candidate_reciprocal = Vec::new();

    for query in QUERIES {
        let candidate = candidate_ranking(&db, query.text).await;

        baseline_precision.push(precision_at(&baseline, query.expected, 5));
        baseline_reciprocal.push(reciprocal_rank(&baseline, query.expected));
        candidate_precision.push(precision_at(&candidate, query.expected, 5));
        candidate_reciprocal.push(reciprocal_rank(&candidate, query.expected));
    }

    let baseline_p5 = mean(&baseline_precision);
    let candidate_p5 = mean(&candidate_precision);
    let baseline_mrr = mean(&baseline_reciprocal);
    let candidate_mrr = mean(&candidate_reciprocal);

    // The baseline cannot answer any of these: its slots are taken by newer,
    // higher-confidence claims that have nothing to do with the message.
    assert_eq!(0.0, baseline_mrr, "baseline MRR: {baseline_mrr}");
    assert_eq!(0.0, baseline_p5, "baseline P@5: {baseline_p5}");

    // Recall must find each target and rank it first.
    assert_eq!(
        1.0, candidate_mrr,
        "candidate MRR: {candidate_mrr} (P@5: {candidate_p5})"
    );
    assert!(
        candidate_p5 > baseline_p5,
        "candidate P@5 {candidate_p5} must beat baseline {baseline_p5}"
    );
}

#[tokio::test]
async fn recall_stays_silent_for_a_message_the_store_cannot_help_with() {
    let db = seeded_store().await;

    let candidate = candidate_ranking(&db, "把 12 和 30 相加").await;

    assert!(
        candidate.is_empty(),
        "no query terms means no prompt cost, got {candidate:?}"
    );
}
