//! Write pipeline: discover odyBox sessions, extract the unprocessed ones, and
//! record the result in the assistant memory root.
//!
//! The pipeline is deliberately *not* a Phase 1 / Phase 2 pair. There is no job
//! table, no lease and no separate consolidation agent: sessions are found by
//! listing rollout files, processed at most once each, and their output is
//! written straight into the assistant memory root. Consolidation across
//! sessions is left to a later stage.

use std::path::Path;

use ody_protocol::ThreadId;
use ody_rollout::RolloutRecorder;

use crate::MemoryDb;
use crate::extract::MemoryExtractor;
use crate::ledger::ExtractionLedger;
use crate::memory_db::VIEW_FILENAME;
use crate::prompts;
use crate::reconcile::ReconcileJudge;
use crate::reconcile::apply_claims;
use crate::scan;
use crate::store::ASSISTANT_MEMORY_DIR;

/// Claims shown to the extraction model, so it can name what a new statement
/// replaces. Bounded because the block is resident in every extraction prompt.
pub const KNOWN_CLAIMS_LIMIT: usize = 40;

/// Sessions extracted per pipeline run, bounding model spend per start.
pub const DEFAULT_SESSION_BUDGET: usize = 3;
/// Sessions pulled from the rollout listing before budget filtering.
pub const SESSION_SCAN_LIMIT: usize = 40;

/// Counters behind one session's outcome, for the run log.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SessionStats {
    /// Claims the model proposed.
    pub candidates: usize,
    /// Claims thrown away for citing no line, or a line that is not there.
    pub dropped_ungrounded: usize,
    pub added: usize,
    pub skipped: usize,
    pub superseded: usize,
    pub store_failed: usize,
    /// Claims already in the store that the model was shown.
    pub known_claims_shown: usize,
}

impl SessionStats {
    fn absorb(&mut self, other: &SessionStats) {
        self.candidates += other.candidates;
        self.dropped_ungrounded += other.dropped_ungrounded;
        self.added += other.added;
        self.skipped += other.skipped;
        self.superseded += other.superseded;
        self.store_failed += other.store_failed;
        self.known_claims_shown = self.known_claims_shown.max(other.known_claims_shown);
    }
}

/// One session's outcome plus the counters that explain it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionReport {
    pub outcome: SessionOutcome,
    pub stats: SessionStats,
}

impl SessionReport {
    fn bare(outcome: SessionOutcome) -> Self {
        Self {
            outcome,
            stats: SessionStats::default(),
        }
    }
}

/// Outcome of processing one session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionOutcome {
    /// A memory record was produced and written.
    Extracted,
    /// The model judged the session to hold no durable memory. Recorded as
    /// processed so it is not retried forever.
    Empty,
    /// The model call or the write failed. Not recorded: the session is retried
    /// on a later run.
    Failed,
}

/// Aggregate result of one pipeline pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PipelineReport {
    pub sessions_seen: usize,
    pub already_processed: usize,
    pub empty_transcript: usize,
    pub extracted: usize,
    pub empty: usize,
    pub failed: usize,
}

/// Processes a single session transcript through extraction and recording.
///
/// Separated from discovery so it can be exercised without fabricating rollout
/// files.
pub async fn process_transcript(
    extractor: &dyn MemoryExtractor,
    judge: &dyn ReconcileJudge,
    db: &MemoryDb,
    ledger: &ExtractionLedger,
    thread_id: &ThreadId,
    transcript: &prompts::Transcript,
    recorded_at: &str,
) -> SessionReport {
    if transcript.is_blank() {
        return SessionReport::bare(SessionOutcome::Empty);
    }

    let (mut transcript, _truncated) = transcript.truncated();
    // Shown to the extraction model so a new statement can replace an old one
    // instead of accumulating beside it. A failure here must not cost the user
    // their extraction, so it degrades to "nothing known yet".
    transcript.known_claims = db
        .active_claims(KNOWN_CLAIMS_LIMIT)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|claim| prompts::KnownClaim {
            id: claim.id,
            subject: claim.subject,
            statement: claim.statement,
        })
        .collect();
    let mut memory = match extractor.extract(&transcript).await {
        Ok(memory) => memory,
        Err(err) => {
            tracing::warn!(
                thread_id = %thread_id,
                transcript_chars = transcript.text.chars().count(),
                error = %err,
                "assistant memory: extraction failed"
            );
            return SessionReport::bare(SessionOutcome::Failed);
        }
    };

    let proposed = memory.claims.len();
    memory.retain_grounded(&transcript.items);
    let mut stats = SessionStats {
        candidates: proposed,
        dropped_ungrounded: proposed - memory.claims.len(),
        known_claims_shown: transcript.known_claims.len(),
        ..SessionStats::default()
    };

    // The store is written before the record: if storing fails the session is
    // not marked done, so a later pass retries it. Re-running is safe because
    // reconciliation skips claims it already holds.
    let observed_at = chrono::DateTime::parse_from_rfc3339(recorded_at)
        .map(|at| at.with_timezone(&chrono::Utc))
        .unwrap_or_else(|_| chrono::Utc::now());
    let candidates = memory.claims.len();
    let outcome = match apply_claims(
        db,
        judge,
        memory.to_candidates(&thread_id.to_string(), observed_at),
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(err) => {
            tracing::warn!(
                thread_id = %thread_id,
                error = %err,
                "assistant memory: could not store extracted claims"
            );
            return SessionReport::bare(SessionOutcome::Failed);
        }
    };
    stats.added = outcome.added;
    stats.skipped = outcome.skipped;
    stats.superseded = outcome.superseded;
    stats.store_failed = outcome.failed;
    tracing::info!(
        thread_id = %thread_id,
        candidates,
        added = outcome.added,
        skipped = outcome.skipped,
        superseded = outcome.superseded,
        failed = outcome.failed,
        "assistant memory: reconciled session claims"
    );

    let body = memory.render(&thread_id.to_string(), recorded_at);
    if let Err(err) = ledger.record(thread_id, &body).await {
        tracing::warn!(
            thread_id = %thread_id,
            error = %err,
            "assistant memory: could not record extraction"
        );
        return SessionReport::bare(SessionOutcome::Failed);
    }

    let outcome = if memory.is_empty() {
        SessionOutcome::Empty
    } else {
        SessionOutcome::Extracted
    };
    SessionReport { outcome, stats }
}

/// Reads a rollout file and renders it into a transcript.
pub async fn transcript_for_rollout(path: &Path) -> std::io::Result<prompts::Transcript> {
    let (items, _, _) = RolloutRecorder::load_rollout_items(path)
        .await
        .map_err(|err| std::io::Error::other(err.to_string()))?;
    Ok(prompts::transcript_from_rollout(&items))
}

/// Runs one pass of the write pipeline.
///
/// Returns an error only when discovery itself fails; per-session failures are
/// reported in [`PipelineReport`] so one bad session cannot abort the pass.
/// Everything one pass needs. Grouped so the entry point stays readable.
pub struct PipelineInput<'a> {
    pub extractor: &'a dyn MemoryExtractor,
    pub judge: &'a dyn ReconcileJudge,
    pub db: &'a MemoryDb,
    pub ledger: &'a ExtractionLedger,
    pub ody_home: &'a Path,
    pub default_provider: &'a str,
    pub budget: usize,
    pub recorded_at: &'a str,
}

pub async fn run_once(input: PipelineInput<'_>) -> std::io::Result<PipelineReport> {
    let memory_root = input.ody_home.join(ASSISTANT_MEMORY_DIR);

    // Whatever the settings panel asked for (forget / keep) takes effect before
    // this pass extracts anything new.
    let (ops_applied, ops_failed) =
        match crate::user_ops::apply_pending(input.db, &memory_root).await {
            Ok(outcome) => {
                if outcome.applied > 0 || outcome.failed > 0 {
                    tracing::info!(
                        applied = outcome.applied,
                        failed = outcome.failed,
                        "assistant memory: applied user operations"
                    );
                }
                (outcome.applied, outcome.failed)
            }
            Err(err) => {
                tracing::warn!(error = %err, "assistant memory: could not read user operations");
                (0, 0)
            }
        };

    let started = std::time::Instant::now();
    let sessions =
        scan::odybox_sessions(input.ody_home, input.default_provider, SESSION_SCAN_LIMIT).await?;

    let mut report = PipelineReport {
        sessions_seen: sessions.len(),
        ..Default::default()
    };
    let mut processed = 0_usize;
    let mut totals = SessionStats::default();

    for item in sessions {
        if processed >= input.budget {
            break;
        }
        let Some(thread_id) = item.thread_id else {
            continue;
        };
        if input.ledger.is_extracted(&thread_id).await {
            report.already_processed += 1;
            continue;
        }
        processed += 1;

        let transcript = match transcript_for_rollout(&item.path).await {
            Ok(transcript) => transcript,
            Err(err) => {
                tracing::warn!(
                    path = %item.path.display(),
                    error = %err,
                    "assistant memory: could not read rollout transcript"
                );
                report.failed += 1;
                continue;
            }
        };

        let session = process_transcript(
            input.extractor,
            input.judge,
            input.db,
            input.ledger,
            &thread_id,
            &transcript,
            input.recorded_at,
        )
        .await;
        totals.absorb(&session.stats);
        match session.outcome {
            SessionOutcome::Extracted => report.extracted += 1,
            SessionOutcome::Empty => {
                if transcript.is_blank() {
                    report.empty_transcript += 1;
                } else {
                    report.empty += 1;
                }
            }
            SessionOutcome::Failed => report.failed += 1,
        }
    }

    // Fold claims another current claim already covers, so the store does not
    // accumulate three wordings of one belief.
    let (consolidated_subjects, subsumed) =
        match crate::consolidate::consolidate_all(input.db).await {
            Ok(outcome) => {
                if outcome.subsumed > 0 {
                    tracing::info!(
                        subjects = outcome.subjects,
                        subsumed = outcome.subsumed,
                        "assistant memory: folded subsumed claims"
                    );
                }
                (outcome.subjects, outcome.subsumed)
            }
            Err(err) => {
                tracing::warn!(error = %err, "assistant memory: consolidation failed");
                (0, 0)
            }
        };

    // One line of detail per pass, and the counters the panel shows, so the
    // question "is memory working?" is answerable later without guesswork.
    let injected_claims = input
        .db
        .injection_candidates(crate::INJECTION_LIMIT, chrono::Utc::now())
        .await
        .map(|claims| claims.len())
        .unwrap_or(0);
    let event = crate::PassEvent {
        at: chrono::Utc::now().to_rfc3339(),
        duration_ms: started.elapsed().as_millis() as u64,
        sessions_seen: report.sessions_seen,
        extracted: report.extracted,
        empty: report.empty,
        failed: report.failed,
        candidates: totals.candidates,
        dropped_ungrounded: totals.dropped_ungrounded,
        added: totals.added,
        skipped: totals.skipped,
        superseded: totals.superseded,
        store_failed: totals.store_failed,
        consolidated_subjects,
        subsumed,
        ops_applied,
        ops_failed,
        known_claims_shown: totals.known_claims_shown,
        injected_claims,
    };
    if let Err(err) = crate::append_event(&memory_root, &event).await {
        tracing::warn!(error = %err, "assistant memory: could not append the run log");
    }
    let claims_in_store = input
        .db
        .active_claims(1_000)
        .await
        .map(|claims| claims.len() as u64)
        .unwrap_or(0);
    if let Err(err) = crate::update_summary(&memory_root, &event, claims_in_store).await {
        tracing::warn!(error = %err, "assistant memory: could not update the run summary");
    }

    // Refresh the view the settings panel reads, once per pass.
    let view_path = memory_root.join(VIEW_FILENAME);
    if let Err(err) = input.db.render_view(&view_path).await {
        tracing::warn!(error = %err, "assistant memory: could not refresh the memory view");
    }

    Ok(report)
}
