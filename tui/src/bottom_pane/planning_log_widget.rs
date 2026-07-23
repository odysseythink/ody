//! Structured planning log widget rendered above the composer in the bottom pane.
//!
//! This widget surfaces Plan/Design mode planning progress (rigor reminders, split checks,
//! completeness gates, mode transitions, and subagent delegation) in a compact, collapsible
//! panel so users perceive planning activity while regular assistant text remains suppressed.

use std::cell::Cell;
use std::collections::VecDeque;
use std::time::Duration;
use std::time::Instant;

use ody_app_server_protocol::PlanModeLogDeltaNotification;
use ody_app_server_protocol::PlanModeLogKind;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::widgets::WidgetRef;

use crate::line_truncation::truncate_line_with_ellipsis_if_overflow;
use crate::render::renderable::Renderable;
use crate::tui::FrameRequester;
use crate::wrapping::RtOptions;
use crate::wrapping::adaptive_wrap_lines;

/// Maximum number of recent planning log entries retained in the widget.
const MAX_ENTRIES: usize = 4;

/// Duration of the breath animation for a newly pushed planning log entry.
const BREATH_DURATION: Duration = Duration::from_millis(1800);

/// Number of full breathing cycles within the animation window.
///
/// Using 1.5 cycles ensures the curve ends at a bright peak, so the prefix settles at its
/// normal color rather than a dim state.
const BREATH_CYCLE_COUNT: f32 = 1.5;

/// Minimum brightness factor applied to the prefix color during the breath.
const BREATH_MIN_FACTOR: f32 = 0.45;

/// Target frame budget for the breath animation (~30 fps).
const BREATH_FRAME_BUDGET: Duration = Duration::from_millis(33);

/// Threshold above which a frame interval is considered slow.
const BREATH_SLOW_FRAME_THRESHOLD: Duration = Duration::from_millis(200);

/// Number of consecutive slow frames before the animation is disabled.
const BREATH_SLOW_FRAME_LIMIT: u8 = 2;

/// Live structured planning log shown above the composer while in Plan/Design mode.
#[derive(Debug, Clone)]
pub(crate) struct PlanningLogWidget {
    entries: VecDeque<PlanningLogEntry>,
    expanded: bool,
    breath: BreathAnimation,
}

#[derive(Debug, Clone)]
struct PlanningLogEntry {
    kind: PlanModeLogKind,
    message: String,
    detail: Option<String>,
    arrival: Instant,
}

/// Internal state that drives the brightness breath animation on the latest prefix.
///
/// Uses `Cell` for interior mutability because the widget is rendered through the
/// `Renderable::render(&self, ...)` interface, which only receives an immutable reference.
#[derive(Debug, Clone)]
struct BreathAnimation {
    frame_requester: FrameRequester,
    animations_enabled: bool,
    last_frame_time: Cell<Option<Instant>>,
    consecutive_slow_frames: Cell<u8>,
}

/// Original RGB base color for a log entry prefix, used to compute dimmed variants without
/// accumulating quantization drift.
#[derive(Debug, Clone, Copy)]
enum BreathBaseColor {
    Rgb { r: f32, g: f32, b: f32 },
    Unadjustable,
}

impl PlanningLogWidget {
    /// Create a new empty planning log widget with the given animation controls.
    pub(crate) fn new(frame_requester: FrameRequester, animations_enabled: bool) -> Self {
        Self {
            entries: VecDeque::new(),
            expanded: false,
            breath: BreathAnimation::new(frame_requester, animations_enabled),
        }
    }

    /// Push a new planning log delta into the widget.
    ///
    /// Older entries are dropped once the retention limit is reached.
    pub(crate) fn push(&mut self, notification: PlanModeLogDeltaNotification) {
        self.entries.push_back(PlanningLogEntry {
            kind: notification.kind,
            message: notification.message,
            detail: notification.detail,
            arrival: Instant::now(),
        });
        while self.entries.len() > MAX_ENTRIES {
            self.entries.pop_front();
        }
        self.breath.reset_slow_frame_detection();
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

    fn display_lines(&self, width: u16, now: Instant) -> Vec<Line<'static>> {
        if self.entries.is_empty() {
            return Vec::new();
        }

        if self.expanded {
            self.render_expanded(width, now)
        } else {
            self.render_collapsed(width, now)
        }
    }

    fn render_collapsed(&self, width: u16, now: Instant) -> Vec<Line<'static>> {
        // Collapsed view shows only the latest entry so the composer keeps as much
        // vertical room as possible.
        let entry = self.entries.back().expect("non-empty entries");
        let line = self.render_entry_line(entry, width, now, /*is_latest*/ true);
        vec![truncate_line_with_ellipsis_if_overflow(line, width as usize)]
    }

    fn render_expanded(&self, width: u16, now: Instant) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let latest_idx = self.entries.len().saturating_sub(1);
        for (idx, entry) in self.entries.iter().enumerate() {
            lines.push(self.render_entry_line(entry, width, now, idx == latest_idx));
            if let Some(detail) = &entry.detail {
                lines.extend(wrap_detail(width, detail));
            }
        }
        lines
    }

    fn render_entry_line(
        &self,
        entry: &PlanningLogEntry,
        width: u16,
        now: Instant,
        is_latest: bool,
    ) -> Line<'static> {
        let (prefix, prefix_style) = kind_prefix(entry.kind);
        let prefix_style = if is_latest {
            let factor = self.breath.breath_factor(now, entry.arrival);
            if let Some(dark_fg) =
                apply_breath_to_color(kind_prefix_breath_base_color(entry.kind), factor)
            {
                prefix_style.fg(dark_fg)
            } else {
                prefix_style
            }
        } else {
            prefix_style
        };
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

impl BreathAnimation {
    fn new(frame_requester: FrameRequester, animations_enabled: bool) -> Self {
        Self {
            frame_requester,
            animations_enabled,
            last_frame_time: Cell::new(None),
            consecutive_slow_frames: Cell::new(0),
        }
    }

    fn is_breathing(&self, now: Instant, arrival: Instant) -> bool {
        if !self.animations_enabled {
            return false;
        }
        let elapsed = now.saturating_duration_since(arrival);
        elapsed < BREATH_DURATION
            && self.consecutive_slow_frames.get() < BREATH_SLOW_FRAME_LIMIT
    }

    fn breath_factor(&self, now: Instant, arrival: Instant) -> f32 {
        if !self.animations_enabled {
            return 1.0;
        }
        if self.consecutive_slow_frames.get() >= BREATH_SLOW_FRAME_LIMIT {
            return 1.0;
        }
        let elapsed = now.saturating_duration_since(arrival);
        if elapsed >= BREATH_DURATION {
            return 1.0;
        }

        let progress = elapsed.as_secs_f32() / BREATH_DURATION.as_secs_f32();
        let phase = progress * BREATH_CYCLE_COUNT * 2.0 * std::f32::consts::PI;
        let cosine_env = (1.0 - phase.cos()) / 2.0;
        let fade_window = 1.0 - progress;
        let amplitude = cosine_env * fade_window;
        1.0 - (1.0 - BREATH_MIN_FACTOR) * amplitude
    }

    fn schedule_next_frame(&self, now: Instant) {
        if !self.animations_enabled {
            return;
        }
        if self.consecutive_slow_frames.get() >= BREATH_SLOW_FRAME_LIMIT {
            return;
        }

        let mut slow_count = self.consecutive_slow_frames.get();
        if let Some(last) = self.last_frame_time.get() {
            let elapsed = now.saturating_duration_since(last);
            if elapsed > BREATH_SLOW_FRAME_THRESHOLD {
                slow_count += 1;
                if slow_count >= BREATH_SLOW_FRAME_LIMIT {
                    self.consecutive_slow_frames.set(slow_count);
                    return;
                }
            } else {
                slow_count = 0;
            }
        }
        self.consecutive_slow_frames.set(slow_count);
        self.last_frame_time.set(Some(now));
        self.frame_requester.schedule_frame_in(BREATH_FRAME_BUDGET);
    }

    fn reset_slow_frame_detection(&self) {
        self.last_frame_time.set(None);
        self.consecutive_slow_frames.set(0);
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

fn kind_prefix_breath_base_color(kind: PlanModeLogKind) -> BreathBaseColor {
    match kind {
        PlanModeLogKind::RigorReminder => BreathBaseColor::Rgb {
            r: 255.0,
            g: 255.0,
            b: 0.0,
        },
        PlanModeLogKind::SplitStarted | PlanModeLogKind::SplitContinued => BreathBaseColor::Rgb {
            r: 0.0,
            g: 255.0,
            b: 255.0,
        },
        PlanModeLogKind::SplitCompleted | PlanModeLogKind::ModeTransition => BreathBaseColor::Rgb {
            r: 0.0,
            g: 255.0,
            b: 0.0,
        },
        PlanModeLogKind::FinalReview => BreathBaseColor::Rgb {
            r: 255.0,
            g: 0.0,
            b: 255.0,
        },
        PlanModeLogKind::DesignCompletenessCheck => BreathBaseColor::Rgb {
            r: 0.0,
            g: 128.0,
            b: 255.0,
        },
        PlanModeLogKind::SubAgentDelegation => BreathBaseColor::Rgb {
            r: 0.0,
            g: 255.0,
            b: 255.0,
        },
    }
}

fn apply_breath_to_color(base: BreathBaseColor, factor: f32) -> Option<Color> {
    let BreathBaseColor::Rgb { r, g, b } = base else {
        return None;
    };
    Some(Color::Rgb(
        (r * factor).clamp(0.0, 255.0) as u8,
        (g * factor).clamp(0.0, 255.0) as u8,
        (b * factor).clamp(0.0, 255.0) as u8,
    ))
}

impl Renderable for PlanningLogWidget {
    fn desired_height(&self, width: u16) -> u16 {
        self.display_lines(width, Instant::now()).len() as u16
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let now = Instant::now();
        let lines = self.display_lines(area.width, now);
        if lines.is_empty() {
            return;
        }
        Paragraph::new(lines).render_ref(area, buf);
        if let Some(entry) = self.entries.back() {
            if self.breath.is_breathing(now, entry.arrival) {
                self.breath.schedule_next_frame(now);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_records_arrival_and_widget_holds_frame_requester() {
        let requester = FrameRequester::test_dummy();
        let mut widget = PlanningLogWidget::new(requester.clone(), true);
        let before = Instant::now();
        widget.push(PlanModeLogDeltaNotification {
            thread_id: "t".into(),
            turn_id: "t".into(),
            event_id: "e".into(),
            occurred_at_ms: 0,
            kind: PlanModeLogKind::RigorReminder,
            message: "msg".into(),
            detail: None,
        });
        let after = Instant::now();

        assert_eq!(widget.len(), 1);
        let entry = widget.entries.front().unwrap();
        assert!(entry.arrival >= before && entry.arrival <= after);
        assert!(widget.has_frame_requester_for_tests());
        assert!(widget.animations_enabled_for_tests());
    }

    #[test]
    fn all_kinds_have_breath_base_color() {
        for kind in [
            PlanModeLogKind::RigorReminder,
            PlanModeLogKind::SplitStarted,
            PlanModeLogKind::SplitContinued,
            PlanModeLogKind::SplitCompleted,
            PlanModeLogKind::FinalReview,
            PlanModeLogKind::DesignCompletenessCheck,
            PlanModeLogKind::ModeTransition,
            PlanModeLogKind::SubAgentDelegation,
        ] {
            let base = kind_prefix_breath_base_color(kind);
            assert!(
                matches!(base, BreathBaseColor::Rgb { .. }),
                "kind {:?} should have a breathable RGB base color",
                kind
            );
        }
    }

    #[test]
    fn breath_factor_boundaries_and_disabled() {
        let requester = FrameRequester::test_dummy();
        let breath = BreathAnimation::new(requester, true);
        let arrival = Instant::now();

        assert_eq!(breath.breath_factor(arrival, arrival), 1.0);
        assert_eq!(
            breath.breath_factor(arrival + BREATH_DURATION + Duration::from_millis(1), arrival),
            1.0
        );

        let mid = breath.breath_factor(arrival + BREATH_DURATION / 2, arrival);
        assert!(
            mid >= BREATH_MIN_FACTOR && mid <= 1.0,
            "mid factor {} should be in [{}, 1.0]",
            mid,
            BREATH_MIN_FACTOR
        );

        let disabled = BreathAnimation::new(FrameRequester::test_dummy(), false);
        assert_eq!(
            disabled.breath_factor(arrival + Duration::from_millis(100), arrival),
            1.0
        );
    }

    impl PlanningLogWidget {
        fn has_frame_requester_for_tests(&self) -> bool {
            let _ = self.breath.frame_requester.clone();
            true
        }

        fn animations_enabled_for_tests(&self) -> bool {
            self.breath.animations_enabled
        }
    }
}
