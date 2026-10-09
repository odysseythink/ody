//! Tests for the extraction prompt: what the model is actually shown.
//!
//! The template tells the model to name a claim it replaces with that claim's id.
//! That instruction is only usable if the list of held claims is in the request,
//! which is what these lock down.

use super::*;

fn transcript(text: &str, known: Vec<KnownClaim>) -> Transcript {
    Transcript {
        text: text.to_string(),
        items: [1, 2].into_iter().collect(),
        known_claims: known,
    }
}

fn known(id: &str, statement: &str, review_due_passed: bool) -> KnownClaim {
    KnownClaim {
        id: id.to_string(),
        subject: "user".to_string(),
        statement: statement.to_string(),
        review_due_passed,
    }
}

#[test]
fn the_claims_already_held_reach_the_model_with_their_ids() {
    let prompt = build_extraction_input(
        &transcript(
            "[item:1] USER: hi",
            vec![known("claim-1", "回复要简短", false)],
        ),
        /*truncated*/ false,
    );

    assert!(prompt.contains("CLAIMS YOU ALREADY HOLD"));
    assert!(prompt.contains("id=claim-1"), "the id must be citable");
    assert!(prompt.contains("回复要简短"));
    assert!(prompt.contains("supersedes"));
}

#[test]
fn an_overdue_claim_is_flagged_for_reconsideration() {
    let prompt = build_extraction_input(
        &transcript(
            "[item:1] USER: hi",
            vec![known("claim-1", "试用账号下月到期", true)],
        ),
        /*truncated*/ false,
    );

    assert!(
        prompt.contains("due for review"),
        "the only place a stale belief can be closed is this pass"
    );
}

#[test]
fn a_fresh_claim_is_not_flagged() {
    let prompt = build_extraction_input(
        &transcript(
            "[item:1] USER: hi",
            vec![known("claim-1", "回复要简短", false)],
        ),
        /*truncated*/ false,
    );

    assert!(!prompt.contains("due for review"));
}

#[test]
fn a_session_with_nothing_known_omits_the_section() {
    let prompt = build_extraction_input(&transcript("[item:1] USER: hi", Vec::new()), false);

    assert!(!prompt.contains("CLAIMS YOU ALREADY HOLD"));
    assert!(prompt.contains("TRANSCRIPT BEGINS"));
}

#[test]
fn a_truncated_session_says_so() {
    let prompt = build_extraction_input(&transcript("[item:1] USER: hi", Vec::new()), true);

    assert!(prompt.contains("truncated"));
}
