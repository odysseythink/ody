//! Tests for the extraction contract: what survives from a model response.
//!
//! The rule under test throughout: the model proposes, the transcript decides.
//! A claim that cannot be traced to a line in the session never reaches the
//! store, no matter how confident the model sounds about it.

use chrono::TimeZone;
use chrono::Utc;

use super::*;
use crate::ClaimKind;
use crate::Confidence;

fn at(day: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, day, 9, 0, 0).unwrap()
}

fn items(values: &[u32]) -> BTreeSet<u32> {
    values.iter().copied().collect()
}

const GROUNDED: &str = concat!(
    r#"{"summary":"session about a manager","#,
    r#""claims":[{"kind":"entity","subject":"manager","statement":"does not hand out task plans","#,
    r#""confidence":"high","review_in_days":30,"#,
    r#""evidence":[{"item":4,"quote":"there are still several things in his hands"}]}]}"#
);

#[test]
fn a_claim_the_model_gave_no_evidence_for_is_dropped() {
    let raw = concat!(
        r#"{"summary":"s","claims":[{"kind":"preference","subject":"user","#,
        r#""statement":"likes terse answers","confidence":"high"}]}"#
    );

    let result = parse_extraction_result(raw).expect("parses");

    assert!(
        result.claims.is_empty(),
        "a claim with no citation must not survive parsing"
    );
}

#[test]
fn a_claim_citing_a_line_outside_the_transcript_is_dropped() {
    let mut result = parse_extraction_result(GROUNDED).expect("parses");

    result.retain_grounded(&items(&[1, 2, 3]));

    assert!(
        result.is_empty(),
        "evidence must point at a line that exists"
    );
}

#[test]
fn grounded_claim_keeps_its_source_kind_and_confidence() {
    let mut result = parse_extraction_result(GROUNDED).expect("parses");
    result.retain_grounded(&items(&[4]));

    let candidates = result.to_candidates("thread-1", at(22));

    assert_eq!(candidates.len(), 1);
    let candidate = &candidates[0];
    let claim = &candidate.claim;
    let evidence = &candidate.evidence;
    assert_eq!(claim.kind, ClaimKind::Entity);
    assert_eq!(claim.confidence, Confidence::High);
    assert_eq!(claim.subject, "manager");
    assert_eq!(claim.valid_from, Some(at(22)));
    assert_eq!(evidence.len(), 1);
    assert_eq!(evidence[0].source_thread, "thread-1");
    assert_eq!(evidence[0].source_locator, "item:4");
    assert!(evidence[0].excerpt.contains("things in his hands"));
}

#[test]
fn review_in_days_becomes_an_absolute_due_date() {
    let mut result = parse_extraction_result(GROUNDED).expect("parses");
    result.retain_grounded(&items(&[4]));

    let mut candidates = result.to_candidates("thread-1", at(22));
    let claim = candidates.remove(0).claim;

    assert_eq!(claim.review_due, Some(at(22) + Duration::days(30)));
}

#[test]
fn unknown_kind_and_confidence_fall_back_to_safe_values() {
    let raw = concat!(
        r#"{"summary":"s","claims":[{"kind":"vibes","subject":"user","statement":"x","#,
        r#""confidence":"very-high","evidence":[{"item":1,"quote":"q"}]}]}"#
    );

    let result = parse_extraction_result(raw).expect("parses");

    assert_eq!(result.claims.len(), 1);
    assert_eq!(
        crate::memory_db::claim_kind_from(&result.claims[0].kind),
        ClaimKind::Preference
    );
    assert_eq!(
        crate::memory_db::confidence_from(&result.claims[0].confidence),
        Confidence::High
    );
}

/// Contract: parsing tolerates the shapes a chat-completions provider returns.
///
/// This is the failure that broke the first end-to-end run: `output_schema` is
/// a Responses-API feature, so a `wire_api = "chat"` provider ignores it and
/// answers with fenced or prose-wrapped JSON.
#[test]
fn parsing_tolerates_wrapped_json() {
    let cases = [
        ("bare", GROUNDED.to_string()),
        ("fenced", format!("```json\n{GROUNDED}\n```")),
        (
            "prose around it",
            format!("Sure, here is what I found:\n\n{GROUNDED}\n\nLet me know if you need more."),
        ),
    ];

    for (label, raw) in cases {
        let parsed = parse_extraction_result(&raw)
            .unwrap_or_else(|err| panic!("case '{label}' should parse but failed: {err}"));
        assert_eq!(1, parsed.claims.len(), "case '{label}'");
    }

    let err = parse_extraction_result("I could not find anything worth remembering.")
        .expect_err("prose with no JSON must be a parse error");
    assert!(
        matches!(err, ExtractError::Parse(_)),
        "expected a Parse error, got {err:?}"
    );
}

#[test]
fn empty_verdict_parses_and_counts_as_empty() {
    let parsed =
        parse_extraction_result(r#"{"summary":"","claims":[]}"#).expect("the empty verdict parses");

    assert!(parsed.is_empty());
}

#[test]
fn a_declared_supersede_travels_with_the_candidate() {
    let raw = concat!(
        r#"{"summary":"s","claims":[{"kind":"entity","subject":"manager","#,
        r#""statement":"is not coachable","confidence":"high","supersedes":"claim-old","#,
        r#""evidence":[{"item":4,"quote":"q"}]}]}"#
    );
    let mut result = parse_extraction_result(raw).expect("parses");
    result.retain_grounded(&items(&[4]));

    let candidates = result.to_candidates("t-1", at(22));

    assert_eq!(Some("claim-old".to_string()), candidates[0].supersedes);
}

// ---------------------------------------------------------------------------
// Faithfulness to the quote
// ---------------------------------------------------------------------------

/// A claim whose statement keeps the negation the user spoke.
const KEEPS_NEGATION: &str = concat!(
    r#"{"summary":"s","claims":[{"kind":"preference","subject":"user","#,
    r#""statement":"回复不要用套话","confidence":"high","#,
    r#""evidence":[{"item":1,"quote":"回复不要用套话"}]}]}"#
);

/// The same conversation, with the negation lost on the way out.
const DROPS_NEGATION: &str = concat!(
    r#"{"summary":"s","claims":[{"kind":"preference","subject":"user","#,
    r#""statement":"回复用套话","confidence":"high","#,
    r#""evidence":[{"item":1,"quote":"回复不要用套话"}]}]}"#
);

#[test]
fn a_statement_that_keeps_the_users_negation_survives() {
    let mut result = parse_extraction_result(KEEPS_NEGATION).expect("parses");
    result.retain_grounded(&items(&[1]));

    result.retain_faithful();

    assert_eq!(1, result.claims.len());
}

#[test]
fn a_statement_that_drops_the_users_negation_is_dropped() {
    let mut result = parse_extraction_result(DROPS_NEGATION).expect("parses");
    result.retain_grounded(&items(&[1]));

    result.retain_faithful();

    assert!(
        result.is_empty(),
        "a memory that says the opposite of the quote must not be stored"
    );
}

#[test]
fn a_long_statement_is_clamped_to_the_budget() {
    let long = "字".repeat(MAX_STATEMENT_CHARS + 120);
    let raw = format!(
        r#"{{"summary":"s","claims":[{{"kind":"preference","subject":"user","statement":"{long}","confidence":"high","evidence":[{{"item":1,"quote":"原话"}}]}}]}}"#
    );
    let mut result = parse_extraction_result(&raw).expect("parses");
    result.retain_grounded(&items(&[1]));

    result.clamp_statements();

    assert_eq!(
        MAX_STATEMENT_CHARS,
        result.claims[0].statement.chars().count()
    );
}
