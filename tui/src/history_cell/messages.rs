//! User, assistant, reasoning, and streaming message history cells.

use super::*;

#[derive(Debug)]
pub(crate) struct UserHistoryCell {
    pub message: String,
    pub text_elements: Vec<TextElement>,
    #[allow(dead_code)]
    pub local_image_paths: Vec<PathBuf>,
    pub remote_image_urls: Vec<String>,
}

/// Build logical lines for a user message with styled text elements.
///
/// This preserves explicit newlines while interleaving element spans and skips
/// malformed byte ranges instead of panicking during history rendering.
fn build_user_message_lines_with_elements(
    message: &str,
    elements: &[TextElement],
    style: Style,
    element_style: Style,
) -> Vec<Line<'static>> {
    let mut elements = elements.to_vec();
    elements.sort_by_key(|e| e.byte_range.start);
    let mut offset = 0usize;
    let mut raw_lines: Vec<Line<'static>> = Vec::new();
    for line_text in message.split('\n') {
        let line_start = offset;
        let line_end = line_start + line_text.len();
        let mut spans: Vec<Span<'static>> = Vec::new();
        // Track how much of the line we've emitted to interleave plain and styled spans.
        let mut cursor = line_start;
        for elem in &elements {
            let start = elem.byte_range.start.max(line_start);
            let end = elem.byte_range.end.min(line_end);
            if start >= end {
                continue;
            }
            let rel_start = start - line_start;
            let rel_end = end - line_start;
            // Guard against malformed UTF-8 byte ranges from upstream data; skip
            // invalid elements rather than panicking while rendering history.
            if !line_text.is_char_boundary(rel_start) || !line_text.is_char_boundary(rel_end) {
                continue;
            }
            let rel_cursor = cursor - line_start;
            if cursor < start
                && line_text.is_char_boundary(rel_cursor)
                && let Some(segment) = line_text.get(rel_cursor..rel_start)
            {
                spans.push(Span::from(segment.to_string()));
            }
            if let Some(segment) = line_text.get(rel_start..rel_end) {
                spans.push(Span::styled(segment.to_string(), element_style));
                cursor = end;
            }
        }
        let rel_cursor = cursor - line_start;
        if cursor < line_end
            && line_text.is_char_boundary(rel_cursor)
            && let Some(segment) = line_text.get(rel_cursor..)
        {
            spans.push(Span::from(segment.to_string()));
        }
        let line = if spans.is_empty() {
            Line::from(line_text.to_string()).style(style)
        } else {
            Line::from(spans).style(style)
        };
        raw_lines.push(line);
        // Split on '\n' so any '\r' stays in the line; advancing by 1 accounts
        // for the separator byte.
        offset = line_end + 1;
    }

    raw_lines
}

fn remote_image_display_line(style: Style, index: usize) -> Line<'static> {
    Line::from(local_image_label_text(index)).style(style)
}

fn trim_trailing_blank_lines(mut lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
    while lines
        .last()
        .is_some_and(|line| line.spans.iter().all(|span| span.content.trim().is_empty()))
    {
        lines.pop();
    }
    lines
}

impl HistoryCell for UserHistoryCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        let wrap_width = width
            .saturating_sub(
                LIVE_PREFIX_COLS + 1, /* keep a one-column right margin for wrapping */
            )
            .max(1);

        let style = user_message_style();
        let element_style = style.fg(Color::Cyan);

        let wrapped_remote_images = if self.remote_image_urls.is_empty() {
            None
        } else {
            Some(adaptive_wrap_lines(
                self.remote_image_urls
                    .iter()
                    .enumerate()
                    .map(|(idx, _url)| {
                        remote_image_display_line(element_style, idx.saturating_add(1))
                    }),
                RtOptions::new(usize::from(wrap_width))
                    .wrap_algorithm(textwrap::WrapAlgorithm::FirstFit),
            ))
        };

        let wrapped_message = if self.message.is_empty() && self.text_elements.is_empty() {
            None
        } else if self.text_elements.is_empty() {
            let message_without_trailing_newlines = self.message.trim_end_matches(['\r', '\n']);
            let wrapped = adaptive_wrap_lines(
                message_without_trailing_newlines
                    .split('\n')
                    .map(|line| Line::from(line).style(style)),
                // Wrap algorithm matches textarea.rs.
                RtOptions::new(usize::from(wrap_width))
                    .wrap_algorithm(textwrap::WrapAlgorithm::FirstFit),
            );
            let wrapped = trim_trailing_blank_lines(wrapped);
            (!wrapped.is_empty()).then_some(wrapped)
        } else {
            let raw_lines = build_user_message_lines_with_elements(
                &self.message,
                &self.text_elements,
                style,
                element_style,
            );
            let wrapped = adaptive_wrap_lines(
                raw_lines,
                RtOptions::new(usize::from(wrap_width))
                    .wrap_algorithm(textwrap::WrapAlgorithm::FirstFit),
            );
            let wrapped = trim_trailing_blank_lines(wrapped);
            (!wrapped.is_empty()).then_some(wrapped)
        };

        if wrapped_remote_images.is_none() && wrapped_message.is_none() {
            return Vec::new();
        }

        let mut lines: Vec<Line<'static>> = vec![Line::from("").style(style)];

        if let Some(wrapped_remote_images) = wrapped_remote_images {
            lines.extend(prefix_lines(
                wrapped_remote_images,
                "  ".into(),
                "  ".into(),
            ));
            if wrapped_message.is_some() {
                lines.push(Line::from("").style(style));
            }
        }

        if let Some(wrapped_message) = wrapped_message {
            lines.extend(prefix_lines(
                wrapped_message,
                "› ".bold().dim(),
                "  ".into(),
            ));
        }

        lines.push(Line::from("").style(style));
        lines
    }

    fn raw_lines(&self) -> Vec<Line<'static>> {
        let mut lines = raw_lines_from_source(self.message.trim_end_matches(['\r', '\n']));
        if !self.remote_image_urls.is_empty() {
            if !lines.is_empty() {
                lines.push(Line::from(""));
            }
            lines.extend(
                self.remote_image_urls
                    .iter()
                    .enumerate()
                    .map(|(idx, _url)| Line::from(local_image_label_text(idx.saturating_add(1)))),
            );
        }
        lines
    }
}

/// Number of wrapped lines to show before collapsing a reasoning summary.
const REASONING_PREVIEW_LINES: usize = 2;

#[derive(Debug)]
pub(crate) struct ReasoningSummaryCell {
    _header: String,
    content: String,
    /// Session cwd used to render local file links inside the reasoning body.
    cwd: PathBuf,
    transcript_only: bool,
    /// Whether the full reasoning text is expanded in the transcript.
    expanded: std::sync::atomic::AtomicBool,
}

impl ReasoningSummaryCell {
    /// Create a reasoning summary cell that will render local file links relative to the session
    /// cwd active when the summary was recorded.
    pub(crate) fn new(header: String, content: String, cwd: &Path, transcript_only: bool) -> Self {
        Self {
            _header: header,
            content,
            cwd: cwd.to_path_buf(),
            transcript_only,
            expanded: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Toggle the expanded/collapsed state of this reasoning block.
    ///
    /// Returns the new state after toggling.
    pub(crate) fn toggle_expanded(&self) -> bool {
        let current = self.expanded.load(std::sync::atomic::Ordering::Relaxed);
        let new = !current;
        self.expanded
            .store(new, std::sync::atomic::Ordering::Relaxed);
        new
    }

    #[cfg(test)]
    pub(crate) fn set_expanded(&self, expanded: bool) {
        self.expanded
            .store(expanded, std::sync::atomic::Ordering::Relaxed);
    }

    fn lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        append_markdown(
            &self.content,
            crate::width::usable_content_width_u16(width, /*reserved_cols*/ 2),
            Some(self.cwd.as_path()),
            &mut lines,
        );
        let summary_style = Style::default().dim().italic();
        let summary_lines = lines
            .into_iter()
            .map(|mut line| {
                line.spans = line
                    .spans
                    .into_iter()
                    .map(|span| span.patch_style(summary_style))
                    .collect();
                line
            })
            .collect::<Vec<_>>();

        let wrapped = adaptive_wrap_lines(
            &summary_lines,
            RtOptions::new(width as usize)
                .initial_indent("• ".dim().into())
                .subsequent_indent("  ".into()),
        );
        self.apply_collapse_hint(wrapped, width)
    }

    /// Truncate a long wrapped reasoning block and append a hint line.
    fn apply_collapse_hint(&self, wrapped: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
        if self.expanded.load(std::sync::atomic::Ordering::Relaxed)
            || wrapped.len() <= REASONING_PREVIEW_LINES
        {
            return wrapped;
        }

        let remaining = wrapped.len() - REASONING_PREVIEW_LINES;
        let mut truncated: Vec<Line<'static>> =
            wrapped.into_iter().take(REASONING_PREVIEW_LINES).collect();
        let more_lines = if remaining == 1 { "line" } else { "lines" };
        let hint = Line::from(format!(
            "... ({remaining} more {more_lines}, alt+o to expand)"
        ))
        .dim();
        let wrapped_hint = adaptive_wrap_lines(
            std::slice::from_ref(&hint),
            RtOptions::new(width as usize)
                .initial_indent("  ".into())
                .subsequent_indent("  ".into()),
        );
        truncated.extend(wrapped_hint);
        truncated
    }
}

impl HistoryCell for ReasoningSummaryCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        if self.transcript_only {
            Vec::new()
        } else {
            self.lines(width)
        }
    }

    fn transcript_lines(&self, width: u16) -> Vec<Line<'static>> {
        self.lines(width)
    }

    fn raw_lines(&self) -> Vec<Line<'static>> {
        if self.transcript_only {
            Vec::new()
        } else {
            raw_lines_from_source(self.content.trim())
        }
    }

    fn is_collapsible(&self) -> bool {
        self.content.lines().count() > REASONING_PREVIEW_LINES
    }

    fn is_expanded(&self) -> bool {
        self.expanded.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn toggle_expanded(&self) -> bool {
        let current = self.expanded.load(std::sync::atomic::Ordering::Relaxed);
        let new = !current;
        self.expanded.store(new, std::sync::atomic::Ordering::Relaxed);
        new
    }
}

#[derive(Debug)]
pub(crate) struct AgentMessageCell {
    lines: Vec<HyperlinkLine>,
    is_first_line: bool,
}

impl AgentMessageCell {
    #[cfg(test)]
    pub(crate) fn new(lines: Vec<Line<'static>>, is_first_line: bool) -> Self {
        Self {
            lines: plain_hyperlink_lines(lines),
            is_first_line,
        }
    }

    pub(crate) fn new_hyperlink_lines(lines: Vec<HyperlinkLine>, is_first_line: bool) -> Self {
        Self {
            lines,
            is_first_line,
        }
    }
}

impl HistoryCell for AgentMessageCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        visible_lines(self.display_hyperlink_lines(width))
    }

    fn display_hyperlink_lines(&self, width: u16) -> Vec<HyperlinkLine> {
        let mut wrapped = Vec::new();
        for (index, line) in self.lines.iter().enumerate() {
            let initial_indent = if index == 0 && self.is_first_line {
                "• ".dim().into()
            } else {
                "  ".into()
            };
            let mut subsequent_indent = Line::from("  ");
            subsequent_indent
                .spans
                .extend(crate::insert_history::leading_whitespace_prefix(&line.line).spans);
            wrapped.extend(crate::terminal_hyperlinks::adaptive_wrap_hyperlink_lines(
                std::slice::from_ref(line),
                RtOptions::new(width as usize)
                    .initial_indent(initial_indent)
                    .subsequent_indent(subsequent_indent),
            ));
        }
        wrapped
    }

    fn transcript_hyperlink_lines(&self, width: u16) -> Vec<HyperlinkLine> {
        self.display_hyperlink_lines(width)
    }

    fn raw_lines(&self) -> Vec<Line<'static>> {
        plain_lines(visible_lines(self.lines.clone()))
    }

    fn is_stream_continuation(&self) -> bool {
        !self.is_first_line
    }
}

/// A consolidated agent message cell that stores raw markdown source and re-renders from it.
///
/// After a stream finalizes, the `ConsolidateAgentMessage` handler in `App`
/// replaces the contiguous run of `AgentMessageCell`s with a single
/// `AgentMarkdownCell`. On terminal resize, `display_lines(width)` re-renders
/// from source via `append_markdown_agent`, producing correctly-sized tables
/// with box-drawing borders.
///
/// The cell snapshots `cwd` at construction so local file-link display remains aligned with the
/// session that produced the message. Reusing the current process cwd during reflow would make old
/// transcript content change meaning after a later `/cd` or resumed session.
#[derive(Debug)]
pub(crate) struct AgentMarkdownCell {
    markdown_source: String,
    cwd: PathBuf,
    expanded: std::sync::atomic::AtomicBool,
}

impl AgentMarkdownCell {
    /// Create a finalized source-backed assistant message cell.
    ///
    /// `markdown_source` must be the raw source accumulated by the stream controller, not already
    /// wrapped terminal lines. Passing rendered lines here would make future resize reflow preserve
    /// stale wrapping instead of repairing it.
    pub(crate) fn new(markdown_source: String, cwd: &Path) -> Self {
        Self {
            markdown_source,
            cwd: cwd.to_path_buf(),
            expanded: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn full_display_hyperlink_lines(&self, width: u16) -> Vec<HyperlinkLine> {
        let Some(wrap_width) =
            crate::width::usable_content_width_u16(width, /*reserved_cols*/ 2)
        else {
            return prefix_hyperlink_lines(
                vec![HyperlinkLine::new(Line::default())],
                "• ".dim(),
                "  ".into(),
            );
        };

        // Re-render markdown from source at the current width. Reserve 2 columns for the "• " /
        // " " prefix prepended below.
        let lines = crate::markdown::render_markdown_agent_with_links_and_cwd(
            &self.markdown_source,
            Some(wrap_width),
            Some(self.cwd.as_path()),
        );
        prefix_hyperlink_lines(lines, "• ".dim(), "  ".into())
    }
}

impl HistoryCell for AgentMarkdownCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        visible_lines(self.display_hyperlink_lines(width))
    }

    fn display_hyperlink_lines(&self, width: u16) -> Vec<HyperlinkLine> {
        let full = self.full_display_hyperlink_lines(width);
        if self.is_expanded() || !self.is_collapsible() {
            return full;
        }
        let visible = visible_lines(full);
        let truncated = truncate_lines_with_hint(
            visible,
            MESSAGE_PREVIEW_LINES,
            false,
            |remaining| collapse_hint(remaining),
        );
        plain_hyperlink_lines(truncated)
    }

    fn transcript_hyperlink_lines(&self, width: u16) -> Vec<HyperlinkLine> {
        self.full_display_hyperlink_lines(width)
    }

    fn raw_lines(&self) -> Vec<Line<'static>> {
        raw_lines_from_source(&self.markdown_source)
    }

    fn is_collapsible(&self) -> bool {
        self.markdown_source.lines().count() > MESSAGE_PREVIEW_LINES
    }

    fn is_expanded(&self) -> bool {
        self.expanded.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn toggle_expanded(&self) -> bool {
        let current = self.expanded.load(std::sync::atomic::Ordering::Relaxed);
        let new = !current;
        self.expanded.store(new, std::sync::atomic::Ordering::Relaxed);
        new
    }
}

/// Transient active-cell representation of the mutable tail of an agent stream.
///
/// During streaming, lines that have not yet been committed to scrollback because they belong to
/// an in-progress table are displayed via this cell in the `active_cell` slot. It is replaced on
/// every delta and cleared when the stream finalizes.
#[derive(Debug)]
pub(crate) struct StreamingAgentTailCell {
    lines: Vec<HyperlinkLine>,
    is_first_line: bool,
}

impl StreamingAgentTailCell {
    pub(crate) fn new(lines: Vec<HyperlinkLine>, is_first_line: bool) -> Self {
        Self {
            lines,
            is_first_line,
        }
    }
}

impl HistoryCell for StreamingAgentTailCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        visible_lines(self.display_hyperlink_lines(width))
    }

    fn display_hyperlink_lines(&self, _width: u16) -> Vec<HyperlinkLine> {
        // Tail lines are already rendered at the controller's current stream width.
        // Re-wrapping them here can split table borders and produce malformed in-flight rows.
        let mut lines = prefix_hyperlink_lines(
            self.lines.clone(),
            if self.is_first_line {
                "• ".dim()
            } else {
                "  ".into()
            },
            "  ".into(),
        );
        for line in &mut lines {
            if line
                .line
                .spans
                .iter()
                .all(|span| span.content.chars().all(char::is_whitespace))
            {
                line.line = Line::default().style(line.line.style);
                line.hyperlinks.clear();
            }
        }
        lines
    }

    fn transcript_hyperlink_lines(&self, width: u16) -> Vec<HyperlinkLine> {
        self.display_hyperlink_lines(width)
    }

    fn raw_lines(&self) -> Vec<Line<'static>> {
        plain_lines(self.display_lines(/*width*/ u16::MAX))
    }

    fn is_stream_continuation(&self) -> bool {
        !self.is_first_line
    }
}

/// Transient active-cell representation of the mutable tail of a reasoning stream.
///
/// During a reasoning block, deltas that are not part of an active agent or plan stream are
/// displayed via this cell in the `active_cell` slot. It is replaced on every delta and dropped
/// when the reasoning block finalizes, because the final `ReasoningSummaryCell` is the canonical
/// history entry.
#[derive(Debug)]
pub(crate) struct StreamingReasoningTailCell {
    lines: Vec<HyperlinkLine>,
}

impl StreamingReasoningTailCell {
    pub(crate) fn new(lines: Vec<HyperlinkLine>) -> Self {
        Self { lines }
    }
}

impl HistoryCell for StreamingReasoningTailCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        visible_lines(self.display_hyperlink_lines(width))
    }

    fn display_hyperlink_lines(&self, _width: u16) -> Vec<HyperlinkLine> {
        // Tail lines are already rendered at the current stream width, including the bullet prefix.
        // Re-wrapping them here could produce malformed in-flight rows.
        let mut lines = self.lines.clone();
        for line in &mut lines {
            if line
                .line
                .spans
                .iter()
                .all(|span| span.content.chars().all(char::is_whitespace))
            {
                line.line = Line::default().style(line.line.style);
                line.hyperlinks.clear();
            }
        }
        lines
    }

    fn transcript_hyperlink_lines(&self, width: u16) -> Vec<HyperlinkLine> {
        self.display_hyperlink_lines(width)
    }

    fn raw_lines(&self) -> Vec<Line<'static>> {
        plain_lines(visible_lines(self.lines.clone()))
    }
}

/// Render a transient reasoning tail from raw reasoning text.
///
/// The tail mirrors the final `ReasoningSummaryCell` styling (dim italic, bullet prefix) so the
/// streaming affordance visually matches the history entry that replaces it.
///
/// During active streaming the tail is capped to the last `REASONING_PREVIEW_LINES` wrapped lines
/// so that a long reasoning block does not push the surrounding conversation out of view. The
/// full buffer is retained in the controller and will be rendered by the finalized
/// `ReasoningSummaryCell`.
pub(crate) fn new_streaming_reasoning_tail_cell(
    reasoning_buffer: &str,
    width: u16,
    cwd: &Path,
) -> Box<dyn HistoryCell> {
    let cwd = cwd.to_path_buf();
    let reasoning_buffer = reasoning_buffer.trim();
    if reasoning_buffer.is_empty() {
        return Box::new(StreamingReasoningTailCell::new(Vec::new()));
    }
    let Some(wrap_width) = crate::width::usable_content_width_u16(width, /*reserved_cols*/ 2)
    else {
        return Box::new(StreamingReasoningTailCell::new(Vec::new()));
    };
    let mut lines: Vec<Line<'static>> = Vec::new();
    append_markdown(
        reasoning_buffer,
        Some(wrap_width),
        Some(cwd.as_path()),
        &mut lines,
    );
    let summary_style = Style::default().dim().italic();
    let styled_lines = lines
        .into_iter()
        .map(|mut line| {
            line.spans = line
                .spans
                .into_iter()
                .map(|span| span.patch_style(summary_style))
                .collect();
            line
        })
        .collect::<Vec<_>>();
    // Wrap without the prefix so we can truncate to the most recent preview lines and then
    // re-apply the bullet prefix on the first surviving line. This mirrors the ody-code
    // thinking component, which keeps only the tail of a live reasoning block visible.
    let wrapped = adaptive_wrap_lines(&styled_lines, RtOptions::new(wrap_width as usize));
    let truncated = if wrapped.len() > REASONING_PREVIEW_LINES {
        let start = wrapped.len() - REASONING_PREVIEW_LINES;
        wrapped.into_iter().skip(start).collect()
    } else {
        wrapped
    };
    let prefixed = prefix_lines(truncated, "• ".dim().into(), "  ".into());
    Box::new(StreamingReasoningTailCell::new(plain_hyperlink_lines(
        prefixed,
    )))
}
pub(crate) fn new_user_prompt(
    message: String,
    text_elements: Vec<TextElement>,
    local_image_paths: Vec<PathBuf>,
    remote_image_urls: Vec<String>,
) -> UserHistoryCell {
    UserHistoryCell {
        message,
        text_elements,
        local_image_paths,
        remote_image_urls,
    }
}
/// Create the reasoning history cell emitted at the end of a reasoning block.
///
/// The helper snapshots `cwd` into the returned cell so local file links render the same way they
/// did while the turn was live, even if rendering happens after other app state has advanced.
pub(crate) fn new_reasoning_summary_block(
    full_reasoning_buffer: String,
    cwd: &Path,
) -> Box<dyn HistoryCell> {
    let cwd = cwd.to_path_buf();
    let full_reasoning_buffer = full_reasoning_buffer.trim();
    if let Some(open) = full_reasoning_buffer.find("**") {
        let after_open = &full_reasoning_buffer[(open + 2)..];
        if let Some(close) = after_open.find("**") {
            let after_close_idx = open + 2 + close + 2;
            // if we don't have anything beyond `after_close_idx`
            // then we don't have a summary to inject into history
            if after_close_idx < full_reasoning_buffer.len() {
                let header_buffer = full_reasoning_buffer[..after_close_idx].to_string();
                let summary_buffer = full_reasoning_buffer[after_close_idx..].to_string();
                // Preserve the session cwd so local file links render the same way in the
                // collapsed reasoning block as they did while streaming live content.
                return Box::new(ReasoningSummaryCell::new(
                    header_buffer,
                    summary_buffer,
                    &cwd,
                    /*transcript_only*/ false,
                ));
            }
        }
    }
    Box::new(ReasoningSummaryCell::new(
        "".to_string(),
        full_reasoning_buffer.to_string(),
        &cwd,
        /*transcript_only*/ false,
    ))
}
