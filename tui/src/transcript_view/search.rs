//! Find-in-transcript for the owned history window.
//!
//! Search runs over the entries the app has already loaded, walking the bounded layout cache so a
//! query only ever formats a few entries per step. Scanning is incremental: a step wraps a small
//! number of entries, returns whether more work remains, and the caller asks for another frame.
//! That keeps a long loaded history from being formatted in one keystroke.
//!
//! When the scan reaches the oldest loaded entry and the server still has older turns, the search
//! does not give up: it parks in `SearchProgress::AwaitingHistory` and the app loads one more page.
//! The scan then resumes from the newly inserted page, walking backwards until it finds a hit, runs
//! out of history, or hits the per-query scan cap. Matches are stored as content positions (entry
//! identity plus byte offset), so jumping to one uses the same anchor machinery as ordinary
//! scrolling and survives prepends and resizes.

use super::*;
use crate::transcript_history::TranscriptHistoryState;
use ratatui::style::Modifier;
use ratatui::style::Style;

/// Entries wrapped per search step. Small enough to stay invisible in a frame budget.
const CELLS_PER_STEP: usize = 24;
/// Total entries one query scans before it stops on its own.
///
/// Not a hard cap on reach: reaching the oldest loaded entry with older pages still on the server
/// parks the scan in `AwaitingHistory` instead of stopping, so a query can cross page boundaries.
/// The counter bounds only the entries scanned within one continuous run, which is what keeps a
/// pathological query from wrapping an unbounded history in a single gesture.
const MAX_SCANNED_CELLS: usize = 400;

/// Which way the scan is walking.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ScanDirection {
    /// Toward older entries, prepending as pages arrive.
    Older,
    /// Toward newer entries; never crosses a page boundary because the tail is always loaded.
    Newer,
}

/// Where the incremental scan stands.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum SearchProgress {
    #[default]
    Idle,
    /// Scanning entries inside the loaded window.
    Scanning,
    /// Reached the oldest loaded entry while older pages still exist. The app should load one.
    AwaitingHistory,
    /// The page the scan was waiting for failed to load. `^P` retries.
    Failed,
    /// Every entry the server has was scanned.
    Exhausted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct SearchMatch {
    key: EntryKey,
    index: usize,
    offset: usize,
}

#[derive(Default)]
pub(super) struct SearchState {
    active: bool,
    query: String,
    matches: Vec<SearchMatch>,
    cursor: Option<usize>,
    scanned: usize,
    progress: SearchProgress,
    /// Entry the scan should resume at, as a content position so a prepend keeps it valid.
    resume: Option<SearchMatch>,
    /// First entry the current leg must not scan, as a content position. Set when a scan resumes
    /// over a freshly prepended page: everything from this entry onward was covered before the
    /// page arrived, so the leg stops here instead of re-scanning it and duplicating its matches.
    scan_stop: Option<SearchMatch>,
    direction: Option<ScanDirection>,
    /// Set by `Enter`: the query stays highlighted but stops accepting characters.
    accepted: bool,
}

impl SearchState {
    fn reset_scan(&mut self) {
        self.matches.clear();
        self.cursor = None;
        self.scanned = 0;
        self.progress = SearchProgress::Idle;
        self.resume = None;
        self.scan_stop = None;
        self.direction = None;
    }

    fn query_is_set(&self) -> bool {
        !self.query.is_empty()
    }
}

impl TranscriptView {
    pub(crate) fn search_active(&self) -> bool {
        self.search.active
    }

    pub(super) fn search_editing(&self) -> bool {
        self.search.active && !self.search.accepted
    }

    /// Open find mode over the loaded transcript.
    pub(crate) fn begin_search(&mut self) {
        self.search = SearchState {
            active: true,
            ..SearchState::default()
        };
        self.clear_selection();
    }

    /// Close find mode. Matches and highlights go away; the reading position stays put.
    pub(super) fn cancel_search(&mut self) {
        self.search = SearchState::default();
    }

    /// Stop editing but keep the current query and its highlights.
    pub(super) fn accept_search(&mut self) {
        if !self.search.query.is_empty() {
            self.search.accepted = true;
        }
    }

    pub(super) fn push_search_char(&mut self, ch: char) {
        self.search.query.push(ch);
        self.search.accepted = false;
        self.search.reset_scan();
    }

    pub(super) fn pop_search_char(&mut self) {
        self.search.query.pop();
        self.search.accepted = false;
        self.search.reset_scan();
    }

    /// Whether another scan step is needed for the current query.
    ///
    /// `AwaitingHistory` is not "needs work" here: no amount of further stepping helps until a page
    /// arrives, so this stays false and the caller stops asking for frames. `search_needs_history`
    /// is the hook that starts the load instead.
    pub(crate) fn search_needs_work(&self) -> bool {
        self.search.active
            && matches!(self.search.progress, SearchProgress::Idle | SearchProgress::Scanning)
            && !self.search.query.is_empty()
            && self.search.scanned < MAX_SCANNED_CELLS
    }

    /// Whether the scan is parked waiting for an older page.
    pub(crate) fn search_awaiting_history(&self) -> bool {
        self.search.active
            && matches!(
                self.search.progress,
                SearchProgress::AwaitingHistory | SearchProgress::Failed
            )
    }

    /// Report that the page the scan was waiting for never arrived.
    ///
    /// The scan keeps its query and its matches so `^P` can retry without re-typing.
    pub(crate) fn mark_search_page_failed(&mut self) {
        if self.search.progress == SearchProgress::AwaitingHistory {
            self.search.progress = SearchProgress::Failed;
        }
    }

    /// Whether the query has run out of entries to scan *and* the server has no more.
    ///
    /// Only this state justifies telling the reader "no matches"; `AwaitingHistory` means the
    /// answer is still unknown.
    pub(super) fn search_exhausted(&self) -> bool {
        self.search.progress == SearchProgress::Exhausted
    }

    /// Scan one bounded batch of entries; returns whether more work remains.
    pub(crate) fn search_step(&mut self, cells: &[Arc<dyn HistoryCell>]) -> bool {
        if !self.search_needs_work() {
            return false;
        }
        let needle: Vec<char> = self.search.query.chars().collect();
        let total = cells.len();
        // The leg ends at the boundary of the region scanned before the current page arrived, or
        // at the newest loaded entry when there is no such boundary.
        let stop = self
            .search
            .scan_stop
            .map(|boundary| {
                self.resolve(
                    cells,
                    Anchor {
                        key: boundary.key,
                        index: boundary.index,
                        offset: boundary.offset,
                        row_bias: 0,
                    },
                )
            })
            .unwrap_or(total)
            .min(total);
        let start = match self.search.resume {
            // Resume at the position the previous step stopped at, re-resolved by identity so a
            // prepend between steps does not shift the window.
            Some(resume) => self.resolve(
                cells,
                Anchor {
                    key: resume.key,
                    index: resume.index,
                    offset: resume.offset,
                    row_bias: 0,
                },
            ),
            None => 0,
        }
        .min(stop);
        self.search.progress = SearchProgress::Scanning;
        self.search.direction = Some(ScanDirection::Older);
        let end = start.saturating_add(CELLS_PER_STEP).min(stop);
        for index in start..end {
            let Some(layout) = self.layout(cells, index) else {
                continue;
            };
            let text = layout.text().to_string();
            let key = self.entry_key(cells, index);
            for offset in find_matches(&text, &needle) {
                self.search.matches.push(SearchMatch {
                    key,
                    index,
                    offset,
                });
            }
        }
        self.search.scanned = self.search.scanned.saturating_add(end.saturating_sub(start));
        if end < stop {
            self.search.resume = Some(SearchMatch {
                key: self.entry_key(cells, end),
                index: end,
                offset: 0,
            });
            return self.search_needs_work();
        }
        self.search.scan_stop = None;
        self.finish_older_scan(cells);
        self.search_needs_work()
    }

    /// Decide what reaching the oldest loaded entry means.
    ///
    /// With older pages still on the server this parks in `AwaitingHistory` so the app loads one and
    /// the scan continues; otherwise the query really is done.
    fn finish_older_scan(&mut self, cells: &[Arc<dyn HistoryCell>]) {
        if self.history.has_unloaded_history() && self.search.scanned < MAX_SCANNED_CELLS {
            self.search.progress = SearchProgress::AwaitingHistory;
            self.search.resume = None;
            return;
        }
        self.search.progress = SearchProgress::Exhausted;
        self.search.resume = None;
        // Start on the newest match, which is nearest the live tail the reader came from.
        if self.search.cursor.is_none() && !self.search.matches.is_empty() {
            self.search.cursor = Some(self.search.matches.len() - 1);
            let target = self.search.matches[self.search.matches.len() - 1];
            self.focus_match(cells, target);
        }
    }

    /// Resume a parked scan over a page that was just prepended.
    pub(super) fn search_history_loaded(
        &mut self,
        cells: &[Arc<dyn HistoryCell>],
        inserted: Range<usize>,
    ) {
        if !self.search.active || self.search.query.is_empty() {
            return;
        }
        if inserted.is_empty() {
            // The page landed but produced no cells; nothing new to scan here. If the server is
            // still paginating, stay parked so the next page gets its own chance.
            if self.search.progress == SearchProgress::AwaitingHistory
                && !self.history.has_unloaded_history()
            {
                self.search.progress = SearchProgress::Exhausted;
            }
            return;
        }
        if self.search.progress != SearchProgress::AwaitingHistory {
            // A page arrived while a scan leg was in flight (the reader paged by scrolling, not
            // by find). The new page sits in front of the resume point, so continuing the leg
            // would skip it: restart the scan over the widened window. The per-query cap bounds
            // the repeated work.
            self.search.reset_scan();
            return;
        }
        // Continue from the oldest row of the inserted page, stopping at the region that was
        // already scanned before this page arrived. The per-query scan budget refreshes with each
        // page: the cap bounds one continuous run, and a cross-page continuation is a new run.
        let index = inserted.start;
        self.search.resume = Some(SearchMatch {
            key: self.entry_key(cells, index),
            index,
            offset: 0,
        });
        self.search.scan_stop = (inserted.end < cells.len()).then(|| SearchMatch {
            key: self.entry_key(cells, inserted.end),
            index: inserted.end,
            offset: 0,
        });
        self.search.scanned = 0;
        self.search.progress = SearchProgress::Scanning;
    }

    /// Force a fresh scan of the whole loaded window, e.g. after cells were removed.
    pub(crate) fn restart_search(&mut self) {
        self.search.reset_scan();
    }

    /// Whether find should keep scanning with no further page available.
    pub(super) fn search_found_nothing(&self) -> bool {
        self.search.active
            && self.search.query_is_set()
            && self.search.progress == SearchProgress::Exhausted
            && self.search.matches.is_empty()
    }

    /// Move to the next (newer) or previous (older) match.
    ///
    /// Walking past the oldest known match while older pages remain un-parked the scan, so `^P` at
    /// the end of the loaded window keeps looking instead of silently stopping at the boundary.
    pub(super) fn search_advance(
        &mut self,
        cells: &[Arc<dyn HistoryCell>],
        newer: bool,
    ) -> bool {
        if !newer && self.search.progress == SearchProgress::Failed {
            // A retry gesture: the previous page never arrived, so ask for it again.
            self.search.progress = SearchProgress::AwaitingHistory;
            return true;
        }
        if self.search.matches.is_empty() {
            return false;
        }
        let last = self.search.matches.len() - 1;
        let at_oldest = self.search.cursor == Some(0);
        let cursor = match self.search.cursor {
            Some(cursor) if newer => (cursor + 1).min(last),
            Some(cursor) => cursor.saturating_sub(/*rhs*/ 1),
            None if newer => last,
            None => 0,
        };
        self.search.cursor = Some(cursor);
        let target = self.search.matches[cursor];
        self.focus_match(cells, target);
        if !newer
            && at_oldest
            && self.search.progress == SearchProgress::Exhausted
            && self.history.has_unloaded_history()
            && self.search.scanned < MAX_SCANNED_CELLS
        {
            // The reader walked past the oldest hit after the scan stopped at its per-run cap
            // while older pages remain: park again so paging continues. The loaded window is
            // already fully scanned at this point, so nothing needs re-walking — and rewinding
            // `resume` backwards would duplicate matches.
            self.search.progress = SearchProgress::AwaitingHistory;
        }
        true
    }

    /// Put one match at the top of the viewport through the ordinary anchor machinery.
    fn focus_match(&mut self, cells: &[Arc<dyn HistoryCell>], target: SearchMatch) {
        let index = self.resolve(
            cells,
            Anchor {
                key: target.key,
                index: target.index,
                offset: target.offset,
                row_bias: 0,
            },
        );
        self.position = Position::Reading(Anchor {
            key: self.entry_key(cells, index),
            index,
            offset: target.offset,
            row_bias: 0,
        });
        self.tail_visible = false;
        self.unseen_activity = false;
    }

    #[cfg(test)]
    pub(super) fn search_match_count(&self) -> usize {
        self.search.matches.len()
    }

    /// One-line find status: query, scan progress and how far the cursor got.
    pub(super) fn search_status(&self) -> Option<String> {
        if !self.search.active {
            return None;
        }
        let position = match (self.search.cursor, self.search.matches.len()) {
            (Some(cursor), total) => format!("{}/{}", cursor + 1, total),
            (None, total) => format!("0/{total}"),
        };
        let suffix = match self.search.progress {
            SearchProgress::AwaitingHistory => " (loading earlier messages…)",
            SearchProgress::Failed => " (load failed · ^P retries)",
            // Only claim the search covered everything loaded when it really stopped there.
            SearchProgress::Exhausted if self.search.scanned >= MAX_SCANNED_CELLS => " (scan limit)",
            SearchProgress::Exhausted => " (all messages)",
            _ => "",
        };
        Some(format!(
            " Find: {}{suffix}  {position}  ^P older · ^N newer · esc close",
            self.search.query
        ))
    }

    /// Highlight every match on the rows the last frame recorded, emphasising the current one.
    pub(super) fn highlight_search(
        &mut self,
        cells: &[Arc<dyn HistoryCell>],
        buf: &mut Buffer,
    ) {
        if !self.search.active || self.search.matches.is_empty() {
            return;
        }
        let area = self.area;
        if area.is_empty() {
            return;
        }
        let needle_len = self.search.query.chars().count();
        if needle_len == 0 {
            return;
        }
        let current = self.search.cursor.and_then(|c| self.search.matches.get(c).copied());
        for offset in 0..usize::from(area.height) {
            let Some((index, row)) = self.visible.get(offset).copied() else {
                continue;
            };
            let matches: Vec<SearchMatch> = self
                .search
                .matches
                .iter()
                .copied()
                .filter(|item| item.index == index)
                .collect();
            if matches.is_empty() {
                continue;
            }
            let Some(layout) = self.layout(cells, index) else {
                continue;
            };
            let Some(source) = layout.row_source(row) else {
                continue;
            };
            let content_width = layout.row_content_width(row).unwrap_or(0);
            let y = area.y.saturating_add(u16::try_from(offset).unwrap_or(0));
            for item in matches {
                let end = source.end.min(item.offset.saturating_add(needle_len));
                if item.offset >= source.end || end <= source.start {
                    continue;
                }
                let low = layout.column_for_offset(row, item.offset);
                let high = layout
                    .column_for_offset(row, end)
                    .max(low.saturating_add(/*rhs*/ 1))
                    .min(content_width);
                if low >= high {
                    continue;
                }
                let style = if current.is_some_and(|current| current == item) {
                    Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
                } else {
                    Style::default().add_modifier(Modifier::REVERSED)
                };
                for column in low..high {
                    let x = area.x.saturating_add(column);
                    if x < area.right() {
                        buf[(x, y)].set_style(style);
                    }
                }
            }
        }
    }
}

/// Byte offsets of every ASCII-case-insensitive occurrence of `needle` in `haystack`.
fn find_matches(haystack: &str, needle: &[char]) -> Vec<usize> {
    if needle.is_empty() {
        return Vec::new();
    }
    let chars: Vec<(usize, char)> = haystack.char_indices().collect();
    if chars.len() < needle.len() {
        return Vec::new();
    }
    let mut offsets = Vec::new();
    for start in 0..=chars.len() - needle.len() {
        let matched = chars[start..start + needle.len()]
            .iter()
            .zip(needle)
            .all(|((_, candidate), expected)| candidate.eq_ignore_ascii_case(expected));
        if matched {
            offsets.push(chars[start].0);
        }
    }
    offsets
}

#[cfg(test)]
mod tests {
    use super::find_matches;

    #[test]
    fn find_matches_is_ascii_case_insensitive_and_offset_accurate() {
        let needle: Vec<char> = "ab".chars().collect();
        assert_eq!(find_matches("AB ab xAba", &needle), vec![0, 3, 7]);
        assert!(find_matches("", &needle).is_empty());
        assert!(find_matches("a", &needle).is_empty());
    }

    #[test]
    fn find_matches_keeps_offsets_for_multibyte_text() {
        let needle: Vec<char> = "错".chars().collect();
        // "错误" starts after a two-byte and a three-byte character.
        let haystack = "é错错";
        assert_eq!(find_matches(haystack, &needle), vec![2, 5]);
    }
}
