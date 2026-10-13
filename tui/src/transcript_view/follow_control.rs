//! Return-to-latest affordance for the owned transcript.
//!
//! The hint only appears when the reader is away from the bottom of the transcript. It borrows the
//! last row of the history area, which shrinks the painted window by one row instead of covering
//! transcript content; pointer interaction for this control lands in P3 together with the rest of
//! the transcript input routing.

use super::*;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthStr;

/// Longest first, so the widest hint that fits the viewport wins.
const HINTS: [&str; 4] = [" ↓ Back to bottom ", " ↓ Bottom ", " ↓ More ", " ↓ "];
const ACTIVITY_HINTS: [&str; 4] = [" ↓ New activity ", " ↓ New ", " ↓↓ ", " ↓ "];

impl TranscriptView {
    pub(super) fn render_return_to_bottom(&mut self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        let Some(label) = self.return_to_bottom_label(area.width) else {
            return;
        };
        let width = u16::try_from(label.width()).unwrap_or(0).min(area.width);
        let target = Rect::new(
            area.x + area.width.saturating_sub(width) / 2,
            area.y,
            width,
            /*height*/ 1,
        );
        Paragraph::new(label)
            .style(Style::default().add_modifier(Modifier::DIM))
            .render(target, buf);
    }

    fn return_to_bottom_label(&self, width: u16) -> Option<&'static str> {
        if width == 0 || self.area.is_empty() || !self.needs_return_to_bottom() {
            return None;
        }
        let hints = if self.has_unseen_activity() {
            ACTIVITY_HINTS
        } else {
            HINTS
        };
        hints
            .into_iter()
            .find(|label| label.width() <= usize::from(width))
    }
}
