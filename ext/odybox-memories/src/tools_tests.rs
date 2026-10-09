//! Behavioral tests for the assistant memory tools.

use chrono::Utc;

use super::*;
use crate::ClaimKind;
use crate::Confidence;
use crate::EvidenceKind;
use crate::NewClaim;
use crate::NewEvidence;

async fn store(root: &std::path::Path, statement: &str, locator: &str) {
    let db = MemoryDb::open(root).await.expect("store");
    db.insert_claim(
        NewClaim {
            kind: ClaimKind::Preference,
            subject: "user".to_string(),
            statement: statement.to_string(),
            confidence: Confidence::High,
            scope: Some("writing".to_string()),
            decision_implication: None,
            review_due: None,
            valid_from: None,
        },
        vec![NewEvidence {
            kind: EvidenceKind::Fact,
            occurred_at: Utc::now(),
            source_thread: "t-1".to_string(),
            source_locator: locator.to_string(),
            excerpt: "用户说：回复要简短".to_string(),
        }],
    )
    .await
    .expect("claim");
}

#[tokio::test]
async fn recall_returns_each_memory_with_the_quote_behind_it() {
    let dir = tempfile::tempdir().unwrap();
    let assistant = AssistantMemoryStore::new(dir.path());
    store(dir.path(), "用户偏好简短回复", "item:7").await;

    let response = recall_response(&assistant, "回复风格", DEFAULT_RECALL_TOOL_RESULTS)
        .await
        .expect("recall");

    assert_eq!(1, response.memories.len());
    let memory = &response.memories[0];
    assert_eq!("用户偏好简短回复", memory.statement);
    assert_eq!("preference", memory.kind);
    assert_eq!("high", memory.confidence);
    assert_eq!(Some("writing".to_string()), memory.scope);
    assert_eq!(1, memory.evidence.len());
    assert_eq!("item:7", memory.evidence[0].source_locator);
    assert!(memory.evidence[0].excerpt.contains("回复要简短"));
    assert!(!response.truncated);
}

#[tokio::test]
async fn recall_says_when_it_held_back_matches() {
    let dir = tempfile::tempdir().unwrap();
    let assistant = AssistantMemoryStore::new(dir.path());
    store(dir.path(), "用户偏好简短回复", "item:1").await;
    store(dir.path(), "用户偏好简短回复，不要客套", "item:2").await;

    let response = recall_response(&assistant, "回复", 1)
        .await
        .expect("recall");

    assert_eq!(1, response.memories.len());
    assert!(response.truncated, "the model must know it saw a sample");
}

#[tokio::test]
async fn recall_of_an_unrelated_query_returns_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let assistant = AssistantMemoryStore::new(dir.path());
    store(dir.path(), "用户偏好简短回复", "item:1").await;

    let response = recall_response(&assistant, "3 加到 5", DEFAULT_RECALL_TOOL_RESULTS)
        .await
        .expect("recall");

    assert!(response.memories.is_empty());
    assert!(!response.truncated);
}
