//! Behavioral tests for turn-scoped injection.

use chrono::TimeZone;
use chrono::Utc;
use ody_protocol::user_input::UserInput;

use super::*;
use crate::ClaimKind;
use crate::Confidence;
use crate::EvidenceKind;
use crate::NewClaim;
use crate::NewEvidence;

fn text(value: &str) -> UserInput {
    UserInput::Text {
        text: value.to_string(),
        text_elements: Vec::new(),
    }
}

fn image() -> UserInput {
    UserInput::Image {
        image_url: "data:image/png;base64,AAAA".to_string(),
        detail: None,
    }
}

async fn store_preference(root: &std::path::Path, statement: &str) {
    let db = MemoryDb::open(root).await.unwrap();
    db.insert_claim(
        NewClaim {
            kind: ClaimKind::Preference,
            subject: "user".to_string(),
            statement: statement.to_string(),
            confidence: Confidence::High,
            scope: None,
            decision_implication: None,
            review_due: None,
            valid_from: Some(Utc.with_ymd_and_hms(2026, 9, 1, 9, 0, 0).unwrap()),
        },
        vec![NewEvidence {
            kind: EvidenceKind::Fact,
            occurred_at: Utc.with_ymd_and_hms(2026, 9, 1, 9, 0, 0).unwrap(),
            source_thread: "t-1".to_string(),
            source_locator: "item:1".to_string(),
            excerpt: "原话".to_string(),
        }],
    )
    .await
    .unwrap();
}

#[test]
fn the_query_comes_from_the_text_parts() {
    let query = query_from_user_input(&[image(), text("回复要简短")]);

    assert_eq!(query.as_deref(), Some("回复要简短"));
}

#[test]
fn several_text_parts_are_joined_into_one_query() {
    let query = query_from_user_input(&[text("回复要简短"), image(), text("不要客套")]);

    assert_eq!(query.as_deref(), Some("回复要简短 不要客套"));
}

#[test]
fn a_message_without_text_has_no_query() {
    assert_eq!(query_from_user_input(&[image()]), None);
    assert_eq!(query_from_user_input(&[text("   ")]), None);
    assert_eq!(query_from_user_input(&[]), None);
}

#[test]
fn a_long_message_keeps_its_opening_and_its_ending() {
    let head = "开".repeat(300);
    let tail = "尾".repeat(300);
    let query = query_from_user_input(&[text(&format!("{head}{tail}"))]).unwrap();

    assert_eq!(MAX_QUERY_CHARS + 1, query.chars().count());
    assert!(query.starts_with("开"));
    assert!(query.ends_with("尾"));
}

#[test]
fn the_fragment_marks_its_own_boundaries() {
    let fragment = AssistantMemoryFragment::new("## Memory recalled for this message\n");

    assert_eq!("user", fragment.role());
    assert_eq!("<assistant_memory>", fragment.markers().0);
    assert!(fragment.render().starts_with("<assistant_memory>"));
    assert!(fragment.render().ends_with("</assistant_memory>"));
}

#[tokio::test]
async fn a_relevant_turn_receives_the_recalled_claims() {
    let dir = tempfile::tempdir().unwrap();
    store_preference(dir.path(), "用户偏好简短回复，不要用套话").await;

    let fragment =
        turn_memory_fragment(dir.path(), &[text("帮我写介绍，回复简短点")], Some(128_000))
            .await
            .expect("the past preference is relevant to this request");

    assert!(fragment.block().contains("简短回复"));
    assert!(fragment.block().contains("not instructions"));
}

#[tokio::test]
async fn an_unrelated_turn_injects_nothing() {
    let dir = tempfile::tempdir().unwrap();
    store_preference(dir.path(), "用户偏好简短回复").await;

    let fragment =
        turn_memory_fragment(dir.path(), &[text("3 加到 5 等于多少")], Some(128_000)).await;

    assert!(fragment.is_none(), "an unrelated turn must stay clean");
}

#[tokio::test]
async fn a_missing_store_is_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("not-created-yet");

    let fragment = turn_memory_fragment(&missing, &[text("hello")], None).await;

    assert!(fragment.is_none());
}
