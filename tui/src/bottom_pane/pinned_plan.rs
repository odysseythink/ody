//! Pinned live plan widget rendered above the composer in the bottom pane.
//!
//! Phase 1: this widget renders the current `UpdatePlanArgs` checklist using the same shared
//! rendering logic as `PlanUpdateCell` so the pinned plan matches the committed history entry.
//! It does not yet receive live data from the controller; that wiring is for Phase 2/3.

use ody_protocol::plan_tool::PlanItemArg;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Styled;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::widgets::WidgetRef;

use crate::history_cell::render_plan_steps;
use crate::render::line_utils::{prefix_lines, push_owned_lines};
use crate::render::renderable::Renderable;
use crate::wrapping::{adaptive_wrap_line, RtOptions};

/// Live pinned plan widget shown between the Working status line and the composer.
///
/// The widget owns a snapshot of the latest plan-update arguments and re-renders in place each
/// time `update_plan` is called, avoiding the stacked scrolling history cells used for committed
/// plan updates.
///
/// When the plan has many steps, folded mode (default) limits the visible height to `max_lines`
/// and hides completed steps first. Press `ctrl+e` to toggle `expanded` and see all steps.
#[derive(Debug, Clone)]
pub(crate) struct PinnedPlanWidget {
    explanation: Option<String>,
    plan: Vec<PlanItemArg>,
    /// Maximum visible lines in folded mode (including header).
    max_lines: usize,
    /// Whether all steps are visible (toggled via ctrl+e).
    expanded: bool,
}

impl Default for PinnedPlanWidget {
    fn default() -> Self {
        Self {
            explanation: None,
            plan: Vec::new(),
            max_lines: 8,
            expanded: false,
        }
    }
}

impl PinnedPlanWidget {
    /// Create a new empty pinned plan widget.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Replace the displayed plan with a new set of plan-update arguments.
    pub(crate) fn update(&mut self, args: ody_protocol::plan_tool::UpdatePlanArgs) {
        self.explanation = args.explanation;
        self.plan = args.plan;
        // Reset to folded when a new plan arrives.
        self.expanded = false;
    }

    /// Clear the displayed plan, leaving the widget empty.
    pub(crate) fn clear(&mut self) {
        self.explanation = None;
        self.plan.clear();
        self.expanded = false;
    }

    /// Toggle expanded / folded state.
    pub(crate) fn toggle_expanded(&mut self) {
        self.expanded = !self.expanded;
    }

    /// Whether the plan is currently expanded.
    pub(crate) fn is_expanded(&self) -> bool {
        self.expanded
    }

    /// Extract the current plan state as `UpdatePlanArgs`, or `None` if no plan is set.
    pub(crate) fn to_update_args(&self) -> Option<ody_protocol::plan_tool::UpdatePlanArgs> {
        if self.plan.is_empty() && self.explanation.is_none() {
            return None;
        }
        Some(ody_protocol::plan_tool::UpdatePlanArgs {
            explanation: self.explanation.clone(),
            plan: self.plan.clone(),
        })
    }

    /// Render the plan content into a vector of lines at the given width.
    fn display_lines(&self, width: u16) -> Vec<ratatui::text::Line<'static>> {
        let full = render_plan_steps(width, self.explanation.as_deref(), &self.plan);

        if self.expanded || full.len() <= self.max_lines {
            return full;
        }

        self.folded_lines(width, full.len())
    }

    /// Build a compact folded view when the plan has too many lines.
    fn folded_lines(&self, width: u16, _total_lines: usize) -> Vec<Line<'static>> {
        use ody_protocol::plan_tool::StepStatus;
        use ratatui::style::Style;

        let mut lines: Vec<Line<'static>> = vec![];
        let completed = self
            .plan
            .iter()
            .filter(|p| matches!(p.status, StepStatus::Completed))
            .count();
        let total = self.plan.len();

        // Header with progress
        lines.push(
            vec![
                "• ".dim(),
                "Updated Plan".bold(),
                format!("  ({completed}/{total} done)").dim(),
            ]
            .into(),
        );

        let wrap_width = width.saturating_sub(4).max(1) as usize;
        let mut indented: Vec<Line<'static>> = vec![];

        // Show explanation if present
        if let Some(expl) = self
            .explanation
            .as_ref()
            .map(|s| s.trim())
            .filter(|t| !t.is_empty())
        {
            let note = Line::from(expl.to_string().dim().italic());
            let wrapped = adaptive_wrap_line(&note, RtOptions::new(wrap_width));
            push_owned_lines(&wrapped, &mut indented);
        }

        // Budget: max_lines - 1 (header) - explanation lines
        let mut remaining = self
            .max_lines
            .saturating_sub(1)
            .saturating_sub(indented.len());

        // Show in-progress items first
        for item in self
            .plan
            .iter()
            .filter(|p| matches!(p.status, StepStatus::InProgress))
        {
            if remaining == 0 {
                break;
            }
            let (box_str, step_style) = ("□ ", Style::default().cyan().bold());
            let opts = RtOptions::new(wrap_width)
                .initial_indent(box_str.into())
                .subsequent_indent("  ".into());
            let step = Line::from(item.step.clone().set_style(step_style));
            let wrapped = adaptive_wrap_line(&step, opts);
            let mut out = Vec::new();
            push_owned_lines(&wrapped, &mut out);
            if out.len() <= remaining {
                let added = out.len();
                indented.extend(out);
                remaining = remaining.saturating_sub(added);
            } else {
                break;
            }
        }

        // Show pending items next
        for item in self
            .plan
            .iter()
            .filter(|p| matches!(p.status, StepStatus::Pending))
        {
            if remaining == 0 {
                break;
            }
            let (box_str, step_style) = ("□ ", Style::default().dim());
            let opts = RtOptions::new(wrap_width)
                .initial_indent(box_str.into())
                .subsequent_indent("  ".into());
            let step = Line::from(item.step.clone().set_style(step_style));
            let wrapped = adaptive_wrap_line(&step, opts);
            let mut out = Vec::new();
            push_owned_lines(&wrapped, &mut out);
            if out.len() <= remaining {
                let added = out.len();
                indented.extend(out);
                remaining = remaining.saturating_sub(added);
            } else {
                break;
            }
        }

        // Count explanation lines for hidden calculation
        let expl_lines = if let Some(e) = self
            .explanation
            .as_ref()
            .filter(|t| !t.trim().is_empty())
        {
            let note = Line::from(e.to_string().dim().italic());
            let wrapped = adaptive_wrap_line(&note, RtOptions::new(wrap_width));
            let mut out = Vec::new();
            push_owned_lines(&wrapped, &mut out);
            out.len()
        } else {
            0
        };
        let shown_visible = indented.len().saturating_sub(expl_lines);
        let hidden = total.saturating_sub(completed).saturating_sub(shown_visible);

        if hidden > 0 {
            indented.push(Line::from(
                format!("…+{hidden} more (ctrl+e to expand)").dim(),
            ));
        } else if completed > 0 {
            indented.push(Line::from(format!("✓ {completed} completed").dim()));
        }

        lines.extend(prefix_lines(indented, "  └ ".dim(), "    ".into()));

        lines
    }
}

impl Renderable for PinnedPlanWidget {
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
    use insta::assert_snapshot;
    use ody_protocol::plan_tool::PlanItemArg;
    use ody_protocol::plan_tool::StepStatus;
    use ody_protocol::plan_tool::UpdatePlanArgs;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;
    use ratatui::Terminal;

    use super::PinnedPlanWidget;
    use crate::history_cell::new_plan_update;
    use crate::history_cell::HistoryCell;
    use crate::render::renderable::Renderable;

    #[test]
    fn renders_pinned_plan_with_explanation_and_steps() {
        let widget = PinnedPlanWidget {
            explanation: Some("Phase 1 pinned plan".to_string()),
            plan: vec![
                PlanItemArg {
                    step: "Extract shared render function".to_string(),
                    status: StepStatus::Completed,
                },
                PlanItemArg {
                    step: "Create PinnedPlanWidget".to_string(),
                    status: StepStatus::InProgress,
                },
                PlanItemArg {
                    step: "Add snapshot tests".to_string(),
                    status: StepStatus::Pending,
                },
            ],
            max_lines: 8,
            expanded: false,
        };

        let mut terminal = Terminal::new(TestBackend::new(60, 10)).expect("terminal");
        terminal
            .draw(|f| widget.render(Rect::new(0, 0, 60, 10), f.buffer_mut()))
            .expect("draw");
        assert_snapshot!(terminal.backend());
    }

    #[test]
    fn renders_empty_pinned_plan() {
        let widget = PinnedPlanWidget::new();

        let mut terminal = Terminal::new(TestBackend::new(40, 5)).expect("terminal");
        terminal
            .draw(|f| widget.render(Rect::new(0, 0, 40, 5), f.buffer_mut()))
            .expect("draw");
        assert_snapshot!(terminal.backend());
    }

    #[test]
    fn desired_height_matches_lines() {
        let widget = PinnedPlanWidget {
            explanation: Some("Note".to_string()),
            plan: vec![
                PlanItemArg {
                    step: "Step one".to_string(),
                    status: StepStatus::Completed,
                },
                PlanItemArg {
                    step: "Step two".to_string(),
                    status: StepStatus::InProgress,
                },
            ],
            max_lines: 8,
            expanded: false,
        };

        // 1 header + 1 note + 2 steps = 4 lines (no wrapping at 80).
        assert_eq!(widget.desired_height(80), 4);
    }

    #[test]
    fn update_replaces_content() {
        let mut widget = PinnedPlanWidget::new();
        widget.update(UpdatePlanArgs {
            explanation: Some("New plan".to_string()),
            plan: vec![PlanItemArg {
                step: "Only step".to_string(),
                status: StepStatus::InProgress,
            }],
        });
        // Header + note prefix + step = 3 lines (note renders on the same indented prefix line).
        assert_eq!(widget.desired_height(80), 3);

        widget.clear();
        assert_eq!(widget.desired_height(80), 2); // header + (no steps provided)
    }

    #[test]
    fn pinned_plan_renders_identically_to_history_cell() {
        let update = UpdatePlanArgs {
            explanation: Some("Shared rendering contract check".to_string()),
            plan: vec![
                PlanItemArg {
                    step: "Extract shared render function".to_string(),
                    status: StepStatus::Completed,
                },
                PlanItemArg {
                    step: "Create PinnedPlanWidget".to_string(),
                    status: StepStatus::InProgress,
                },
                PlanItemArg {
                    step: "Add contract test".to_string(),
                    status: StepStatus::Pending,
                },
            ],
        };

        let pinned = PinnedPlanWidget {
            explanation: update.explanation.clone(),
            plan: update.plan.clone(),
            max_lines: 8,
            expanded: true,
        };
        let cell = new_plan_update(update);

        assert_eq!(
            pinned.display_lines(80),
            cell.display_lines(80),
            "PinnedPlanWidget and PlanUpdateCell must produce identical display_lines from render_plan_steps"
        );
    }
}
