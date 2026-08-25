//! Contract tests for the memory prompt templates.
//!
//! The evidence-stamp rules (`[verified]` / `[user-stated]` / `[inferred]`)
//! are the machine-readable traceability contract between Phase 1 extraction
//! and Phase 2 consolidation. These tests fail if the templates drift away
//! from that contract.

use std::path::Path;

#[test]
fn stage_one_prompt_defines_the_three_evidence_stamps() {
    let prompt = crate::stage_one::PROMPT;
    for stamp in ["[verified]", "[user-stated]", "[inferred]"] {
        assert!(
            prompt.contains(stamp),
            "stage_one_system.md must define evidence stamp {stamp}"
        );
    }
}

#[test]
fn stage_one_prompt_requires_stamps_on_preference_and_knowledge_bullets() {
    let prompt = crate::stage_one::PROMPT;
    assert!(
        prompt.contains("Evidence stamps"),
        "stage_one_system.md must contain the Evidence stamps section"
    );
    // The stamp rule must apply to both bullet types that survive into
    // durable memory.
    let stamps_section = prompt
        .split("Evidence stamps")
        .nth(1)
        .expect("Evidence stamps section");
    assert!(stamps_section.contains("Preference signals"));
    assert!(stamps_section.contains("Reusable knowledge"));
}

#[test]
fn consolidation_prompt_preserves_stamps_and_weakens_on_merge() {
    let prompt = crate::build_consolidation_prompt(Path::new("/tmp/memory-root"));
    assert!(
        prompt.contains("[verified]")
            && prompt.contains("[user-stated]")
            && prompt.contains("[inferred]"),
        "consolidation.md must keep the evidence stamps"
    );
    // Merged entries must not be upgraded beyond their weakest source.
    assert!(
        prompt.contains("weakest"),
        "consolidation.md must require weakest-stamp-wins on merge"
    );
}
