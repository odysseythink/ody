#!/usr/bin/env python3
"""Apply the two-line footer changes to ody-tui."""

import re
from pathlib import Path

ROOT = Path("/Users/ranwei/workspace/rust_work/ody-rs")


def read(path: Path) -> str:
    return path.read_text()


def write(path: Path, content: str) -> None:
    path.write_text(content)


def edit_footer_rs(content: str) -> str:
    # Imports
    content = content.replace(
        "use crate::key_hint;\nuse crate::key_hint::KeyBinding;\nuse crate::render::line_utils::prefix_lines;",
        "use crate::key_hint;\nuse crate::key_hint::KeyBinding;\nuse crate::line_truncation::truncate_line_with_ellipsis_if_overflow;\nuse crate::render::line_utils::prefix_lines;",
    )
    content = content.replace(
        "use ratatui::buffer::Buffer;\nuse ratatui::layout::Rect;",
        "use ratatui::buffer::Buffer;\nuse ratatui::layout::Constraint;\nuse ratatui::layout::Layout;\nuse ratatui::layout::Rect;",
    )

    # footer_height
    old = """pub(crate) fn footer_height(props: &FooterProps) -> u16 {
    let show_shortcuts_hint = match props.mode {
        FooterMode::ComposerEmpty => true,
        FooterMode::ComposerHasDraft => false,
        FooterMode::HistorySearch
        | FooterMode::QuitShortcutReminder
        | FooterMode::ShortcutOverlay
        | FooterMode::EscHint => false,
    };
    let show_queue_hint = match props.mode {
        FooterMode::ComposerHasDraft => props.is_task_running,
        FooterMode::QuitShortcutReminder
        | FooterMode::HistorySearch
        | FooterMode::ComposerEmpty
        | FooterMode::ShortcutOverlay
        | FooterMode::EscHint => false,
    };
    footer_from_props_lines(
        props,
        /*collaboration_mode_indicator*/ None,
        /*show_cycle_hint*/ false,
        show_shortcuts_hint,
        show_queue_hint,
    )
    .len() as u16
}"""
    new = """pub(crate) fn footer_height(props: &FooterProps) -> u16 {
    let is_base_mode = matches!(
        props.mode,
        FooterMode::ComposerEmpty | FooterMode::ComposerHasDraft
    );
    let show_shortcuts_hint = match props.mode {
        FooterMode::ComposerEmpty => true,
        FooterMode::ComposerHasDraft => false,
        FooterMode::HistorySearch
        | FooterMode::QuitShortcutReminder
        | FooterMode::ShortcutOverlay
        | FooterMode::EscHint => false,
    };
    let show_queue_hint = match props.mode {
        FooterMode::ComposerHasDraft => props.is_task_running,
        FooterMode::QuitShortcutReminder
        | FooterMode::HistorySearch
        | FooterMode::ComposerEmpty
        | FooterMode::ShortcutOverlay
        | FooterMode::EscHint => false,
    };
    let lines = footer_from_props_lines(
        props,
        /*collaboration_mode_indicator*/ None,
        /*show_cycle_hint*/ false,
        show_shortcuts_hint,
        show_queue_hint,
    )
    .len() as u16;
    if is_base_mode {
        lines.max(2)
    } else {
        lines
    }
}"""
    content = content.replace(old, new)

    # format_context helpers and render_second_footer_line
    old = """pub(crate) fn context_window_line(percent: Option<i64>, used_tokens: Option<i64>) -> Line<'static> {
    if let Some(percent) = percent {
        let percent = percent.clamp(0, 100);
        return Line::from(vec![Span::from(format!("{percent}% context left")).dim()]);
    }

    if let Some(tokens) = used_tokens {
        let used_fmt = format_tokens_compact(tokens);
        return Line::from(vec![Span::from(format!("{used_fmt} used")).dim()]);
    }

    Line::from(vec![Span::from("100% context left").dim()])
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ShortcutId {"""
    new = """pub(crate) fn context_window_line(percent: Option<i64>, used_tokens: Option<i64>) -> Line<'static> {
    if let Some(percent) = percent {
        let percent = percent.clamp(0, 100);
        return Line::from(vec![Span::from(format!("{percent}% context left")).dim()]);
    }

    if let Some(tokens) = used_tokens {
        let used_fmt = format_tokens_compact(tokens);
        return Line::from(vec![Span::from(format!("{used_fmt} used")).dim()]);
    }

    Line::from(vec![Span::from("100% context left").dim()])
}

/// Format a raw token count for the second footer line.
///
/// Mirrors `crate::status::format_tokens_compact` but returns `"--"` when the value is unknown.
pub(crate) fn format_context_token_count(tokens: Option<i64>) -> String {
    tokens.map(format_tokens_compact).unwrap_or_else(|| "--".to_string())
}

/// Format the raw context usage as `context: XX.X% (used/max)`.
///
/// Uses the raw token ratio (not the `BASELINE_TOKENS` adjustment used by the first-row context
/// indicator). Returns `context: --` when the maximum context window is unknown or zero.
pub(crate) fn format_context_usage(context_tokens: Option<i64>, max_context_tokens: Option<i64>) -> String {
    let max = match max_context_tokens {
        Some(max) if max > 0 => max,
        _ => return "context: --".to_string(),
    };
    let used = context_tokens.unwrap_or(0).max(0);
    let percent = (used as f64 / max as f64 * 100.0).clamp(0.0, 100.0);
    format!(
        "context: {:.1}% ({}/{})",
        percent,
        format_context_token_count(Some(used)),
        format_context_token_count(Some(max)),
    )
}

/// Render the second footer line: model name on the left and raw context usage on the right.
///
/// The line is indented on both sides to match the first footer line. If the model name would
/// collide with the context usage, the model name is truncated with an ellipsis.
pub(crate) fn render_second_footer_line(
    area: Rect,
    buf: &mut Buffer,
    model_name: &str,
    context_tokens: Option<i64>,
    max_context_tokens: Option<i64>,
) {
    if area.is_empty() {
        return;
    }

    let right_text = format_context_usage(context_tokens, max_context_tokens);
    let right_line = Line::from(vec![Span::from(right_text).dim()]);
    let right_width = right_line.width() as u16;

    let left_line = if let Some(max_left) = max_left_width_for_right(area, right_width) {
        truncate_line_with_ellipsis_if_overflow(Line::from(model_name.to_string()), max_left as usize)
    } else {
        Line::from(model_name.to_string())
    };

    render_footer_line(area, buf, left_line);
    render_context_right(area, buf, &right_line);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ShortcutId {"""
    content = content.replace(old, new)

    # draw_footer_frame test helper
    old = """            .draw(|f| {
                let area = Rect::new(0, 0, f.area().width, height);
                let show_cycle_hint = !props.is_task_running;"""
    new = """            .draw(|f| {
                let total_area = Rect::new(0, 0, f.area().width, height);
                let (first_line_area, second_line_area) = if total_area.height >= 2
                    && matches!(
                        props.mode,
                        FooterMode::ComposerEmpty | FooterMode::ComposerHasDraft
                    ) {
                    let [first, second] = Layout::vertical([
                        Constraint::Length(1),
                        Constraint::Length(1),
                    ])
                    .areas(total_area);
                    (first, Some(second))
                } else {
                    (total_area, None)
                };
                {
                    let area = first_line_area;
                    let show_cycle_hint = !props.is_task_running;"""
    content = content.replace(old, new)

    old = """                    if show_context && let Some(line) = &right_line {
                        render_context_right(area, f.buffer_mut(), line);
                    }
                }
            })
            .unwrap();"""
    new = """                    if show_context && let Some(line) = &right_line {
                        render_context_right(area, f.buffer_mut(), line);
                    }
                }
                if let Some(second_line_area) = second_line_area {
                    render_second_footer_line(
                        second_line_area,
                        f.buffer_mut(),
                        "",
                        None,
                        None,
                    );
                }
            })
            .unwrap();"""
    content = content.replace(old, new)

    # Unit tests for format_context helpers
    old = """    #[test]
    fn footer_snapshots() {"""
    new = """    #[test]
    fn format_context_usage_handles_missing_max() {
        assert_eq!(format_context_usage(Some(100), None), "context: --");
        assert_eq!(format_context_usage(Some(100), Some(0)), "context: --");
        assert_eq!(format_context_usage(None, None), "context: --");
    }

    #[test]
    fn format_context_usage_computes_raw_percent_and_token_counts() {
        assert_eq!(
            format_context_usage(Some(500), Some(2000)),
            "context: 25.0% (500/2.00K)"
        );
        assert_eq!(
            format_context_usage(Some(1_500_000), Some(2_000_000)),
            "context: 75.0% (1.50M/2.00M)"
        );
    }

    #[test]
    fn format_context_usage_clamps_negative_and_overflow() {
        assert_eq!(format_context_usage(Some(-100), Some(1000)), "context: 0.0% (0/1.00K)");
        assert_eq!(format_context_usage(Some(2000), Some(1000)), "context: 100.0% (2.00K/1.00K)");
    }

    #[test]
    fn format_context_token_count_fallback() {
        assert_eq!(format_context_token_count(None), "--");
        assert_eq!(format_context_token_count(Some(1234)), "1.23K");
    }

    #[test]
    fn render_second_footer_line_shows_model_and_context_usage() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 60, 1));
        render_second_footer_line(
            Rect::new(0, 0, 60, 1),
            &mut buf,
            "ody-model",
            Some(500),
            Some(2000),
        );
        let row: String = (0..60)
            .map(|x| buf[(x, 0)].symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(row.contains("ody-model"), "row: {row}");
        assert!(row.contains("context: 25.0% (500/2.00K)"), "row: {row}");
    }

    #[test]
    fn footer_snapshots() {"""
    content = content.replace(old, new)

    return content


def edit_footer_state_rs(content: str) -> str:
    old = """    pub(super) context_window_percent: Option<i64>,
    pub(super) context_window_used_tokens: Option<i64>,"""
    new = """    pub(super) context_window_percent: Option<i64>,
    pub(super) context_window_used_tokens: Option<i64>,
    pub(super) model_name: String,
    pub(super) context_tokens: Option<i64>,
    pub(super) max_context_tokens: Option<i64>,"""
    content = content.replace(old, new)
    return content


def edit_chat_composer_rs(content: str) -> str:
    # Add import for render_second_footer_line and TokenUsage
    content = content.replace(
        "use super::footer::render_context_right;\nuse super::footer::render_footer_from_props;",
        "use super::footer::render_context_right;\nuse super::footer::render_footer_from_props;\nuse super::footer::render_second_footer_line;",
    )
    content = content.replace(
        "use crate::bottom_pane::paste_burst::FlushResult;",
        "use crate::bottom_pane::paste_burst::FlushResult;\nuse crate::token_usage::TokenUsage;",
    )

    # set_context_window: use regex to handle varying parameter names
    content = re.sub(
        r"pub\(crate\) fn set_context_window\(&mut self, [^)]*\) \{[\s\S]*?\n    \}",
        """pub(crate) fn set_context_window(
        &mut self,
        context_tokens: Option<i64>,
        max_context_tokens: Option<i64>,
    ) {
        if self.footer.context_tokens == context_tokens
            && self.footer.max_context_tokens == max_context_tokens
        {
            return;
        }
        self.footer.context_tokens = context_tokens;
        self.footer.max_context_tokens = max_context_tokens;
        let (percent, used_tokens) =
            TokenUsage::context_window_percent_and_used_tokens(context_tokens, max_context_tokens);
        self.footer.context_window_percent = percent;
        self.footer.context_window_used_tokens = used_tokens;
    }

    pub(crate) fn set_model_name(&mut self, model_name: String) {
        self.footer.model_name = model_name;
    }""",
        content,
        count=1,
    )

    # render block: replace all hint_rect with first_line_rect
    content = content.replace("hint_rect", "first_line_rect")

    # Add second line rect split and second line rendering
    old = """                let first_line_rect = if footer_spacing > 0 && footer_hint_height > 0 {
                    let [_, first_line_rect] = Layout::vertical([
                        Constraint::Length(footer_spacing),
                        Constraint::Length(footer_hint_height),
                    ])
                    .areas(popup_rect);
                    first_line_rect
                } else {
                    popup_rect
                };
                if let Some(line) = self.history_search_footer_line() {"""
    new = """                let first_line_rect = if footer_spacing > 0 && footer_hint_height > 0 {
                    let [_, first_line_rect] = Layout::vertical([
                        Constraint::Length(footer_spacing),
                        Constraint::Length(footer_hint_height),
                    ])
                    .areas(popup_rect);
                    first_line_rect
                } else {
                    popup_rect
                };
                let (first_line_rect, second_line_rect) = if footer_hint_height >= 2 {
                    let [first, second] = Layout::vertical([
                        Constraint::Length(1),
                        Constraint::Length(1),
                    ])
                    .areas(first_line_rect);
                    (first, Some(second))
                } else {
                    (first_line_rect, None)
                };
                if let Some(line) = self.history_search_footer_line() {"""
    content = content.replace(old, new)

    old = """                    if status_line_active
                        && let Some(url) = self.footer.status_line_hyperlink_url.as_deref()
                    {
                        mark_underlined_hyperlink(buf, first_line_rect, url);
                    }
                }
            }
        }"""
    new = """                    if status_line_active
                        && let Some(url) = self.footer.status_line_hyperlink_url.as_deref()
                    {
                        mark_underlined_hyperlink(buf, first_line_rect, url);
                    }
                }
                if let Some(second_line_rect) = second_line_rect {
                    render_second_footer_line(
                        second_line_rect,
                        buf,
                        &self.footer.model_name,
                        self.footer.context_tokens,
                        self.footer.max_context_tokens,
                    );
                }
            }
        }"""
    content = content.replace(old, new)

    # Update footer_collapse_snapshots test helper
    old = """            composer.set_collaboration_modes_enabled(/*enabled*/ true);
            composer.set_collaboration_mode_indicator(indicator);
            composer.set_context_window(Some(context_percent), /*used_tokens*/ None);"""
    new = """            composer.set_collaboration_modes_enabled(/*enabled*/ true);
            composer.set_collaboration_mode_indicator(indicator);
            const MAX_CONTEXT_TOKENS: i64 = 1_000_000;
            let raw_used = (context_percent * MAX_CONTEXT_TOKENS / 100)
                .saturating_sub(TokenUsage::BASELINE_TOKENS);
            composer.set_context_window(Some(raw_used), Some(MAX_CONTEXT_TOKENS));"""
    content = content.replace(old, new)

    return content


def edit_bottom_pane_mod_rs(content: str) -> str:
    # Add TokenUsage import
    if "use crate::token_usage::TokenUsage;" not in content:
        content = content.replace(
            "use crate::bottom_pane::chat_composer::InputResult;",
            "use crate::bottom_pane::chat_composer::InputResult;\nuse crate::token_usage::TokenUsage;",
        )

    old = """    pub(crate) fn set_context_window(&mut self, percent: Option<i64>, used_tokens: Option<i64>) {
        if self.context_window_percent == percent && self.context_window_used_tokens == used_tokens
        {
            return;
        }

        self.context_window_percent = percent;
        self.context_window_used_tokens = used_tokens;
        self.composer
            .set_context_window(percent, self.context_window_used_tokens);
        self.request_redraw();
    }"""
    new = """    pub(crate) fn set_context_window(&mut self, context_tokens: Option<i64>, max_context_tokens: Option<i64>) {
        if self.context_tokens == context_tokens && self.max_context_tokens == max_context_tokens {
            return;
        }

        self.context_tokens = context_tokens;
        self.max_context_tokens = max_context_tokens;
        let (percent, used_tokens) =
            TokenUsage::context_window_percent_and_used_tokens(context_tokens, max_context_tokens);
        self.context_window_percent = percent;
        self.context_window_used_tokens = used_tokens;
        self.composer
            .set_context_window(context_tokens, max_context_tokens);
        self.request_redraw();
    }

    pub(crate) fn set_model_name(&mut self, model_name: String) {
        self.composer.set_model_name(model_name);
    }"""
    content = content.replace(old, new)
    return content


def edit_chatwidget_rs(content: str) -> str:
    # Replace apply_token_info
    old = """    fn apply_token_info(&mut self, info: TokenUsageInfo) {
        let percent = self.context_remaining_percent(&info);
        let used_tokens = self.context_used_tokens(&info, percent.is_some());
        self.bottom_pane.set_context_window(percent, used_tokens);
        self.token_info = Some(info);
    }

    fn context_remaining_percent(&self, info: &TokenUsageInfo) -> Option<i64> {
        info.model_context_window.map(|window| {
            info.last_token_usage
                .percent_of_context_window_remaining(window)
        })
    }

    fn context_used_tokens(&self, info: &TokenUsageInfo, percent_known: bool) -> Option<i64> {
        if percent_known {
            return None;
        }

        Some(info.total_token_usage.tokens_in_context_window())
    }"""
    new = """    fn apply_token_info(&mut self, info: TokenUsageInfo) {
        let context_tokens = Some(info.total_token_usage.tokens_in_context_window());
        let max_context_tokens = info.model_context_window;
        self.bottom_pane.set_context_window(context_tokens, max_context_tokens);
        self.bottom_pane
            .set_model_name(self.model_display_name().to_string());
        self.token_info = Some(info);
    }"""
    content = content.replace(old, new)

    # Replace None case
    content = content.replace(
        "self.bottom_pane\n                    .set_context_window(/*percent*/ None, /*used_tokens*/ None);",
        "self.bottom_pane\n                    .set_context_window(None, None);",
    )
    return content


def edit_turn_runtime_rs(content: str) -> str:
    # No changes needed in current tree
    return content


def edit_token_usage_rs(content: str) -> str:
    # Add helper method. Insert after `pub const BASELINE_TOKENS` if present, else after is_zero.
    if "pub const BASELINE_TOKENS" not in content:
        content = content.replace(
            "    pub fn is_zero(&self) -> bool {",
            "    pub const BASELINE_TOKENS: i64 = BASELINE_TOKENS;\n\n    pub fn is_zero(&self) -> bool {",
        )
    marker = """    pub const BASELINE_TOKENS: i64 = BASELINE_TOKENS;

    pub fn is_zero(&self) -> bool {"""
    new_marker = """    pub const BASELINE_TOKENS: i64 = BASELINE_TOKENS;

    /// Derive the first-row context-window percent and used-token count from raw usage.
    ///
    /// Applies the same `BASELINE_TOKENS` adjustment that the rest of the TUI uses so the
    /// first footer row matches existing behavior.
    pub(crate) fn context_window_percent_and_used_tokens(
        context_tokens: Option<i64>,
        max_context_tokens: Option<i64>,
    ) -> (Option<i64>, Option<i64>) {
        let max = match max_context_tokens {
            Some(max) if max > 0 => max,
            _ => return (None, None),
        };
        let used = context_tokens.unwrap_or(0).max(0);
        let effective_used = (used + Self::BASELINE_TOKENS).min(max);
        let percent = Some(((effective_used * 100) / max).clamp(0, 100));
        (percent, Some(effective_used))
    }

    pub fn is_zero(&self) -> bool {"""
    content = content.replace(marker, new_marker)
    return content


def main() -> None:
    files = {
        "tui/src/bottom_pane/footer.rs": edit_footer_rs,
        "tui/src/bottom_pane/chat_composer/footer_state.rs": edit_footer_state_rs,
        "tui/src/bottom_pane/chat_composer.rs": edit_chat_composer_rs,
        "tui/src/bottom_pane/mod.rs": edit_bottom_pane_mod_rs,
        "tui/src/chatwidget.rs": edit_chatwidget_rs,
        "tui/src/chatwidget/turn_runtime.rs": edit_turn_runtime_rs,
        "tui/src/token_usage.rs": edit_token_usage_rs,
    }
    for rel, editor in files.items():
        path = ROOT / rel
        original = read(path)
        updated = editor(original)
        if updated != original:
            write(path, updated)
            print(f"updated {rel}")
        else:
            print(f"no changes for {rel}")


if __name__ == "__main__":
    main()
