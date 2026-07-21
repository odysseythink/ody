//! Transcript consolidation for finalized streaming agent messages.
//!
//! During streaming, the chat widget emits transient `AgentMessageCell`s so it
//! can animate stable lines into scrollback while keeping the active mutable
//! tail in the bottom pane. Once the answer finishes, the app replaces that
//! trailing run with a single source-backed `AgentMarkdownCell`. This makes the
//! transcript the canonical owner of the raw markdown source used for future
//! resize re-renders.

use std::path::PathBuf;
use std::sync::Arc;

use color_eyre::eyre::Result;

use super::App;
use super::resize_reflow::trailing_run_start;
use crate::app_event::ConsolidationScrollbackReflow;
use crate::history_cell;
use crate::history_cell::HistoryCell;
use crate::pager_overlay::Overlay;
use crate::tui;

impl App {
    pub(super) fn handle_consolidate_agent_message(
        &mut self,
        tui: &mut tui::Tui,
        source: String,
        cwd: PathBuf,
        scrollback_reflow: ConsolidationScrollbackReflow,
        deferred_history_cell: Option<Box<dyn HistoryCell>>,
        pending_reasoning_cells: Vec<Box<dyn HistoryCell>>,
    ) -> Result<()> {
        // Some finalize paths must preserve a last provisional stream cell long
        // enough for queue ordering, then fold it into the canonical
        // source-backed cell during consolidation.
        if let Some(cell) = deferred_history_cell {
            let cell: Arc<dyn HistoryCell> = cell.into();
            if let Some(Overlay::Transcript(t)) = &mut self.overlay {
                t.insert_cell(cell.clone());
            }
            self.transcript_cells.push(cell);
        }

        let pending_reasoning: Vec<Arc<dyn HistoryCell>> =
            pending_reasoning_cells.into_iter().map(Arc::from).collect();
        let has_pending_reasoning = !pending_reasoning.is_empty();

        // Walk backward to find the contiguous run of streaming AgentMessageCells that
        // belong to the just-finalized stream.
        let end = self.transcript_cells.len();
        tracing::debug!(
            "ConsolidateAgentMessage: transcript_cells.len()={end}, source_len={}",
            source.len()
        );
        let start = trailing_run_start::<history_cell::AgentMessageCell>(&self.transcript_cells);
        if start < end {
            tracing::debug!(
                "ConsolidateAgentMessage: replacing cells [{start}..{end}] with AgentMarkdownCell"
            );
            let consolidated: Arc<dyn HistoryCell> =
                Arc::new(history_cell::AgentMarkdownCell::new(source, &cwd));
            let replacement: Vec<Arc<dyn HistoryCell>> = pending_reasoning
                .into_iter()
                .chain(std::iter::once(consolidated.clone()))
                .collect();
            self.transcript_cells.splice(start..end, replacement);

            if let Some(Overlay::Transcript(t)) = &mut self.overlay {
                t.replace_cells(self.transcript_cells.clone());
                tui.frame_requester().schedule_frame();
            }

            // Inserting pending reasoning before the consolidated message changes the
            // scrollback, so force a full reflow when there is deferred reasoning.
            let scrollback_reflow = if has_pending_reasoning {
                ConsolidationScrollbackReflow::Required
            } else {
                scrollback_reflow
            };
            self.finish_agent_message_consolidation(tui, scrollback_reflow)?;
        } else if has_pending_reasoning {
            tracing::debug!(
                "ConsolidateAgentMessage: appending {count} pending reasoning cells",
                count = pending_reasoning.len(),
            );
            self.transcript_cells.extend(pending_reasoning);
            if let Some(Overlay::Transcript(t)) = &mut self.overlay {
                t.replace_cells(self.transcript_cells.clone());
                tui.frame_requester().schedule_frame();
            }
            // New cells at the end still need to be written to scrollback.
            self.finish_required_stream_reflow(tui)?;
        } else {
            tracing::debug!(
                "ConsolidateAgentMessage: no cells to consolidate(start={start}, end={end})",
            );
            self.maybe_finish_stream_reflow(tui)?;
        }

        Ok(())
    }

    fn finish_agent_message_consolidation(
        &mut self,
        tui: &mut tui::Tui,
        scrollback_reflow: ConsolidationScrollbackReflow,
    ) -> Result<()> {
        match scrollback_reflow {
            ConsolidationScrollbackReflow::IfResizeReflowRan => {
                self.maybe_finish_stream_reflow(tui)?;
            }
            ConsolidationScrollbackReflow::Required => {
                self.finish_required_stream_reflow(tui)?;
            }
        }

        Ok(())
    }
}
