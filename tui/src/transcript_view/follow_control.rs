//! Reserved-row affordances for the owned transcript.
//!
//! One row at the bottom of the history area is borrowed for a single line of feedback: the
//! return-to-latest hint while the reader is away from the bottom, or the find/selection/notice
//! status once P3's interaction needs it. Borrowing a row shrinks the painted window by one instead
//! of covering transcript content.

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

    /// Draw find status, action feedback or the selection hint in the reserved row.
    ///
    /// Unlike the return-to-bottom control this is left aligned, because it carries a variable
    /// amount of text (a query, a match counter) rather than one short label.
    pub(super) fn render_status_row(&self, area: Rect, buf: &mut Buffer, label: &str) {
        if area.is_empty() {
            return;
        }
        let width = u16::try_from(label.width()).unwrap_or(0).min(area.width);
        let target = Rect::new(area.x, area.y, width, /*height*/ 1);
        Paragraph::new(label.to_string())
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
