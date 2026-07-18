//! Preview constants and truncation helpers for collapsible history cells.
//!
//! This module is intentionally minimal: it only exposes the preview thresholds
//! and a shared truncation helper. The actual decision of whether a cell is
//! collapsible, and the state management for expanded/collapsed, lives in the
//! concrete cell implementations (see `HistoryCell::is_collapsible` and
//! `HistoryCell::toggle_expanded`).

use ratatui::prelude::*;

/// Number of wrapped logical lines to show before collapsing a long assistant message.
pub(crate) const MESSAGE_PREVIEW_LINES: usize = 5;

/// Number of wrapped logical lines to show before collapsing a proposed plan.
pub(crate) const PLAN_PREVIEW_LINES: usize = 5;

/// Number of wrapped logical lines to show before collapsing an MCP tool result.
pub(crate) const TOOL_RESULT_PREVIEW_LINES: usize = 3;

/// Collapse `lines` to `preview_lines` plus a trailing hint, unless already expanded or short.
///
/// The `make_hint` callback receives the number of remaining lines and should produce a
/// single `Line` (typically dim/italic) telling the user how much content is hidden and how
/// to expand it. Keeping the hint construction a callback lets each cell control its own
/// wording and styling while reusing the truncation logic.
pub(crate) fn truncate_lines_with_hint(
    lines: Vec<Line<'static>>,
    preview_lines: usize,
    expanded: bool,
    make_hint: impl FnOnce(usize) -> Line<'static>,
) -> Vec<Line<'static>> {
    if expanded || lines.len() <= preview_lines {
        return lines;
    }
    let remaining = lines.len() - preview_lines;
    let mut truncated: Vec<Line<'static>> = lines.into_iter().take(preview_lines).collect();
    truncated.push(make_hint(remaining));
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::text::Line;

    fn render_line_text(line: &Line<'static>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }

    fn render_lines(lines: &[Line<'static>]) -> Vec<String> {
        lines.iter().map(render_line_text).collect()
    }

    #[test]
    fn constants_match_roadmap() {
        assert_eq!(MESSAGE_PREVIEW_LINES, 5);
        assert_eq!(PLAN_PREVIEW_LINES, 5);
        assert_eq!(TOOL_RESULT_PREVIEW_LINES, 3);
    }

    #[test]
    fn truncate_short_content_does_not_add_hint() {
        let lines = vec![Line::from("one"), Line::from("two")];
        let out = truncate_lines_with_hint(lines, 5, false, |_remaining| Line::from("hint"));
        assert_eq!(render_lines(&out), vec!["one", "two"]);
    }

    #[test]
    fn truncate_long_content_adds_hint() {
        let lines = (1..=6).map(|i| Line::from(i.to_string())).collect();
        let out = truncate_lines_with_hint(lines, 5, false, |remaining| {
            Line::from(format!("... ({remaining} more lines, alt+o to expand)")).dim()
        });
        let rendered = render_lines(&out);
        assert_eq!(rendered.len(), 6);
        assert_eq!(rendered[..5], vec!["1", "2", "3", "4", "5"]);
        assert!(rendered[5].contains("1 more lines"));
        assert!(rendered[5].contains("alt+o to expand"));
    }

    #[test]
    fn truncate_expanded_keeps_all_lines() {
        let lines = (1..=10).map(|i| Line::from(i.to_string())).collect();
        let out =
            truncate_lines_with_hint(lines, 5, true, |_remaining| Line::from("hint"));
        assert_eq!(render_lines(&out).len(), 10);
    }
}
