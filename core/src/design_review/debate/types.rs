//! Value types for the design-mode debate, plus resolution of the
//! `[design_review.debate]` config into a runnable [`DebateConfig`].

use std::time::Duration;

use ody_config::config_toml::DesignReviewDebateToml;

use crate::config::Config;
use crate::design_review::orchestrator::DESIGN_REVIEW_TIMEOUT;

/// The maximum Advocate↔Skeptic rounds we allow, regardless of config. Debate
/// cost is `1 + (2 * rounds + 1)` model calls; the cap bounds the blast radius.
const MAX_ROUNDS: u8 = 3;

/// Which side of the debate a turn speaks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DebateRole {
    Advocate,
    Skeptic,
    Judge,
}

impl DebateRole {
    /// Stable label used in the transcript and prompts.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Advocate => "Advocate",
            Self::Skeptic => "Skeptic",
            Self::Judge => "Judge",
        }
    }
}

/// One recorded turn. `content` is persona free-text (Advocate/Skeptic); the
/// Judge turn is parsed separately into findings and is not stored here.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DebateTurn {
    pub role: DebateRole,
    pub round: u8,
    pub content: String,
}

/// Ephemeral debate transcript. Discarded after the Judge synthesizes findings;
/// only the resulting `DesignReviewOutput` escapes the debate.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct DebateTranscript {
    pub turns: Vec<DebateTurn>,
}

impl DebateTranscript {
    pub(crate) fn push(&mut self, role: DebateRole, round: u8, content: String) {
        self.turns.push(DebateTurn {
            role,
            round,
            content,
        });
    }

    /// Render the transcript so far for the next persona / the judge.
    pub(crate) fn render(&self) -> String {
        self.turns
            .iter()
            .map(|t| format!("[{}]: {}", t.role.label(), t.content))
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

/// A resolved, runnable debate configuration. Built from [`Config`] only when
/// the `[design_review.debate]` table is present with `enable = true`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DebateConfig {
    /// Advocate↔Skeptic rounds, already clamped to `1..=MAX_ROUNDS`.
    pub rounds: u8,
    advocate_model: Option<String>,
    skeptic_model: Option<String>,
    judge_model: Option<String>,
    /// Fallback chain shared by every seat: `design_review_model` then
    /// `review_model` (mirrors the single-shot path).
    fallback_design_review_model: Option<String>,
    fallback_review_model: Option<String>,
    /// Per-turn wall-clock budget: the single-shot review's total budget sliced
    /// across all `2*rounds + 1` calls so the debate cannot exceed it in sum.
    pub per_turn_timeout: Duration,
}

impl DebateConfig {
    /// Resolve from a [`Config`]. Returns `None` (debate off) when the table is
    /// absent or `enable = false`. Thin extractor over [`Self::resolve`].
    pub(crate) fn from_config(config: &Config) -> Option<Self> {
        Self::resolve(
            config.design_review_debate.as_ref(),
            config.design_review_model.as_deref(),
            config.review_model.as_deref(),
        )
    }

    /// Pure resolution over just the fields debate needs — testable without a
    /// full [`Config`].
    pub(crate) fn resolve(
        debate: Option<&DesignReviewDebateToml>,
        design_review_model: Option<&str>,
        review_model: Option<&str>,
    ) -> Option<Self> {
        let debate = debate?;
        if !debate.enable {
            return None;
        }
        let rounds = debate.rounds.unwrap_or(1).clamp(1, MAX_ROUNDS);
        let calls = u32::from(2 * rounds + 1);
        Some(Self {
            rounds,
            advocate_model: debate.advocate_model.clone(),
            skeptic_model: debate.skeptic_model.clone(),
            judge_model: debate.judge_model.clone(),
            fallback_design_review_model: design_review_model.map(str::to_string),
            fallback_review_model: review_model.map(str::to_string),
            per_turn_timeout: DESIGN_REVIEW_TIMEOUT / calls,
        })
    }

    /// Resolve the model alias for a seat: the seat's own override, else
    /// `design_review_model`, else `review_model`. `None` only when nothing is
    /// configured (the caller then degrades to the single-shot review).
    pub(crate) fn model_for(&self, role: DebateRole) -> Option<String> {
        let seat = match role {
            DebateRole::Advocate => &self.advocate_model,
            DebateRole::Skeptic => &self.skeptic_model,
            DebateRole::Judge => &self.judge_model,
        };
        seat.clone()
            .or_else(|| self.fallback_design_review_model.clone())
            .or_else(|| self.fallback_review_model.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ody_config::config_toml::DesignReviewDebateToml;

    fn debate(enable: bool, rounds: Option<u8>) -> DesignReviewDebateToml {
        DesignReviewDebateToml {
            enable,
            rounds,
            advocate_model: None,
            skeptic_model: None,
            judge_model: None,
        }
    }

    #[test]
    fn disabled_or_absent_resolves_to_none() {
        assert!(DebateConfig::resolve(None, Some("m"), None).is_none());
        assert!(DebateConfig::resolve(Some(&debate(false, Some(2))), Some("m"), None).is_none());
    }

    #[test]
    fn rounds_are_clamped_into_range() {
        let zero = DebateConfig::resolve(Some(&debate(true, Some(0))), Some("m"), None).unwrap();
        assert_eq!(zero.rounds, 1);
        let huge = DebateConfig::resolve(Some(&debate(true, Some(99))), Some("m"), None).unwrap();
        assert_eq!(huge.rounds, MAX_ROUNDS);
        let default_rounds =
            DebateConfig::resolve(Some(&debate(true, None)), Some("m"), None).unwrap();
        assert_eq!(default_rounds.rounds, 1);
    }

    #[test]
    fn per_turn_timeout_slices_the_total_budget() {
        // rounds=1 -> 3 calls; rounds=3 -> 7 calls.
        let one = DebateConfig::resolve(Some(&debate(true, Some(1))), Some("m"), None).unwrap();
        assert_eq!(one.per_turn_timeout, DESIGN_REVIEW_TIMEOUT / 3);
        let three = DebateConfig::resolve(Some(&debate(true, Some(3))), Some("m"), None).unwrap();
        assert_eq!(three.per_turn_timeout, DESIGN_REVIEW_TIMEOUT / 7);
    }

    #[test]
    fn model_for_falls_back_through_the_chain() {
        // No per-seat override -> falls back to design_review_model.
        let cfg = DebateConfig::resolve(Some(&debate(true, Some(1))), Some("dr-model"), None)
            .unwrap();
        assert_eq!(cfg.model_for(DebateRole::Judge).as_deref(), Some("dr-model"));

        // Per-seat override wins for its own seat only.
        let mut d = debate(true, Some(1));
        d.judge_model = Some("glm_1/glm-5.1".to_string());
        let cfg = DebateConfig::resolve(Some(&d), Some("dr-model"), None).unwrap();
        assert_eq!(
            cfg.model_for(DebateRole::Judge).as_deref(),
            Some("glm_1/glm-5.1")
        );
        assert_eq!(
            cfg.model_for(DebateRole::Advocate).as_deref(),
            Some("dr-model")
        );
    }

    #[test]
    fn model_for_falls_back_to_review_model_when_no_design_review_model() {
        let cfg =
            DebateConfig::resolve(Some(&debate(true, Some(1))), None, Some("review-only")).unwrap();
        assert_eq!(
            cfg.model_for(DebateRole::Skeptic).as_deref(),
            Some("review-only")
        );
    }

    #[test]
    fn transcript_renders_labeled_turns() {
        let mut t = DebateTranscript::default();
        t.push(DebateRole::Advocate, 0, "for".to_string());
        t.push(DebateRole::Skeptic, 0, "against".to_string());
        assert_eq!(t.render(), "[Advocate]: for\n\n[Skeptic]: against");
    }
}
