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
    /// Store it, but hold it out of retrieval until the user settles which of two
    /// contradicting statements holds.
    NeedsReview { conflicts_with: String },
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
    /// Claims stored but held back because they contradict something current.
    pub needs_review: usize,
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
                normalize(&claim.subject) == normalize(&candidate.subject)
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
        // The store keys on the canonical spelling; a candidate still carrying
        // `User` rather than `user` would otherwise miss its own history.
        let claim = claim.normalized();
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
            ReconcileAction::NeedsReview { conflicts_with } => {
                match db
                    .insert_conflicting(claim, evidence, &conflicts_with)
                    .await
                {
                    Ok(_) => outcome.needs_review += 1,
                    Err(err) => {
                        tracing::warn!(
                            error = %err,
                            "assistant memory: could not store a claim awaiting review"
                        );
                        outcome.failed += 1;
                    }
                }
            }
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

/// Words that flip the meaning of a statement.
const NEGATIONS_CJK: &[&str] = &["不", "别", "勿", "没", "非"];
const NEGATIONS_WORD: &[&str] = &["no", "not", "never", "dont", "don't", "avoid", "without"];
/// Auxiliaries that negation leaves behind: "do not use tabs" has to look like
/// "use tabs" once the negation is gone, or the comparison fails on `do`.
const AUXILIARIES_WORD: &[&str] = &["do", "does", "did", "be", "is", "are", "was", "were", "am"];

/// Whether two statements about one subject contradict each other.
///
/// The rule is deliberately narrow: strip the negations from both sides and the
/// remaining wording must be *identical*, while exactly one side was negated.
/// That catches "reply briefly" against "do not reply briefly" without guessing
/// at looser paraphrases, where a wrong call would hide a true memory behind a
/// review the user never asked for.
pub fn is_contradiction(left: &str, right: &str) -> bool {
    let (left_negated, left_core) = polarity(left);
    let (right_negated, right_core) = polarity(right);
    left_negated != right_negated && !left_core.trim().is_empty() && left_core == right_core
}

/// Whether the text contains a negation.
///
/// Used by extraction to reject a claim that drops a `not` the user actually
/// said — the one paraphrase that silently inverts a memory.
pub(crate) fn has_negation(text: &str) -> bool {
    polarity(text).0
}

/// Returns whether the text is negated, plus the text with negations removed.
fn polarity(text: &str) -> (bool, String) {
    let normalized = normalize(text);
    let mut negated = false;
    for token in NEGATIONS_CJK {
        if normalized.contains(token) {
            negated = true;
        }
    }
    let stripped = NEGATIONS_CJK
        .iter()
        .fold(normalized, |acc, token| acc.replace(token, ""));

    let mut core = String::with_capacity(stripped.len());
    for word in stripped.split_whitespace() {
        let bare = word.trim_matches(|ch: char| !ch.is_alphanumeric());
        if NEGATIONS_WORD.contains(&bare) {
            negated = true;
            continue;
        }
        // Dropped from both sides equally, so the comparison stays symmetric while
        // "do not use tabs" and "use tabs" line up.
        if AUXILIARIES_WORD.contains(&bare) {
            continue;
        }
        if !core.is_empty() {
            core.push(' ');
        }
        core.push_str(word);
    }
    (negated, core)
}

/// The default judge: adds, skips what is already held, and flags contradictions.
///
/// It cannot *resolve* a contradiction — deciding which of two opposite
/// statements is true needs the user, or a model reading both sides — so the
/// claim is stored with the conflict recorded and held out of retrieval until
/// the user confirms it or removes it.
#[derive(Clone, Copy, Debug, Default)]
pub struct ContradictionJudge;

impl ReconcileJudge for ContradictionJudge {
    fn decide<'a>(&'a self, candidate: &'a NewClaim, existing: &'a [Claim]) -> ReconcileFuture<'a> {
        Box::pin(async move {
            let subject = normalize(&candidate.subject);
            if existing.iter().any(|claim| {
                normalize(&claim.subject) == subject
                    && is_same_statement(&claim.statement, &candidate.statement)
            }) {
                return ReconcileAction::SkipDuplicate;
            }

            match existing.iter().find(|claim| {
                normalize(&claim.subject) == subject
                    && is_contradiction(&claim.statement, &candidate.statement)
            }) {
                Some(claim) => ReconcileAction::NeedsReview {
                    conflicts_with: claim.id.clone(),
                },
                None => ReconcileAction::Add,
            }
        })
    }
}

pub(crate) fn normalize(statement: &str) -> String {
    crate::memory_db::normalize_key(statement)
}

#[cfg(test)]
#[path = "reconcile_tests.rs"]
mod tests;
