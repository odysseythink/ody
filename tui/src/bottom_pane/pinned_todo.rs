//! Pinned live plan widget rendered above the composer in the bottom pane.
//!
//! Phase 1: this widget renders the current `UpdatePlanArgs` checklist using the same shared
//! rendering logic as the legacy `PlanUpdateCell` so the pinned todo matches the history entry
//! rendering. It does not yet receive live data from the controller; that wiring is for Phase 2/3.

use ody_protocol::plan_tool::PlanItemArg;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Paragraph;
use ratatui::widgets::WidgetRef;

use crate::render::renderable::Renderable;

/// Live pinned todo widget shown between the Working status line and the composer.
///
/// The widget owns a snapshot of the latest plan-update arguments and re-renders in place each
/// time `update_plan` is called, avoiding the stacked scrolling history cells used for committed
/// plan updates.
///
/// When the plan has many steps, folded mode (default) limits the visible height to `max_lines`
/// and hides completed steps first. Press `ctrl+e` to toggle `expanded` and see all steps.
#[derive(Debug, Clone)]
pub(crate) struct PinnedTodoWidget {
    explanation: Option<String>,
    plan: Vec<PlanItemArg>,
    /// Maximum visible lines in folded mode (including header).
    max_lines: usize,
    /// Whether all steps are visible (toggled via ctrl+e).
    expanded: bool,
    /// Folded-mode monotone watermark: the largest folded height (in lines)
    /// this widget instance has ever rendered, capped at `max_lines`.
    /// `None` until the first folded render. Drops with the widget instance
    /// (set_pinned_todo(None) / new thread); never shrinks while alive.
    /// Interior mutability: `Renderable` methods are `&self`, so the folded
    /// render path records the watermark through a `Cell`.
    watermark: std::cell::Cell<Option<usize>>,
}

impl Default for PinnedTodoWidget {
    fn default() -> Self {
        Self {
            explanation: None,
            plan: Vec::new(),
            max_lines: 8,
            expanded: false,
            watermark: std::cell::Cell::new(None),
        }
    }
}

impl PinnedTodoWidget {
    /// Create a new empty pinned todo widget.
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

    /// Toggle expanded / folded state.
    pub(crate) fn toggle_expanded(&mut self) {
        self.expanded = !self.expanded;
    }

    /// Clear the displayed plan, leaving the widget empty.
    #[cfg(test)]
    pub(crate) fn clear(&mut self) {
        self.explanation = None;
        self.plan.clear();
        self.expanded = false;
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
    ///
    /// Folded mode uses a monotone watermark: the visible height never
    /// shrinks while this widget instance is alive, capped at `max_lines`.
    /// An empty plan (no tasks) hides the widget entirely.
    fn display_lines(&self, width: u16) -> Vec<ratatui::text::Line<'static>> {
        // No tasks: don't take up any space.
        if self.plan.is_empty() {
            return vec![];
        }

        if self.expanded {
            return plan_layout::full(width, self.explanation.as_deref(), &self.plan);
        }

        plan_layout::folded(
            width,
            self.explanation.as_deref(),
            &self.plan,
            self.max_lines,
            &self.watermark,
        )
    }
}

/// Private rendering routines for the pinned todo widget.
///
/// These are pure functions over the plan data; the only state they touch is
/// the folded-mode watermark passed in by the widget.
mod plan_layout {
    use ody_protocol::plan_tool::PlanItemArg;
    use ody_protocol::plan_tool::StepStatus;
    use ratatui::style::Style;
    use ratatui::style::Styled;
    use ratatui::style::Stylize;
    use ratatui::text::Line;
    use ratatui::text::Span;
    use std::cell::Cell;

    use crate::history_cell::render_plan_steps;
    use crate::render::line_utils::{prefix_lines, push_owned_lines};
    use crate::style::accent_style;
    use crate::wrapping::{RtOptions, adaptive_wrap_line};

    /// Full rendering: delegate to the shared history-cell renderer so the
    /// pinned widget and history entries stay identical.
    pub(super) fn full(
        width: u16,
        explanation: Option<&str>,
        plan: &[PlanItemArg],
    ) -> Vec<Line<'static>> {
        render_plan_steps(width, explanation, plan)
    }

    /// Folded rendering with a monotone watermark. The only place that
    /// knows about the watermark: bump it to the largest candidate seen,
    /// then pad the body up to it.
    pub(super) fn folded(
        width: u16,
        explanation: Option<&str>,
        plan: &[PlanItemArg],
        max_lines: usize,
        watermark: &Cell<Option<usize>>,
    ) -> Vec<Line<'static>> {
        let full_lines = full(width, explanation, plan);
        if full_lines.is_empty() {
            return vec![];
        }

        let candidate = full_lines.len().min(max_lines);
        let new_watermark = watermark.get().unwrap_or(0).max(candidate);
        watermark.set(Some(new_watermark));

        let mut body = if full_lines.len() <= max_lines {
            full_lines
        } else {
            compact_fold(width, explanation, plan, max_lines)
        };

        while body.len() < new_watermark {
            body.push(Line::from(""));
        }
        body
    }

    /// Build a compact folded view when the plan has too many lines.
    pub(super) fn compact_fold(
        width: u16,
        explanation: Option<&str>,
        plan: &[PlanItemArg],
        max_lines: usize,
    ) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = vec![];
        let completed = plan
            .iter()
            .filter(|p| matches!(p.status, StepStatus::Completed))
            .count();
        let total = plan.len();

        // Header with progress
        lines.push(
            vec![
                "• ".dim(),
                Span::styled("Todo", accent_style()),
                format!("  ({completed}/{total} done)").dim(),
            ]
            .into(),
        );

        let wrap_width = width.saturating_sub(4).max(1) as usize;
        let mut indented: Vec<Line<'static>> = vec![];

        // Show explanation if present and remember how many lines it consumes.
        let expl_lines = if let Some(expl) = explanation.map(|s| s.trim()).filter(|t| !t.is_empty())
        {
            let note = Line::from(expl.to_string().dim().italic());
            let wrapped = adaptive_wrap_line(&note, RtOptions::new(wrap_width));
            let before = indented.len();
            push_owned_lines(&wrapped, &mut indented);
            indented.len().saturating_sub(before)
        } else {
            0
        };

        // Reserve one line for the footer (ellipsis or completed summary)
        // whenever the plan is non-empty, so the folded view never exceeds
        // max_lines.
        let reserve_footer = total > 0;
        let mut remaining = max_lines
            .saturating_sub(1) // header
            .saturating_sub(indented.len()) // explanation
            .saturating_sub(if reserve_footer { 1 } else { 0 });

        // Show in-progress items first
        for item in plan
            .iter()
            .filter(|p| matches!(p.status, StepStatus::InProgress))
        {
            if remaining == 0 {
                break;
            }
            let (box_str, step_style) = ("☐ ", Style::default().cyan().bold());
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
        for item in plan
            .iter()
            .filter(|p| matches!(p.status, StepStatus::Pending))
        {
            if remaining == 0 {
                break;
            }
            let (box_str, step_style) = ("☐ ", Style::default().dim());
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

        let shown_visible = indented.len().saturating_sub(expl_lines);
        let hidden = total
            .saturating_sub(completed)
            .saturating_sub(shown_visible);

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
    use insta::assert_snapshot;
    use ody_protocol::plan_tool::PlanItemArg;
    use ody_protocol::plan_tool::StepStatus;
    use ody_protocol::plan_tool::UpdatePlanArgs;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;

    use super::PinnedTodoWidget;
    use crate::history_cell::HistoryCell;
    use crate::history_cell::new_plan_update;
    use crate::render::renderable::Renderable;
    fn make_plan(total: usize, completed: usize) -> Vec<ody_protocol::plan_tool::PlanItemArg> {
        (0..total)
            .map(|i| ody_protocol::plan_tool::PlanItemArg {
                step: format!("Step {i}"),
                status: if i < completed {
                    ody_protocol::plan_tool::StepStatus::Completed
                } else if i == completed {
                    ody_protocol::plan_tool::StepStatus::InProgress
                } else {
                    ody_protocol::plan_tool::StepStatus::Pending
                },
            })
            .collect()
    }

    fn args_with(plan: Vec<ody_protocol::plan_tool::PlanItemArg>) -> UpdatePlanArgs {
        UpdatePlanArgs {
            explanation: None,
            plan,
        }
    }

    #[test]
    fn watermark_monotone_across_updates_same_instance() {
        let mut widget = PinnedTodoWidget::new();
        let total = 20;
        let mut heights = Vec::new();
        for completed in 0..=total {
            widget.update(args_with(make_plan(total, completed)));
            let h = widget.desired_height(80);
            let lines_len = widget.display_lines(80).len() as u16;
            assert_eq!(h, lines_len, "desired_height must equal display_lines len");
            heights.push(h);
        }
        assert_eq!(heights[0], 8, "initial folded height must be max_lines");
        for w in heights.windows(2) {
            assert!(w[1] >= w[0], "height must not shrink: {:?}", heights);
        }
        assert_eq!(*heights.last().unwrap(), 8);
    }

    #[test]
    fn watermark_caps_at_max_lines_after_all_complete() {
        let mut widget = PinnedTodoWidget::new();
        widget.update(args_with(make_plan(20, 0)));
        assert_eq!(widget.desired_height(80), 8);
        widget.update(args_with(make_plan(20, 20)));
        assert_eq!(
            widget.desired_height(80),
            8,
            "height must stay capped at max_lines after all tasks complete"
        );
    }

    #[test]
    fn expanded_does_not_update_watermark() {
        let mut widget = PinnedTodoWidget::new();
        widget.update(args_with(make_plan(20, 10)));
        let folded_before = widget.desired_height(80);
        assert_eq!(folded_before, 8);

        widget.toggle_expanded();
        let expanded_height = widget.desired_height(80);
        assert!(
            expanded_height > 8,
            "expanded must show all lines: got {expanded_height}"
        );

        widget.toggle_expanded();
        assert_eq!(
            widget.desired_height(80),
            folded_before,
            "collapsing must return to the pre-expansion folded watermark"
        );
    }

    #[test]
    fn renders_pinned_todo_with_explanation_and_steps() {
        let widget = PinnedTodoWidget {
            explanation: Some("Phase 1 pinned plan".to_string()),
            plan: vec![
                PlanItemArg {
                    step: "Extract shared render function".to_string(),
                    status: StepStatus::Completed,
                },
                PlanItemArg {
                    step: "Create PinnedTodoWidget".to_string(),
                    status: StepStatus::InProgress,
                },
                PlanItemArg {
                    step: "Add snapshot tests".to_string(),
                    status: StepStatus::Pending,
                },
            ],
            max_lines: 8,
            expanded: false,
            watermark: std::cell::Cell::new(None),
        };

        let mut terminal = Terminal::new(TestBackend::new(60, 10)).expect("terminal");
        terminal
            .draw(|f| widget.render(Rect::new(0, 0, 60, 10), f.buffer_mut()))
            .expect("draw");
        assert_snapshot!(terminal.backend());
    }

    #[test]
    fn renders_empty_pinned_todo() {
        let widget = PinnedTodoWidget::new();

        let mut terminal = Terminal::new(TestBackend::new(40, 5)).expect("terminal");
        terminal
            .draw(|f| widget.render(Rect::new(0, 0, 40, 5), f.buffer_mut()))
            .expect("draw");
        assert_snapshot!(terminal.backend());
    }

    #[test]
    fn desired_height_matches_lines() {
        let widget = PinnedTodoWidget {
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
            watermark: std::cell::Cell::new(None),
        };

        // 1 header + 1 note + 2 steps = 4 lines (no wrapping at 80).
        assert_eq!(widget.desired_height(80), 4);
    }

    #[test]
    fn update_replaces_content() {
        let mut widget = PinnedTodoWidget::new();
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
        assert_eq!(widget.desired_height(80), 0); // empty plan -> hidden
    }

    #[test]
    fn folded_mode_respects_max_lines() {
        let widget = PinnedTodoWidget {
            explanation: Some("A short note".to_string()),
            plan: (0..20)
                .map(|i| PlanItemArg {
                    step: format!("Step {i} with enough text to avoid wrapping at 80 columns"),
                    status: if i < 10 {
                        StepStatus::Completed
                    } else if i == 10 {
                        StepStatus::InProgress
                    } else {
                        StepStatus::Pending
                    },
                })
                .collect(),
            max_lines: 8,
            expanded: false,
            watermark: std::cell::Cell::new(None),
        };

        assert!(
            widget.desired_height(80) <= 8,
            "folded pinned plan must not exceed max_lines: got {}",
            widget.desired_height(80)
        );
    }

    #[test]
    fn fixed_height_for_short_plan_when_all_completed() {
        let mut widget = PinnedTodoWidget::new();
        widget.update(args_with(vec![
            PlanItemArg {
                step: "Step one".to_string(),
                status: StepStatus::Completed,
            },
            PlanItemArg {
                step: "Step two".to_string(),
                status: StepStatus::InProgress,
            },
            PlanItemArg {
                step: "Step three".to_string(),
                status: StepStatus::Pending,
            },
        ]));
        let initial_height = widget.desired_height(80);
        assert_eq!(initial_height, 4); // 1 header + 3 steps

        widget.update(args_with(vec![
            PlanItemArg {
                step: "Step one".to_string(),
                status: StepStatus::Completed,
            },
            PlanItemArg {
                step: "Step two".to_string(),
                status: StepStatus::Completed,
            },
            PlanItemArg {
                step: "Step three".to_string(),
                status: StepStatus::Completed,
            },
        ]));
        assert_eq!(
            widget.desired_height(80),
            initial_height,
            "short plan height must stay fixed after all tasks complete"
        );
    }

    #[test]
    fn fixed_height_for_long_plan_when_all_completed() {
        let mut widget = PinnedTodoWidget::new();
        widget.update(args_with(make_plan(20, 10)));
        let initial_height = widget.desired_height(80);
        assert_eq!(initial_height, 8);

        widget.update(args_with(make_plan(20, 20)));
        assert_eq!(
            widget.desired_height(80),
            initial_height,
            "long plan height must stay at max_lines after all tasks complete"
        );
    }

    #[test]
    fn pinned_todo_renders_identically_to_history_cell() {
        let update = UpdatePlanArgs {
            explanation: Some("Shared rendering contract check".to_string()),
            plan: vec![
                PlanItemArg {
                    step: "Extract shared render function".to_string(),
                    status: StepStatus::Completed,
                },
                PlanItemArg {
                    step: "Create PinnedTodoWidget".to_string(),
                    status: StepStatus::InProgress,
                },
                PlanItemArg {
                    step: "Add contract test".to_string(),
                    status: StepStatus::Pending,
                },
            ],
        };

        let pinned = PinnedTodoWidget {
            explanation: update.explanation.clone(),
            plan: update.plan.clone(),
            max_lines: 8,
            expanded: true,
            watermark: std::cell::Cell::new(None),
        };
        let cell = new_plan_update(update);

        assert_eq!(
            pinned.display_lines(80),
            cell.display_lines(80),
            "PinnedTodoWidget and PlanUpdateCell must produce identical display_lines from render_plan_steps"
        );
    }
}
