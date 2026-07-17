//! Pinned live plan widget rendered above the composer in the bottom pane.
//!
//! This widget mirrors the ody-code `TodoPanel` experience: a compact,
//! live-updating TODO list shown between the status line and the composer.
//! It keeps items in their original order, caps the visible list to
//! `MAX_VISIBLE`, and uses the same selection/summary logic as the TypeScript
//! implementation.

use std::collections::HashSet;

use ody_protocol::plan_tool::PlanItemArg;
use ody_protocol::plan_tool::StepStatus;
use ody_protocol::plan_tool::UpdatePlanArgs;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::widgets::WidgetRef;

use crate::render::renderable::Renderable;
use crate::style::accent_style;

/// Maximum number of TODO rows visible at once (not counting the border and
/// title). This matches the ody-code `TodoPanel` cap.
const MAX_VISIBLE: usize = 5;

/// Live pinned todo widget shown between the status line and the composer.
#[derive(Debug, Clone, Default)]
pub(crate) struct PinnedTodoWidget {
    plan: Vec<PlanItemArg>,
}

impl PinnedTodoWidget {
    /// Create a new empty pinned todo widget.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Replace the displayed plan with a new set of plan-update arguments.
    pub(crate) fn update(&mut self, args: UpdatePlanArgs) {
        self.plan = args.plan;
    }

    /// Clear the displayed plan, leaving the widget empty.
    #[cfg(test)]
    pub(crate) fn clear(&mut self) {
        self.plan.clear();
    }

    /// Extract the current plan state as `UpdatePlanArgs`, or `None` if no plan is set.
    pub(crate) fn to_update_args(&self) -> Option<UpdatePlanArgs> {
        if self.plan.is_empty() {
            return None;
        }
        Some(UpdatePlanArgs {
            explanation: None,
            plan: self.plan.clone(),
        })
    }

    /// Returns whether the plan is empty.
    pub(crate) fn is_empty(&self) -> bool {
        self.plan.is_empty()
    }

    /// Render the plan content into a vector of lines at the given width.
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        if self.plan.is_empty() {
            return vec![];
        }

        let mut lines: Vec<Line<'static>> = vec![
            Line::from(vec![Span::styled(
                "─".repeat(width as usize),
                Style::default().dim(),
            )]),
            Line::from(vec![
                Span::styled(" ", Style::default()),
                Span::styled("Todo", accent_style()),
            ]),
        ];

        let visible = select_visible_todos(&self.plan);
        for item in &visible.rows {
            lines.push(render_row(item));
        }

        if visible.hidden > 0 {
            lines.push(Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(
                    format_summary_line(&self.plan, visible.hidden),
                    Style::default().dim(),
                ),
            ]));
        }

        lines
    }
}

/// Selected subset of todos visible within the widget cap.
struct VisibleTodos {
    rows: Vec<PlanItemArg>,
    hidden: usize,
}

/// Pick which todos to render when the list exceeds [`MAX_VISIBLE`].
///
/// The selector is order-agnostic — the plan tool keeps whatever order the model
/// produced, so an interleaved sequence is possible and must still yield
/// `MAX_VISIBLE` rows when enough exist.
///
/// Strategy:
/// 1. Include every `in_progress` item (capped at `MAX_VISIBLE`).
/// 2. Fill remaining slots with "what's next" — the earliest `pending` items in
///    their original positions — while reserving one slot for "what just
///    finished" — the latest `done` item — when both kinds exist. If one side has
///    too few candidates, the other expands.
///
/// Items are returned in their original order.
fn select_visible_todos(plan: &[PlanItemArg]) -> VisibleTodos {
    if plan.len() <= MAX_VISIBLE {
        return VisibleTodos {
            rows: plan.to_vec(),
            hidden: 0,
        };
    }

    let mut in_progress: Vec<usize> = vec![];
    let mut pending: Vec<usize> = vec![];
    let mut done: Vec<usize> = vec![];
    for (i, item) in plan.iter().enumerate() {
        match item.status {
            StepStatus::InProgress => in_progress.push(i),
            StepStatus::Pending => pending.push(i),
            StepStatus::Completed => done.push(i),
        }
    }

    let mut picked = HashSet::new();
    for &i in in_progress.iter().take(MAX_VISIBLE) {
        picked.insert(i);
    }

    if picked.len() < MAX_VISIBLE {
        // Most recent done first; earliest pending first.
        let done_candidates: Vec<usize> = done.iter().copied().rev().collect();
        let pending_candidates = pending;

        let remaining = MAX_VISIBLE - picked.len();
        let (done_count, pending_count) = if done_candidates.is_empty() {
            (0, remaining.min(pending_candidates.len()))
        } else if pending_candidates.is_empty() {
            (remaining.min(done_candidates.len()), 0)
        } else {
            let mut done_count = 1usize;
            let mut pending_count = (remaining - 1).min(pending_candidates.len());
            if pending_count < remaining - 1 {
                done_count = done_candidates.len().min(remaining - pending_count);
            }
            (done_count, pending_count)
        };

        for &i in done_candidates.iter().take(done_count) {
            picked.insert(i);
        }
        for &i in pending_candidates.iter().take(pending_count) {
            picked.insert(i);
        }
    }

    let mut sorted_idx: Vec<usize> = picked.into_iter().collect();
    sorted_idx.sort_unstable();

    VisibleTodos {
        hidden: plan.len() - sorted_idx.len(),
        rows: sorted_idx.into_iter().map(|i| plan[i].clone()).collect(),
    }
}

fn format_summary_line(plan: &[PlanItemArg], hidden: usize) -> String {
    let mut finished = 0;
    let mut left = 0;
    for item in plan {
        if matches!(item.status, StepStatus::Completed) {
            finished += 1;
        } else {
            left += 1;
        }
    }
    format!("… +{hidden} more, {finished} finished, {left} left")
}

fn render_row(item: &PlanItemArg) -> Line<'static> {
    let (marker, marker_style, title_style) = match item.status {
        StepStatus::InProgress => ("●", accent_style(), Style::default().bold()),
        StepStatus::Completed => (
            "✓",
            Style::default().green(),
            Style::default().crossed_out().dim(),
        ),
        StepStatus::Pending => ("○", Style::default().dim(), Style::default()),
    };

    Line::from(vec![
        Span::styled("  ", Style::default()),
        Span::styled(marker, marker_style),
        Span::styled(" ", Style::default()),
        Span::styled(item.step.clone(), title_style),
    ])
}

impl Renderable for PinnedTodoWidget {
    fn desired_height(&self, width: u16) -> u16 {
        self.display_lines(width).len() as u16
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let lines = self.display_lines(area.width);
        Paragraph::new(lines).render_ref(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ody_protocol::plan_tool::{PlanItemArg, StepStatus, UpdatePlanArgs};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Modifier;

    fn item(step: &str, status: StepStatus) -> PlanItemArg {
        PlanItemArg {
            step: step.to_string(),
            status,
        }
    }

    fn update_with(plan: Vec<PlanItemArg>) -> UpdatePlanArgs {
        UpdatePlanArgs {
            explanation: None,
            plan,
        }
    }

    fn render_text(widget: &PinnedTodoWidget, width: u16) -> Vec<String> {
        let height = widget.desired_height(width);
        if height == 0 {
            return Vec::new();
        }
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buf[(x, y)].symbol().chars().next().unwrap_or(' '))
                    .collect()
            })
            .collect()
    }

    fn row_titles(rows: &[PlanItemArg]) -> Vec<&str> {
        rows.iter().map(|i| i.step.as_str()).collect()
    }

    #[test]
    fn empty_plan_hides_widget() {
        let widget = PinnedTodoWidget::new();
        assert!(widget.is_empty());
        assert_eq!(widget.desired_height(80), 0);
        assert!(render_text(&widget, 80).is_empty());
    }

    #[test]
    fn renders_border_title_and_rows() {
        let mut widget = PinnedTodoWidget::new();
        widget.update(update_with(vec![
            item("Investigate parser", StepStatus::Completed),
            item("Add tests", StepStatus::InProgress),
            item("Open PR", StepStatus::Pending),
        ]));

        let lines = render_text(&widget, 40);
        assert_eq!(lines.len(), 5); // border + title + 3 rows
        assert!(lines[0].starts_with('─'));
        assert!(lines[1].contains("Todo"));
        assert!(lines[2].contains("✓ Investigate parser"));
        assert!(lines[3].contains("● Add tests"));
        assert!(lines[4].contains("○ Open PR"));
    }

    #[test]
    fn shows_all_rows_and_no_summary_when_count_within_max() {
        let mut widget = PinnedTodoWidget::new();
        widget.update(update_with(vec![
            item("a", StepStatus::Completed),
            item("b", StepStatus::InProgress),
            item("c", StepStatus::Pending),
            item("d", StepStatus::Pending),
            item("e", StepStatus::Pending),
        ]));

        let lines = render_text(&widget, 40);
        assert_eq!(lines.len(), 7); // border + title + 5 rows
        assert!(lines[2].contains("a"));
        assert!(lines[6].contains("e"));
        assert!(!lines.join("\n").contains("more"));
    }

    #[test]
    fn overflow_appends_summary_footer() {
        let mut widget = PinnedTodoWidget::new();
        widget.update(update_with(vec![
            item("t0", StepStatus::Completed),
            item("t1", StepStatus::InProgress),
            item("t2", StepStatus::Pending),
            item("t3", StepStatus::Pending),
            item("t4", StepStatus::Pending),
            item("t5", StepStatus::Pending),
            item("t6", StepStatus::Pending),
        ]));

        let lines = render_text(&widget, 40);
        assert_eq!(lines.len(), 8); // border + title + 5 rows + summary
        assert!(lines.last().unwrap().contains("+2 more"));
    }

    #[test]
    fn summary_line_counts_finished_and_left() {
        let mut widget = PinnedTodoWidget::new();
        widget.update(update_with(vec![
            item("d0", StepStatus::Completed),
            item("d1", StepStatus::Completed),
            item("ip", StepStatus::InProgress),
            item("p0", StepStatus::Pending),
            item("p1", StepStatus::Pending),
            item("p2", StepStatus::Pending),
            item("p3", StepStatus::Pending),
        ]));

        let text = render_text(&widget, 80).join("\n");
        assert!(text.contains("+2 more, 2 finished, 5 left"));
    }

    #[test]
    fn update_replaces_content() {
        let mut widget = PinnedTodoWidget::new();
        widget.update(update_with(vec![item("old", StepStatus::Pending)]));
        widget.update(update_with(vec![item("new", StepStatus::InProgress)]));
        let text = render_text(&widget, 80).join("\n");
        assert!(text.contains("● new"));
        assert!(!text.contains("old"));
    }

    #[test]
    fn clear_empties_widget() {
        let mut widget = PinnedTodoWidget::new();
        widget.update(update_with(vec![item("x", StepStatus::Pending)]));
        widget.clear();
        assert_eq!(widget.desired_height(80), 0);
    }

    #[test]
    fn max_height_with_summary_is_capped() {
        let mut widget = PinnedTodoWidget::new();
        widget.update(update_with(
            (0..20)
                .map(|i| item(&format!("Step {i}"), StepStatus::Pending))
                .collect(),
        ));
        assert_eq!(widget.desired_height(80), 8); // border + title + 5 + summary
    }

    #[test]
    fn desired_height_matches_display_lines() {
        let mut widget = PinnedTodoWidget::new();
        widget.update(update_with(vec![
            item("One", StepStatus::Completed),
            item("Two", StepStatus::InProgress),
        ]));
        assert_eq!(
            widget.desired_height(80),
            widget.display_lines(80).len() as u16
        );
    }

    #[test]
    fn in_progress_row_uses_bold_title() {
        let mut widget = PinnedTodoWidget::new();
        widget.update(update_with(vec![item("Active", StepStatus::InProgress)]));
        let lines = widget.display_lines(80);
        let row = lines.last().unwrap();
        let title_span = row.spans.last().unwrap();
        assert!(title_span.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn completed_row_uses_strikethrough() {
        let mut widget = PinnedTodoWidget::new();
        widget.update(update_with(vec![item("Done", StepStatus::Completed)]));
        let lines = widget.display_lines(80);
        let row = lines.last().unwrap();
        let title_span = row.spans.last().unwrap();
        assert!(
            title_span
                .style
                .add_modifier
                .contains(Modifier::CROSSED_OUT)
        );
    }

    #[test]
    fn select_visible_todos_returns_all_when_small() {
        let plan = vec![
            item("a", StepStatus::Completed),
            item("b", StepStatus::InProgress),
            item("c", StepStatus::Pending),
        ];
        let visible = select_visible_todos(&plan);
        assert_eq!(row_titles(&visible.rows), vec!["a", "b", "c"]);
        assert_eq!(visible.hidden, 0);
    }

    #[test]
    fn select_visible_todos_with_one_in_progress_and_done_before() {
        let plan = vec![
            item("d1", StepStatus::Completed),
            item("d2", StepStatus::Completed),
            item("d3", StepStatus::Completed),
            item("ip", StepStatus::InProgress),
            item("p1", StepStatus::Pending),
            item("p2", StepStatus::Pending),
            item("p3", StepStatus::Pending),
            item("p4", StepStatus::Pending),
            item("p5", StepStatus::Pending),
        ];
        let visible = select_visible_todos(&plan);
        let titles = row_titles(&visible.rows);
        assert_eq!(titles, vec!["d3", "ip", "p1", "p2", "p3"]);
    }

    #[test]
    fn select_visible_todos_with_one_in_progress_and_no_done() {
        let plan = vec![
            item("ip", StepStatus::InProgress),
            item("p1", StepStatus::Pending),
            item("p2", StepStatus::Pending),
            item("p3", StepStatus::Pending),
            item("p4", StepStatus::Pending),
            item("p5", StepStatus::Pending),
            item("p6", StepStatus::Pending),
        ];
        let visible = select_visible_todos(&plan);
        let titles = row_titles(&visible.rows);
        assert_eq!(titles, vec!["ip", "p1", "p2", "p3", "p4"]);
    }

    #[test]
    fn select_visible_todos_fills_done_when_few_pending() {
        let plan = vec![
            item("d1", StepStatus::Completed),
            item("d2", StepStatus::Completed),
            item("d3", StepStatus::Completed),
            item("d4", StepStatus::Completed),
            item("d5", StepStatus::Completed),
            item("ip", StepStatus::InProgress),
            item("p1", StepStatus::Pending),
        ];
        let visible = select_visible_todos(&plan);
        let titles = row_titles(&visible.rows);
        assert_eq!(titles, vec!["d3", "d4", "d5", "ip", "p1"]);
    }

    #[test]
    fn select_visible_todos_all_pending_shows_first_five() {
        let plan: Vec<_> = (0..8)
            .map(|i| item(&format!("p{i}"), StepStatus::Pending))
            .collect();
        let visible = select_visible_todos(&plan);
        let titles = row_titles(&visible.rows);
        assert_eq!(titles, vec!["p0", "p1", "p2", "p3", "p4"]);
    }

    #[test]
    fn select_visible_todos_all_done_shows_last_five() {
        let plan: Vec<_> = (0..8)
            .map(|i| item(&format!("d{i}"), StepStatus::Completed))
            .collect();
        let visible = select_visible_todos(&plan);
        let titles = row_titles(&visible.rows);
        assert_eq!(titles, vec!["d3", "d4", "d5", "d6", "d7"]);
    }

    #[test]
    fn select_visible_todos_mixed_done_and_pending_without_in_progress() {
        let plan = vec![
            item("d1", StepStatus::Completed),
            item("d2", StepStatus::Completed),
            item("d3", StepStatus::Completed),
            item("p1", StepStatus::Pending),
            item("p2", StepStatus::Pending),
            item("p3", StepStatus::Pending),
            item("p4", StepStatus::Pending),
            item("p5", StepStatus::Pending),
        ];
        let visible = select_visible_todos(&plan);
        let titles = row_titles(&visible.rows);
        assert_eq!(titles, vec!["d3", "p1", "p2", "p3", "p4"]);
    }

    #[test]
    fn select_visible_todos_interleaved_keeps_original_order() {
        let plan = vec![
            item("p0", StepStatus::Pending),
            item("d0", StepStatus::Completed),
            item("p1", StepStatus::Pending),
            item("d1", StepStatus::Completed),
            item("p2", StepStatus::Pending),
            item("d2", StepStatus::Completed),
            item("p3", StepStatus::Pending),
        ];
        let visible = select_visible_todos(&plan);
        assert_eq!(visible.rows.len(), 5);
        assert_eq!(visible.hidden, 2);
        assert_eq!(
            visible
                .rows
                .iter()
                .filter(|i| matches!(i.status, StepStatus::Pending))
                .count(),
            4
        );
        assert_eq!(
            visible
                .rows
                .iter()
                .filter(|i| matches!(i.status, StepStatus::Completed))
                .count(),
            1
        );
    }

    #[test]
    fn select_visible_todos_caps_many_in_progress() {
        let plan: Vec<_> = (0..7)
            .map(|i| item(&format!("ip{i}"), StepStatus::InProgress))
            .collect();
        let visible = select_visible_todos(&plan);
        let titles = row_titles(&visible.rows);
        assert_eq!(titles, vec!["ip0", "ip1", "ip2", "ip3", "ip4"]);
    }
}
