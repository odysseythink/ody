//! Reconciliation: what a session's claims do to what is already stored.
//!
//! Extraction is per session, memory is cumulative, so every claim has to be
//! placed against what is already known. Two properties matter here: the same
//! observation arriving twice must not become two memories, and a claim that
//! overturns an older one must close it rather than overwrite it — the earlier
//! belief stays answerable.

use std::future::Future;
use std::pin::Pin;

use crate::Claim;
use crate::MemoryDb;
use crate::MemoryDbError;
use crate::NewClaim;
use crate::NewEvidence;

/// What happens to one candidate claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconcileAction {
    /// Store it as a new memory.
    Add,
    /// The same statement is already current; keep the existing memory.
    SkipDuplicate,
    /// Store it and close the claim it replaces.
    Supersede { replaced: String },
}

/// One candidate claim, plus how it relates to what is already known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileCandidate {
    pub claim: NewClaim,
    pub evidence: Vec<NewEvidence>,
    /// The claim this one replaces, as declared by the extraction model when it
    /// was shown the current beliefs. It is only honoured when the store still
    /// holds that claim, so a model cannot point at something that is not there.
    pub supersedes: Option<String>,
}

/// Tally of one reconciliation pass.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ReconcileOutcome {
    pub added: usize,
    pub skipped: usize,
    pub superseded: usize,
    pub failed: usize,
}

pub type ReconcileFuture<'a> = Pin<Box<dyn Future<Output = ReconcileAction> + Send + 'a>>;

/// Decides what a candidate does to the claims already stored for its subject.
pub trait ReconcileJudge: Send + Sync {
    fn decide<'a>(&'a self, candidate: &'a NewClaim, existing: &'a [Claim]) -> ReconcileFuture<'a>;
}

/// Adds a claim unless the same statement is already current for that subject.
///
/// This is the offline default. It cannot detect a *contradiction* (that takes
/// a judge that reads both sides), but it makes the pipeline idempotent, which
/// is what stops a long-running assistant from storing the same observation
/// every time it comes up.
#[derive(Clone, Copy, Debug, Default)]
pub struct ConservativeJudge;

impl ReconcileJudge for ConservativeJudge {
    fn decide<'a>(&'a self, candidate: &'a NewClaim, existing: &'a [Claim]) -> ReconcileFuture<'a> {
        Box::pin(async move {
            let duplicate = existing.iter().any(|claim| {
                claim.subject == candidate.subject
                    && is_same_statement(&claim.statement, &candidate.statement)
            });
            if duplicate {
                ReconcileAction::SkipDuplicate
            } else {
                ReconcileAction::Add
            }
        })
    }
}

/// Applies one session's grounded claims to the store.
pub async fn apply_claims(
    db: &MemoryDb,
    judge: &dyn ReconcileJudge,
    candidates: Vec<ReconcileCandidate>,
) -> Result<ReconcileOutcome, MemoryDbError> {
    let mut outcome = ReconcileOutcome::default();

    for candidate in candidates {
        let ReconcileCandidate {
            claim,
            evidence,
            supersedes,
        } = candidate;
        let existing = db.active_claims_for_subject(&claim.subject).await?;

        // A model-declared replacement is honoured only against a claim that is
        // really there; anything else goes back to the judge.
        let declared = supersedes
            .filter(|target| existing.iter().any(|held| &held.id == target))
            .map(|replaced| ReconcileAction::Supersede { replaced });

        let action = match declared {
            Some(action) => action,
            None => judge.decide(&claim, &existing).await,
        };

        match action {
            ReconcileAction::Add => match db.insert_claim(claim, evidence).await {
                Ok(_) => outcome.added += 1,
                Err(err) => {
                    tracing::warn!(error = %err, "assistant memory: could not store a claim");
                    outcome.failed += 1;
                }
            },
            ReconcileAction::SkipDuplicate => outcome.skipped += 1,
            ReconcileAction::Supersede { replaced } => {
                let observed_at = claim.valid_from.unwrap_or_else(chrono::Utc::now);
                match db.supersede(&replaced, claim, evidence, observed_at).await {
                    Ok(_) => outcome.superseded += 1,
                    Err(err) => {
                        tracing::warn!(error = %err, "assistant memory: could not supersede a claim");
                        outcome.failed += 1;
                    }
                }
            }
        }
    }

    Ok(outcome)
}

/// Whether two statements say the same thing, ignoring case and spacing.
pub fn is_same_statement(left: &str, right: &str) -> bool {
    normalize(left) == normalize(right)
}

pub(crate) fn normalize(statement: &str) -> String {
    statement
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[cfg(test)]
#[path = "reconcile_tests.rs"]
mod tests;
