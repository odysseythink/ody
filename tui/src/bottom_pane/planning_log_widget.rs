//! Structured planning log widget rendered above the composer in the bottom pane.
//!
//! This widget surfaces Plan/Design mode planning progress (rigor reminders, split checks,
//! completeness gates, mode transitions, and subagent delegation) in a compact, collapsible
//! panel so users perceive planning activity while regular assistant text remains suppressed.

use std::collections::VecDeque;

use ody_app_server_protocol::PlanModeLogDeltaNotification;
use ody_app_server_protocol::PlanModeLogKind;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::widgets::WidgetRef;

use crate::line_truncation::truncate_line_with_ellipsis_if_overflow;
use crate::render::renderable::Renderable;
use crate::wrapping::RtOptions;
use crate::wrapping::adaptive_wrap_lines;

/// Maximum number of recent planning log entries retained in the widget.
const MAX_ENTRIES: usize = 4;

/// Live structured planning log shown above the composer while in Plan/Design mode.
#[derive(Debug, Clone)]
pub(crate) struct PlanningLogWidget {
    entries: VecDeque<PlanningLogEntry>,
    expanded: bool,
}

#[derive(Debug, Clone)]
struct PlanningLogEntry {
    kind: PlanModeLogKind,
    message: String,
    detail: Option<String>,
}

impl Default for PlanningLogWidget {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
            expanded: false,
        }
    }
}

impl PlanningLogWidget {
    /// Create a new empty planning log widget.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Push a new planning log delta into the widget.
    ///
    /// Older entries are dropped once the retention limit is reached.
    pub(crate) fn push(&mut self, notification: PlanModeLogDeltaNotification) {
        self.entries.push_back(PlanningLogEntry {
            kind: notification.kind,
            message: notification.message,
            detail: notification.detail,
        });
        while self.entries.len() > MAX_ENTRIES {
            self.entries.pop_front();
        }
    }

    /// Toggle expanded / folded state.
    pub(crate) fn toggle_expanded(&mut self) {
        self.expanded = !self.expanded;
    }

    /// Clear all entries, leaving the widget empty.
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.expanded = false;
    }

    /// Returns true when the widget has no entries to display.
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Number of visible entries currently retained.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the widget is currently expanded.
    #[cfg(test)]
    pub(crate) fn is_expanded(&self) -> bool {
        self.expanded
    }

    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        if self.entries.is_empty() {
            return Vec::new();
        }

        if self.expanded {
            self.render_expanded(width)
        } else {
            self.render_collapsed(width)
        }
    }

    fn render_collapsed(&self, width: u16) -> Vec<Line<'static>> {
        // Collapsed view shows only the latest entry so the composer keeps as much
        // vertical room as possible.
        let entry = self.entries.back().expect("non-empty entries");
        let line = self.render_entry_line(entry, width);
        vec![truncate_line_with_ellipsis_if_overflow(
            line,
            width as usize,
        )]
    }

    fn render_expanded(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        for entry in &self.entries {
            lines.push(self.render_entry_line(entry, width));
            if let Some(detail) = &entry.detail {
                lines.extend(wrap_detail(width, detail));
            }
        }
        lines
    }

    fn render_entry_line(&self, entry: &PlanningLogEntry, width: u16) -> Line<'static> {
        let (prefix, prefix_style) = kind_prefix(entry.kind);
        let line = Line::from(vec![
            Span::styled(prefix, prefix_style),
            Span::raw(" "),
            Span::styled(entry.message.clone(), Style::default().dim()),
        ]);
        if width == 0 {
            return line;
        }
        adaptive_wrap_lines(std::iter::once(line), RtOptions::new(width as usize))
            .into_iter()
            .next()
            .unwrap_or_else(Line::default)
    }
}

fn wrap_detail(width: u16, detail: &str) -> Vec<Line<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let wrap_width = (width as usize).saturating_sub(4).max(1);
    let line = Line::from(detail.to_string().dim().italic());
    adaptive_wrap_lines(std::iter::once(line), RtOptions::new(wrap_width))
        .into_iter()
        .map(|mut line| {
            line.spans.insert(0, "    ".into());
            line
        })
        .collect()
}

fn kind_prefix(kind: PlanModeLogKind) -> (&'static str, Style) {
    match kind {
        PlanModeLogKind::RigorReminder => ("Rigor", Style::default().yellow().bold()),
        PlanModeLogKind::SplitStarted => ("Split", Style::default().cyan().bold()),
        PlanModeLogKind::SplitContinued => ("Split", Style::default().cyan()),
        PlanModeLogKind::SplitCompleted => ("Split", Style::default().green().bold()),
        PlanModeLogKind::FinalReview => ("Review", Style::default().magenta().bold()),
        PlanModeLogKind::DesignCompletenessCheck => ("Design", Style::default().blue().bold()),
        PlanModeLogKind::ModeTransition => ("Mode", Style::default().green().bold()),
        PlanModeLogKind::SubAgentDelegation => ("Subagent", Style::default().cyan().bold()),
    }
}

impl Renderable for PlanningLogWidget {
    fn desired_height(&self, width: u16) -> u16 {
        self.display_lines(width).len() as u16
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let lines = self.display_lines(area.width);
        if lines.is_empty() {
            return;
        }
        Paragraph::new(lines).render_ref(area, buf);
    }
}
