//! Bounded layout cache for recently displayed transcript entries.
//!
//! Only entries the viewport actually touched are ever wrapped, and only a bounded number of them
//! are retained. A cell that is not on screen and not being measured for scroll math is never
//! formatted, which is what keeps a 1000-turn session from laying out its whole backlog per frame.
//!
//! Reuse rules:
//! - within one frame, the entry is wrapped once and shared by measurement and painting;
//! - across frames, a cell whose transcript output is not time-dependent is reused as long as its
//!   identity, width, presentation and expansion state still match;
//! - cells with an animation tick are re-wrapped whenever the frame changes.

use std::sync::Arc;
use std::sync::Weak;

use super::*;

const MAX_CACHED_ENTRIES: usize = 64;
const MAX_CACHED_TEXT_BYTES: usize = 8 * 1024 * 1024;

/// How an entry is presented, beyond its identity and width.
///
/// Any change here means the wrapped rows differ, so the cached layout must be rebuilt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CellPresentation {
    separated: bool,
    expanded: bool,
    mode: HistoryRenderMode,
}

#[derive(Default)]
pub(super) struct LayoutCache {
    entries: Vec<CachedLayout>,
    frame: u64,
}

struct CachedLayout {
    source: Weak<dyn HistoryCell>,
    width: u16,
    presentation: CellPresentation,
    layout: Arc<TextLayout>,
    rendered_frame: u64,
    animation_tick: Option<u64>,
}

impl TranscriptView {
    /// Wrapped rows for entry `index`, wrapping it on demand and caching the result.
    ///
    /// `index == cells.len()` addresses the live tail, which is laid out through the same path so
    /// the tail participates in scroll math exactly like a committed entry.
    pub(super) fn layout(
        &mut self,
        cells: &[Arc<dyn HistoryCell>],
        index: usize,
    ) -> Option<Arc<TextLayout>> {
        if index > cells.len() {
            return None;
        }
        let width = self.area.width.max(/*other*/ 1);
        let Some(cell) = cells.get(index) else {
            let live = self.live.as_ref()?;
            if cells.is_empty() || self.live_continuation {
                return Some(Arc::clone(live));
            }
            if let Some(separated) = self.live_separated.as_ref() {
                return Some(Arc::clone(separated));
            }
            let lines = self.live_lines.clone().unwrap_or_default();
            let layout = Arc::new(TextLayout::new(lines, width).with_leading_separator());
            self.live_separated = Some(Arc::clone(&layout));
            return Some(layout);
        };

        let presentation = CellPresentation {
            separated: index > 0 && !cell.is_stream_continuation(),
            expanded: cell.is_expanded(),
            mode: self.mode,
        };
        let cell = Arc::clone(cell);
        let mode = self.mode;
        Some(self.cache.get(&cell, width, presentation, || {
            let layout = TextLayout::new(cell.display_hyperlink_lines_for_mode(width, mode), width);
            if presentation.separated {
                layout.with_leading_separator()
            } else {
                layout
            }
        }))
    }
}

impl LayoutCache {
    /// Advance the frame counter so intra-frame reuse can be distinguished from cross-frame reuse.
    pub(super) fn begin_frame(&mut self) {
        self.frame = self.frame.wrapping_add(/*rhs*/ 1);
    }

    fn get(
        &mut self,
        cell: &Arc<dyn HistoryCell>,
        width: u16,
        presentation: CellPresentation,
        render: impl FnOnce() -> TextLayout,
    ) -> Arc<TextLayout> {
        let source = Arc::downgrade(cell);
        let animation_tick = cell.transcript_animation_tick();
        // Stable cells (no animation tick) are safe to reuse across frames. Animated cells must be
        // re-wrapped for every new frame, so only same-frame lookups can hit.
        let reusable_across_frames = animation_tick.is_none();

        if let Some(index) = self.entries.iter().position(|entry| {
            entry.width == width
                && entry.presentation == presentation
                && entry.source.ptr_eq(&source)
                && (entry.rendered_frame == self.frame
                    || (reusable_across_frames && entry.animation_tick == animation_tick))
        }) {
            let layout = Arc::clone(&self.entries[index].layout);
            if index + 1 != self.entries.len() {
                let entry = self.entries.remove(index);
                self.entries.push(entry);
            }
            return layout;
        }

        let layout = Arc::new(render());
        self.entries.retain(|entry| !entry.source.ptr_eq(&source));
        self.entries.push(CachedLayout {
            source,
            width,
            presentation,
            layout: Arc::clone(&layout),
            rendered_frame: self.frame,
            animation_tick,
        });
        self.evict();
        layout
    }

    pub(super) fn clear(&mut self) {
        self.entries.clear();
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Drop cache entries whose source cell was dropped or replaced.
    pub(super) fn retain_live_sources(&mut self) {
        self.entries.retain(|entry| entry.source.strong_count() > 0);
    }

    fn evict(&mut self) {
        let mut bytes = self
            .entries
            .iter()
            .map(|entry| entry.layout.text().len())
            .sum::<usize>();
        // Keep a single oversized entry rather than re-wrapping it on every frame while visible.
        while self.entries.len() > 1
            && (self.entries.len() > MAX_CACHED_ENTRIES || bytes > MAX_CACHED_TEXT_BYTES)
        {
            bytes -= self.entries.remove(/*index*/ 0).layout.text().len();
        }
    }
}
