//! Design-mode adversarial *debate*: a bounded Advocate→Skeptic→Judge exchange
//! that augments the single-shot design review (see the approved design at
//! `.ody-code/designs/2026-07-23-design-debate-mode.md`). Off by default; the
//! Judge turn produces the same [`crate::design_review::types::DesignReviewOutput`]
//! the single-shot critic does, so every downstream consumer is unchanged.

pub(crate) mod orchestrator;
pub(crate) mod types;
