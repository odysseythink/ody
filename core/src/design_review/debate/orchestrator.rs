//! Runs the bounded Advocate→Skeptic→Judge debate. Each turn is one
//! [`run_one_shot_review`] sub-session (single-shot, no exploration tools,
//! `ReasoningEffort::None` — mirroring the single-shot critic). The Judge turn
//! is parsed into the same [`DesignReviewOutput`] the single-shot path returns;
//! every other consumer downstream is unchanged. Any turn that yields no output
//! (empty / timeout / cancel), or a seat with no configured model, aborts the
//! debate with [`DesignReviewError::Degraded`] so the caller falls back to the
//! single-shot review — the debate can only add findings, never make finalize
//! worse.

use std::sync::Arc;

use ody_protocol::model_metadata::ReasoningEffort;
use ody_protocol::protocol::ReviewOutputEvent;
use ody_protocol::user_input::UserInput;
use tokio_util::sync::CancellationToken;

use crate::design_review::debate::types::DebateConfig;
use crate::design_review::debate::types::DebateRole;
use crate::design_review::debate::types::DebateTranscript;
use crate::design_review::prompt::Refutation;
use crate::design_review::prompt::build_advocate_prompt;
use crate::design_review::prompt::build_judge_prompt;
use crate::design_review::prompt::build_skeptic_prompt;
use crate::design_review::prompt::parse_judge_output;
use crate::design_review::types::DesignReviewError;
use crate::design_review::types::DesignReviewFinding;
use crate::design_review::types::DesignReviewOutput;
use crate::design_review::types::DesignReviewRequest;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use crate::tasks::SessionTaskContext;
use crate::tasks::run_one_shot_review;

pub(crate) struct DebateOrchestrator;

/// What the debate produced: the Judge's net-new findings, plus (v1.5b, only when
/// `contest_critic` is on) the critic findings the Judge deemed false positives.
/// The caller unions `findings` into the critic set and applies `refutations` to
/// it (marking them Contested — never deleting).
pub(crate) struct DebateOutcome {
    pub findings: DesignReviewOutput,
    pub refutations: Vec<Refutation>,
}

/// Pure: the ordered `(role, round)` plan for the persona phase (Advocate on
/// even turns, Skeptic on odd), terminating at `2 * rounds`. The Judge is a
/// separate final turn, not part of this plan. Mirrors
/// `conditional_logic.py::should_continue_debate` from the upstream design.
fn persona_turn_plan(rounds: u8) -> Vec<(DebateRole, u8)> {
    (0..2 * u16::from(rounds))
        .map(|i| {
            let role = if i % 2 == 0 {
                DebateRole::Advocate
            } else {
                DebateRole::Skeptic
            };
            (role, (i / 2) as u8)
        })
        .collect()
}

impl DebateOrchestrator {
    /// Run the debate and synthesize findings, or `Err(Degraded)` (caller falls
    /// back to the single-shot review). `critic_findings` are the single-shot
    /// critic's output (v1.5a): they seed every persona/judge turn so the debate
    /// targets the gap the critic left, and the Judge emits only findings beyond
    /// them. The caller keeps `critic_findings` verbatim — the debate never
    /// mutates or drops them.
    pub(crate) async fn run(
        session: &Arc<Session>,
        turn: &Arc<TurnContext>,
        request: &DesignReviewRequest,
        cfg: &DebateConfig,
        critic_findings: &[DesignReviewFinding],
    ) -> Result<DebateOutcome, DesignReviewError> {
        let session_ctx = Arc::new(SessionTaskContext::new(
            Arc::clone(session),
            Arc::clone(&turn.extension_data),
        ));
        let cancellation_token = CancellationToken::new();
        let mut transcript = DebateTranscript::default();

        // Persona phase.
        for (role, round) in persona_turn_plan(cfg.rounds) {
            let model = cfg.model_for(role).ok_or_else(|| {
                DesignReviewError::Degraded(format!("no model configured for {}", role.label()))
            })?;
            let prompt = match role {
                DebateRole::Advocate => build_advocate_prompt(
                    &request.design_markdown,
                    critic_findings,
                    &request.accepted_risks,
                    &transcript.render(),
                ),
                DebateRole::Skeptic => build_skeptic_prompt(
                    &request.design_markdown,
                    critic_findings,
                    &request.accepted_risks,
                    &transcript.render(),
                ),
                DebateRole::Judge => unreachable!("Judge is not part of the persona plan"),
            };
            let event = Self::one_call(&session_ctx, turn, &cancellation_token, prompt, model, cfg)
                .await
                .ok_or_else(|| {
                    DesignReviewError::Degraded(format!("{} turn produced no output", role.label()))
                })?;
            transcript.push(role, round, event.overall_explanation);
        }

        // Judge phase — the only structured turn.
        let judge_model = cfg.model_for(DebateRole::Judge).ok_or_else(|| {
            DesignReviewError::Degraded("no model configured for Judge".to_string())
        })?;
        let judge_prompt = build_judge_prompt(
            &request.design_markdown,
            critic_findings,
            &transcript.render(),
            &request.accepted_risks,
            cfg.contest_critic,
        );
        let judge_event = Self::one_call(
            &session_ctx,
            turn,
            &cancellation_token,
            judge_prompt,
            judge_model,
            cfg,
        )
        .await
        .ok_or_else(|| DesignReviewError::Degraded("Judge produced no output".to_string()))?;
        // Reuse the single-shot parser + salvage so a fenced/truncated judge
        // response is handled exactly as today's critic response is; additionally
        // extract any refutations (empty unless contest_critic put the spec in the
        // prompt).
        let (findings, refutations) = parse_judge_output(&judge_event.overall_explanation);
        Ok(DebateOutcome {
            findings,
            refutations,
        })
    }

    /// One persona/judge sub-session. `None` on empty/timeout/cancel (the caller
    /// degrades). Keeps `ReasoningEffort::None` so reasoning tokens do not eat
    /// the output budget (the spike's load-bearing methodology constraint).
    async fn one_call(
        session_ctx: &Arc<SessionTaskContext>,
        turn: &Arc<TurnContext>,
        cancellation_token: &CancellationToken,
        prompt: String,
        model: String,
        cfg: &DebateConfig,
    ) -> Option<ReviewOutputEvent> {
        let input = vec![UserInput::Text {
            text: "Respond exactly as your role instructions above direct.".to_string(),
            text_elements: Vec::new(),
        }];
        let future = run_one_shot_review(
            Arc::clone(session_ctx),
            Arc::clone(turn),
            input,
            cancellation_token.clone(),
            prompt,
            model,
            Some(ReasoningEffort::None),
        );
        match tokio::time::timeout(cfg.per_turn_timeout, future).await {
            Ok(event) => event,
            Err(_) => {
                // Abort the in-flight turn; we are bailing to single-shot anyway.
                cancellation_token.cancel();
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persona_plan_alternates_and_terminates_at_two_rounds() {
        assert_eq!(
            persona_turn_plan(1),
            vec![(DebateRole::Advocate, 0), (DebateRole::Skeptic, 0)]
        );
        assert_eq!(
            persona_turn_plan(2),
            vec![
                (DebateRole::Advocate, 0),
                (DebateRole::Skeptic, 0),
                (DebateRole::Advocate, 1),
                (DebateRole::Skeptic, 1),
            ]
        );
        // rounds=3 -> 6 persona turns (+1 judge = 7 calls total).
        assert_eq!(persona_turn_plan(3).len(), 6);
        // Advocate always opens; Skeptic always closes the persona phase.
        let plan = persona_turn_plan(3);
        assert_eq!(plan.first().unwrap().0, DebateRole::Advocate);
        assert_eq!(plan.last().unwrap().0, DebateRole::Skeptic);
    }
}
