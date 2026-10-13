//! How much of the transcript is loaded, and how much to load up front.
//!
//! Two concerns live here because they answer the same question at different times. The view needs
//! to know *whether older pages exist* so it can show a loading affordance, keep a reading position
//! while pages arrive, and let find continue into history it has not seen. Resume needs to know
//! *how much to fetch first* so the first owned frame can be painted without formatting the whole
//! session.
//!
//! Those budgets are deliberately separate. Terminal mode refills a native scrollback, so it is
//! bounded by what the terminal will actually retain (`tui.terminal_resize_reflow`). The owned view
//! repaints a window instead of replaying history, so its first page only has to cover the viewport
//! and a little nearby reading — and must not inherit a scrollback setting, which the user may have
//! disabled or set very high.

use crate::transcript_mode::TranscriptMode;

/// Screenfuls fetched up front in owned mode.
///
/// Three is the smallest number that still lets the reader scroll up a little before the first page
/// boundary, which keeps the paging indicator from appearing immediately on resume.
pub(crate) const OWNED_INITIAL_SCREENS: usize = 3;

/// Safety cap on turns scanned while filling the initial owned window.
///
/// Rows undercount when a turn renders to nothing (hidden reasoning, empty notices), so a pure row
/// budget can scan indefinitely. This bounds the request count regardless of what the rows say.
pub(crate) const OWNED_INITIAL_TURN_SCAN_LIMIT: usize = 100;

/// Completion state of the loaded transcript window.
///
/// `Idle` means "not a paginated thread": a legacy server that returned full history, or a session
/// that never resumed. Loading and failure are distinct from `Complete` so a reader is never told
/// history is exhausted while a page is still in flight or after a request failed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum TranscriptHistoryState {
    #[default]
    Idle,
    LoadingOlder,
    /// A jump to the beginning is in flight; the view holds its position until history reaches the
    /// start or the user navigates away.
    LoadingBeginning,
    /// Older turns exist on the server but are not loaded yet.
    Partial,
    /// The last page request failed. Retryable.
    Failed,
    /// Everything the server has is loaded.
    Complete,
}

impl TranscriptHistoryState {
    /// Whether older pages exist that this view has not loaded.
    pub(crate) fn has_unloaded_history(self) -> bool {
        matches!(
            self,
            Self::LoadingOlder | Self::LoadingBeginning | Self::Partial | Self::Failed
        )
    }

    pub(crate) fn is_loading(self) -> bool {
        matches!(self, Self::LoadingOlder | Self::LoadingBeginning)
    }
}

/// How many transcript rows to fetch before the first owned frame.
///
/// `terminal_height` is the full screen height, so the *history* region is smaller than
/// `height * OWNED_INITIAL_SCREENS`; that is intentional, since the same budget also has to cover a
/// few rows of scrolling and a taller composer shrinks the window without needing a new page.
pub(crate) fn owned_initial_row_budget(terminal_height: u16) -> usize {
    usize::from(terminal_height.max(/*other*/ 1)) * OWNED_INITIAL_SCREENS
}

/// Row budget for the initial hydration pass under `mode`.
///
/// Owned mode ignores the configured terminal reflow cap entirely; Terminal mode keeps using it,
/// including the `None` ("no cap") case that degrades to a full-history resume.
pub(crate) fn initial_hydration_row_budget(
    mode: TranscriptMode,
    terminal_height: u16,
    terminal_reflow_rows: Option<usize>,
) -> Option<usize> {
    if mode.is_owned() {
        Some(owned_initial_row_budget(terminal_height))
    } else {
        terminal_reflow_rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn owned_budget_scales_with_terminal_height_only() {
        assert_eq!(owned_initial_row_budget(/*terminal_height*/ 24), 72);
        assert_eq!(owned_initial_row_budget(/*terminal_height*/ 1), 3);
        // A degenerate height still yields a usable budget rather than zero rows.
        assert_eq!(owned_initial_row_budget(/*terminal_height*/ 0), 3);
    }

    #[test]
    fn owned_hydration_ignores_the_terminal_reflow_cap() {
        // The user disabling the scrollback cap must not turn owned resume into a full replay.
        assert_eq!(
            initial_hydration_row_budget(
                TranscriptMode::Owned,
                /*terminal_height*/ 40,
                /*terminal_reflow_rows*/ None
            ),
            Some(120)
        );
        assert_eq!(
            initial_hydration_row_budget(
                TranscriptMode::Owned,
                /*terminal_height*/ 40,
                /*terminal_reflow_rows*/ Some(9_001)
            ),
            Some(120)
        );
    }

    #[test]
    fn terminal_hydration_keeps_the_configured_cap() {
        assert_eq!(
            initial_hydration_row_budget(
                TranscriptMode::Terminal,
                /*terminal_height*/ 40,
                /*terminal_reflow_rows*/ Some(60)
            ),
            Some(60)
        );
        assert_eq!(
            initial_hydration_row_budget(
                TranscriptMode::Terminal,
                /*terminal_height*/ 40,
                /*terminal_reflow_rows*/ None
            ),
            None
        );
    }

    #[test]
    fn loading_and_partial_states_count_as_unloaded_history() {
        for state in [
            TranscriptHistoryState::LoadingOlder,
            TranscriptHistoryState::LoadingBeginning,
            TranscriptHistoryState::Partial,
            TranscriptHistoryState::Failed,
        ] {
            assert!(state.has_unloaded_history(), "{state:?}");
        }
        for state in [
            TranscriptHistoryState::Idle,
            TranscriptHistoryState::Complete,
        ] {
            assert!(!state.has_unloaded_history(), "{state:?}");
        }
    }
}
