//! Find-in-transcript for the owned history window.
//!
//! Search runs over the entries the app has already loaded, walking the bounded layout cache so a
//! query only ever formats a few entries per step. Scanning is incremental: a step wraps a small
//! number of entries, returns whether more work remains, and the caller asks for another frame.
//! That keeps a long loaded history from being formatted in one keystroke, and it is the same shape
//! P4 needs when search has to continue into older pages.
//!
//! Matches are stored as content positions (entry identity plus byte offset), so jumping to one
//! uses the same anchor machinery as ordinary scrolling and survives prepends and resizes.

use super::*;
use ratatui::style::Modifier;
use ratatui::style::Style;

/// Entries wrapped per search step. Small enough to stay invisible in a frame budget.
const CELLS_PER_STEP: usize = 24;
/// Total entries one query scans. Older history is P4's job, so search reports what it covered.
const MAX_SCANNED_CELLS: usize = 400;

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
    exhausted: bool,
    /// Set by `Enter`: the query stays highlighted but stops accepting characters.
    accepted: bool,
}

impl SearchState {
    fn reset_scan(&mut self) {
        self.matches.clear();
        self.cursor = None;
        self.scanned = 0;
        self.exhausted = false;
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
    pub(super) fn search_needs_work(&self) -> bool {
        self.search.active
            && !self.search.exhausted
            && !self.search.query.is_empty()
            && self.search.scanned < MAX_SCANNED_CELLS
    }

    /// Scan one bounded batch of entries; returns whether more work remains.
    pub(crate) fn search_step(&mut self, cells: &[Arc<dyn HistoryCell>]) -> bool {
        if !self.search_needs_work() {
            return false;
        }
        let needle: Vec<char> = self.search.query.chars().collect();
        let total = cells.len();
        let end = (self.search.scanned + CELLS_PER_STEP).min(total);
        for index in self.search.scanned..end {
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
        self.search.scanned = end;
        if end >= total {
            self.search.exhausted = true;
            // Start on the newest match, which is nearest the live tail the reader came from.
            if self.search.cursor.is_none() && !self.search.matches.is_empty() {
                self.search.cursor = Some(self.search.matches.len() - 1);
                let target = self.search.matches[self.search.matches.len() - 1];
                self.focus_match(cells, target);
            }
        }
        self.search_needs_work()
    }

    /// Move to the next (newer) or previous (older) match.
    pub(super) fn search_advance(
        &mut self,
        cells: &[Arc<dyn HistoryCell>],
        newer: bool,
    ) -> bool {
        if self.search.matches.is_empty() {
            return false;
        }
        let last = self.search.matches.len() - 1;
        let cursor = match self.search.cursor {
            Some(cursor) if newer => (cursor + 1).min(last),
            Some(cursor) => cursor.saturating_sub(/*rhs*/ 1),
            None if newer => last,
            None => 0,
        };
        self.search.cursor = Some(cursor);
        let target = self.search.matches[cursor];
        self.focus_match(cells, target);
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

    /// One-line find status: query plus how far the scan and the cursor got.
    pub(super) fn search_status(&self) -> Option<String> {
        if !self.search.active {
            return None;
        }
        let position = match (self.search.cursor, self.search.matches.len()) {
            (Some(cursor), total) => format!("{}/{}", cursor + 1, total),
            (None, total) => format!("0/{total}"),
        };
        let suffix = if self.search.exhausted && self.search.scanned < MAX_SCANNED_CELLS {
            " (loaded history)"
        } else {
            ""
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
