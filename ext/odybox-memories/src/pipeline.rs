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

use crate::extract::MemoryExtractor;
use crate::ledger::ExtractionLedger;
use crate::prompts;
use crate::scan;

/// Sessions extracted per pipeline run, bounding model spend per start.
pub const DEFAULT_SESSION_BUDGET: usize = 3;
/// Sessions pulled from the rollout listing before budget filtering.
pub const SESSION_SCAN_LIMIT: usize = 40;

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
    ledger: &ExtractionLedger,
    thread_id: &ThreadId,
    transcript: &str,
    recorded_at: &str,
) -> SessionOutcome {
    if transcript.trim().is_empty() {
        return SessionOutcome::Empty;
    }

    let (transcript, _truncated) = prompts::truncate_transcript(transcript);
    let memory = match extractor.extract(&transcript).await {
        Ok(memory) => memory,
        Err(err) => {
            tracing::warn!(
                thread_id = %thread_id,
                transcript_chars = transcript.chars().count(),
                error = %err,
                "assistant memory: extraction failed"
            );
            return SessionOutcome::Failed;
        }
    };

    let body = memory.render(&thread_id.to_string(), recorded_at);
    if let Err(err) = ledger.record(thread_id, &body).await {
        tracing::warn!(
            thread_id = %thread_id,
            error = %err,
            "assistant memory: could not record extraction"
        );
        return SessionOutcome::Failed;
    }

    if memory.is_empty() {
        SessionOutcome::Empty
    } else {
        SessionOutcome::Extracted
    }
}

/// Reads a rollout file and renders it into a transcript.
pub async fn transcript_for_rollout(path: &Path) -> std::io::Result<String> {
    let (items, _, _) = RolloutRecorder::load_rollout_items(path)
        .await
        .map_err(|err| std::io::Error::other(err.to_string()))?;
    Ok(prompts::transcript_from_rollout(&items))
}

/// Runs one pass of the write pipeline.
///
/// Returns an error only when discovery itself fails; per-session failures are
/// reported in [`PipelineReport`] so one bad session cannot abort the pass.
pub async fn run_once(
    extractor: &dyn MemoryExtractor,
    ledger: &ExtractionLedger,
    ody_home: &Path,
    default_provider: &str,
    budget: usize,
    recorded_at: &str,
) -> std::io::Result<PipelineReport> {
    let sessions = scan::odybox_sessions(ody_home, default_provider, SESSION_SCAN_LIMIT).await?;

    let mut report = PipelineReport {
        sessions_seen: sessions.len(),
        ..Default::default()
    };
    let mut processed = 0_usize;

    for item in sessions {
        if processed >= budget {
            break;
        }
        let Some(thread_id) = item.thread_id else {
            continue;
        };
        if ledger.is_extracted(&thread_id).await {
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

        match process_transcript(extractor, ledger, &thread_id, &transcript, recorded_at).await {
            SessionOutcome::Extracted => report.extracted += 1,
            SessionOutcome::Empty => {
                if transcript.trim().is_empty() {
                    report.empty_transcript += 1;
                } else {
                    report.empty += 1;
                }
            }
            SessionOutcome::Failed => report.failed += 1,
        }
    }

    Ok(report)
}
