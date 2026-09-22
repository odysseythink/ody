//! Consolidation: fold claims that say the same thing, less well.
//!
//! Deliberately conservative. A claim is closed only when another current claim
//! about the same subject, of the same kind, already says everything it says.
//! Nothing is paraphrased, nothing is invented, and nothing is deleted — the
//! closed claim keeps its validity window, so the earlier belief stays
//! answerable. Anything cleverer (merging two half-truths into one) needs a
//! model reading both sides, and belongs in a judge, not here.

use chrono::Utc;

use crate::MemoryDb;
use crate::MemoryDbError;
use crate::reconcile::normalize;

/// Claims examined per subject in one pass.
pub const SUBJECT_CLAIM_LIMIT: usize = 50;
/// Subjects consolidated per pipeline pass.
pub const SUBJECT_LIMIT: usize = 20;

/// Tally of one consolidation pass.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ConsolidateOutcome {
    pub subjects: usize,
    pub subsumed: usize,
}

/// Whether `covering` says everything `candidate` says, and more.
pub fn subsumes(covering: &str, candidate: &str) -> bool {
    let covering = normalize(covering);
    let candidate = normalize(candidate);
    !candidate.is_empty() && covering.len() > candidate.len() && covering.contains(&candidate)
}

/// Folds subsumed claims for one subject, returning how many were closed.
pub async fn consolidate_subject(db: &MemoryDb, subject: &str) -> Result<usize, MemoryDbError> {
    let claims = db.active_claims_for_subject(subject).await?;
    let claims = claims
        .into_iter()
        .take(SUBJECT_CLAIM_LIMIT)
        .collect::<Vec<_>>();

    let mut closed: Vec<String> = Vec::new();
    let now = Utc::now();

    for candidate in &claims {
        if closed.contains(&candidate.id) {
            continue;
        }
        let covering = claims.iter().find(|other| {
            other.id != candidate.id
                && other.kind == candidate.kind
                && !closed.contains(&other.id)
                && subsumes(&other.statement, &candidate.statement)
        });
        let Some(covering) = covering else {
            continue;
        };

        match db
            .mark_superseded_by(&candidate.id, &covering.id, now)
            .await
        {
            Ok(()) => closed.push(candidate.id.clone()),
            Err(err) => tracing::warn!(
                error = %err,
                claim_id = %candidate.id,
                "assistant memory: could not fold a claim"
            ),
        }
    }

    Ok(closed.len())
}

/// Folds subsumed claims across the subjects that have any.
pub async fn consolidate_all(db: &MemoryDb) -> Result<ConsolidateOutcome, MemoryDbError> {
    let mut seen: Vec<String> = Vec::new();
    for claim in db
        .active_claims(SUBJECT_CLAIM_LIMIT * SUBJECT_LIMIT)
        .await?
    {
        if !seen.contains(&claim.subject) {
            seen.push(claim.subject);
        }
    }

    let mut outcome = ConsolidateOutcome::default();
    for subject in seen.into_iter().take(SUBJECT_LIMIT) {
        outcome.subjects += 1;
        outcome.subsumed += consolidate_subject(db, &subject).await?;
    }
    Ok(outcome)
}

#[cfg(test)]
#[path = "consolidate_tests.rs"]
mod tests;
