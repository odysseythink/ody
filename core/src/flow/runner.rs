//! M2.3: host-side [`FlowRunner`] implementation backing the extension-side
//! `skills.flow__run` model tool.
//!
//! The extension tool validates the request against its catalog and forwards
//! execution here; this type reuses the exact same runtime, checkpointing,
//! and run-before guardian approval as the slash-command path
//! ([`run_one_flow_skill`]). Routing is per-turn: each built `TurnContext`
//! registers itself (weakly) and lookup fails cleanly once the turn is gone.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::PoisonError;
use std::sync::Weak;

use ody_core_skills::SkillType;
use ody_extension_api::FlowRunError;
use ody_extension_api::FlowRunFuture;
use ody_extension_api::FlowRunOutput;
use ody_extension_api::FlowRunner;

use crate::TurnContext;
use crate::session::session::Session;

use super::FlowError;
use super::trigger::run_one_flow_skill;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Per-turn routing table: `turn_id` → live `TurnContext` (weak). Dead
/// entries are purged lazily on registration so completed turns do not
/// accumulate; no explicit deregistration hook is required.
#[derive(Default)]
pub(crate) struct FlowTurnRouter {
    turns: Mutex<HashMap<String, Weak<TurnContext>>>,
}

impl FlowTurnRouter {
    pub(crate) fn register(&self, turn_id: String, turn_context: &Arc<TurnContext>) {
        let mut turns = lock(&self.turns);
        turns.retain(|_, weak| weak.strong_count() > 0);
        turns.insert(turn_id, Arc::downgrade(turn_context));
    }

    pub(crate) fn lookup(&self, turn_id: &str) -> Option<Arc<TurnContext>> {
        lock(&self.turns)
            .get(turn_id)
            .and_then(|weak| weak.upgrade())
    }

    /// Entries whose turn is still alive. Purge happens on registration, so
    /// dead entries linger until the next `register` call.
    #[cfg(test)]
    pub(crate) fn live_entry_count(&self) -> usize {
        lock(&self.turns)
            .values()
            .filter(|weak| weak.strong_count() > 0)
            .count()
    }

    /// Total map size including dead entries (pre-purge observation).
    #[cfg(test)]
    pub(crate) fn total_entry_count(&self) -> usize {
        lock(&self.turns).len()
    }
}

/// Session-scoped executor injected into extension data as
/// `Arc<dyn FlowRunner>`; the model tool itself never touches `Session`.
pub(crate) struct SessionFlowRunner {
    session: Mutex<Option<Weak<Session>>>,
    router: FlowTurnRouter,
}

impl SessionFlowRunner {
    pub(crate) fn new() -> Self {
        Self {
            session: Mutex::new(None),
            router: FlowTurnRouter::default(),
        }
    }

    /// Backfill the session weak handle once `Arc<Session>` exists
    /// (the runner is created while `Session` is still being built).
    pub(crate) fn init_session(&self, session: Weak<Session>) {
        *lock(&self.session) = Some(session);
    }

    /// Called for every built turn context; keeps routing live until the
    /// turn is dropped.
    pub(crate) fn register_turn(&self, turn_context: &Arc<TurnContext>) {
        self.router
            .register(turn_context.sub_id.clone(), turn_context);
    }
}

impl FlowRunner for SessionFlowRunner {
    fn run_flow<'a>(
        &'a self,
        turn_id: &'a str,
        name: &'a str,
        args: serde_json::Map<String, serde_json::Value>,
    ) -> FlowRunFuture<'a> {
        Box::pin(async move {
            let turn_context = self.router.lookup(turn_id).ok_or_else(|| {
                FlowRunError::InactiveTurn {
                    turn_id: turn_id.to_string(),
                }
            })?;
            let session = {
                let session = lock(&self.session);
                session.as_ref().and_then(|weak| weak.upgrade())
            }
            .ok_or(FlowRunError::SessionUnavailable)?;

            let skill = turn_context
                .turn_skills
                .snapshot
                .outcome()
                .skills
                .iter()
                .find(|skill| skill.name == name && skill.skill_type == SkillType::Flow)
                .cloned()
                .ok_or_else(|| FlowRunError::NotFound {
                    name: name.to_string(),
                })?;

            let result = run_one_flow_skill(&session, &turn_context, &skill, &args).await;
            turn_context.session_telemetry.counter(
                "ody.flow.run",
                /*inc*/ 1,
                &[
                    ("skill", name),
                    ("status", if result.is_ok() { "ok" } else { "error" }),
                    ("source", "flow__run"),
                ],
            );
            match result {
                Ok(outcome) => Ok(FlowRunOutput {
                    outputs: outcome.outputs,
                }),
                Err(FlowError::Denied { reason }) => Err(FlowRunError::Failed { reason }),
                Err(err) => Err(FlowRunError::Failed {
                    reason: err.to_string(),
                }),
            }
        })
    }
}
