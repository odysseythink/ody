//! Overlay UIs rendered in an alternate screen.
//!
//! This module implements the pager-style overlays used by the TUI, including the transcript
//! overlay (`Ctrl+T`) that renders a full history view separate from the main viewport.
//!
//! The transcript overlay renders committed transcript cells plus an optional render-only live tail
//! derived from the current in-flight active cell. Because rebuilding wrapped `Line`s on every draw
//! can be expensive, that live tail is cached and only recomputed when its cache key changes, which
//! is derived from the terminal width (wrapping), an active-cell revision (in-place mutations), the
//! stream-continuation flag (spacing), and an animation tick (time-based spinner/shimmer output).
//!
//! The transcript overlay live tail is kept in sync by `App` during draws: `App` supplies an
//! `ActiveCellTranscriptKey` and a function to compute the active cell transcript lines, and
//! `TranscriptOverlay::sync_live_tail` uses the key to decide when the cached tail must be
//! recomputed. `ChatWidget` is responsible for producing a key that changes when the active cell
//! mutates in place or when its transcript output is time-dependent.

use std::collections::HashMap;
use std::io::Result;
use std::sync::Arc;

use crate::chatwidget::ActiveCellTranscriptKey;
use crate::copy_targets;
use crate::history_cell::HistoryCell;
use crate::history_cell::HistoryRenderMode;
use crate::history_cell::UserHistoryCell;
use crate::key_hint;
use crate::key_hint::KeyBinding;
use crate::key_hint::KeyBindingListExt;
use crate::keymap::PagerKeymap;
use crate::render::Insets;
use crate::render::renderable::InsetRenderable;
use crate::render::renderable::Renderable;
use crate::style::user_message_style;
use crate::terminal_hyperlinks::HyperlinkLine;
use crate::terminal_hyperlinks::mark_buffer_hyperlinks;
use crate::terminal_hyperlinks::strip_osc8;
use crate::terminal_hyperlinks::visible_lines;
use crate::tui;
use crate::tui::TuiEvent;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;
use crossterm::event::MouseButton;
use crossterm::event::MouseEvent;
use crossterm::event::MouseEventKind;
use ratatui::buffer::Buffer;
use ratatui::buffer::Cell;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::text::Text;
use ratatui::widgets::Clear;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use ratatui::widgets::WidgetRef;
use ratatui::widgets::Wrap;

pub(crate) enum Overlay {
    Transcript(TranscriptOverlay),
    Static(StaticOverlay),
}

impl Overlay {
    pub(crate) fn new_transcript(
        cells: Vec<Arc<dyn HistoryCell>>,
        keymap: PagerKeymap,
        mouse_scroll_speed: f64,
    ) -> Self {
        Self::Transcript(TranscriptOverlay::new(cells, keymap, mouse_scroll_speed))
    }

    pub(crate) fn new_static_with_lines(
        lines: Vec<Line<'static>>,
        title: String,
        keymap: PagerKeymap,
    ) -> Self {
        Self::Static(StaticOverlay::with_title(lines, title, keymap))
    }

    pub(crate) fn new_static_with_renderables(
        renderables: Vec<Box<dyn Renderable>>,
        title: String,
        keymap: PagerKeymap,
    ) -> Self {
        Self::Static(StaticOverlay::with_renderables(renderables, title, keymap))
    }

    pub(crate) fn new_static_with_cell(
        cell: Arc<dyn HistoryCell>,
        title: String,
        keymap: PagerKeymap,
    ) -> Self {
        Self::Static(StaticOverlay::with_renderables(
            vec![Box::new(CachedRenderable::new(CellRenderable {
                cell,
                style: Style::default(),
                mode: CellRenderableMode::Display,
            }))],
            title,
            keymap,
        ))
    }

    pub(crate) fn handle_event(&mut self, tui: &mut tui::Tui, event: TuiEvent) -> Result<()> {
        match self {
            Overlay::Transcript(o) => o.handle_event(tui, event),
            Overlay::Static(o) => o.handle_event(tui, event),
        }
    }

    pub(crate) fn is_done(&self) -> bool {
        match self {
            Overlay::Transcript(o) => o.is_done(),
            Overlay::Static(o) => o.is_done(),
        }
    }

    /// Whether the overlay wants to consume Esc itself (e.g. to clear an active
    /// text selection or exit copy mode) instead of letting `App` start the
    /// backtrack preview.
    pub(crate) fn wants_esc(&self) -> bool {
        match self {
            Overlay::Transcript(o) => o.wants_esc(),
            Overlay::Static(o) => o.wants_esc(),
        }
    }

    pub(crate) fn take_pending_copy(&mut self) -> Option<String> {
        match self {
            Overlay::Transcript(o) => o.view.take_pending_copy(),
            Overlay::Static(o) => o.view.take_pending_copy(),
        }
    }

    pub(crate) fn take_pending_link_open(&mut self) -> Option<String> {
        match self {
            Overlay::Transcript(o) => o.view.take_pending_link_open(),
            Overlay::Static(o) => o.view.take_pending_link_open(),
        }
    }

    pub(crate) fn set_notice(&mut self, notice: Option<String>) {
        match self {
            Overlay::Transcript(o) => o.view.notice = notice,
            Overlay::Static(o) => o.view.notice = notice,
        }
    }
}

fn first_or_empty(bindings: &[KeyBinding]) -> Vec<KeyBinding> {
    bindings.first().copied().into_iter().collect()
}

/// Whether the key event is a "copy selection" chord: Ctrl+C, Ctrl+Shift+C,
/// Super+C, or Ctrl+Insert (key releases excluded).
fn is_copy_key(key: KeyEvent) -> bool {
    if key.kind == KeyEventKind::Release {
        return false;
    }
    let (code, modifiers) = key_hint::normalize_key_parts(key.code, key.modifiers);
    match code {
        KeyCode::Char('c') => modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER),
        KeyCode::Insert => modifiers.contains(KeyModifiers::CONTROL),
        _ => false,
    }
}

/// Extract the destination from an OSC 8 hyperlink escape sequence embedded in
/// a buffer cell symbol (`\x1b]8;;<destination>\x07<text>...`).
fn osc8_destination(symbol: &str) -> Option<&str> {
    let rest = symbol.strip_prefix("\x1b]8;;")?;
    let end = rest.find('\x07')?;
    let destination = &rest[..end];
    (!destination.is_empty()).then_some(destination)
}

// Render a single line of key hints from (key(s), description) pairs.
fn render_key_hints(area: Rect, buf: &mut Buffer, pairs: &[(Vec<KeyBinding>, &str)]) {
    let mut spans: Vec<Span<'static>> = vec![" ".into()];
    let mut first = true;
    for (keys, desc) in pairs {
        if !first {
            spans.push("   ".into());
        }
        for (i, key) in keys.iter().enumerate() {
            if i > 0 {
                spans.push("/".into());
            }
            spans.push(Span::from(key));
        }
        spans.push(" ".into());
        spans.push(Span::from(desc.to_string()));
        first = false;
    }
    Paragraph::new(vec![Line::from(spans).dim()]).render_ref(area, buf);
}

/// Generic widget for rendering a pager view.
struct PagerView {
    renderables: Vec<Box<dyn Renderable>>,
    scroll_offset: usize,
    title: String,
    keymap: PagerKeymap,
    last_content_height: Option<usize>,
    last_rendered_height: Option<usize>,
    /// If set, on next render ensure this chunk is visible.
    pending_scroll_chunk: Option<usize>,
    /// Mouse wheel speed multiplier; one wheel event is one row at `1.0`.
    mouse_scroll_speed: f64,
    /// Fractional wheel remainder carried between events; reset on direction change.
    pending_mouse_scroll: f64,
    /// Active mouse-drag text selection in content coordinates.
    selection: Option<PagerSelection>,
    /// Content area of the last render, used to translate screen to content
    /// coordinates for mouse events.
    last_content_area: Option<Rect>,
    /// Text queued for the `App` layer to copy to the system clipboard.
    pending_copy: Option<String>,
    /// Link destination queued for the `App` layer to open in a browser.
    pending_link_open: Option<String>,
    /// Transient status message rendered in the bottom bar.
    notice: Option<String>,
    /// Hyperlink destinations by screen cell, rebuilt on every render from
    /// OSC 8 sequences embedded in buffer symbols.
    last_links: HashMap<(u16, u16), String>,
}

/// A mouse text selection in content coordinates: `(content_row, column)`.
/// `content_row` is absolute (already includes the scroll offset); `column` is
/// relative to the content area's left edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PagerSelection {
    anchor: (usize, u16),
    pointer: (usize, u16),
    dragging: bool,
}

impl PagerSelection {
    /// Ordered `(start, end)` bounds so `start <= end` in row-major order.
    fn ordered_bounds(&self) -> ((usize, u16), (usize, u16)) {
        if self.anchor <= self.pointer {
            (self.anchor, self.pointer)
        } else {
            (self.pointer, self.anchor)
        }
    }
}

impl PagerView {
    fn new(
        renderables: Vec<Box<dyn Renderable>>,
        title: String,
        scroll_offset: usize,
        keymap: PagerKeymap,
    ) -> Self {
        Self {
            renderables,
            scroll_offset,
            title,
            keymap,
            last_content_height: None,
            last_rendered_height: None,
            pending_scroll_chunk: None,
            mouse_scroll_speed: 1.0,
            pending_mouse_scroll: 0.0,
            selection: None,
            last_content_area: None,
            pending_copy: None,
            pending_link_open: None,
            notice: None,
            last_links: HashMap::new(),
        }
    }

    fn content_height(&self, width: u16) -> usize {
        self.renderables
            .iter()
            .map(|c| c.desired_height(width) as usize)
            .sum()
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);
        self.render_header(area, buf);
        let content_area = self.content_area(area);
        self.last_content_area = Some(content_area);
        self.update_last_content_height(content_area.height);
        let content_height = self.content_height(content_area.width);
        self.last_rendered_height = Some(content_height);
        // If there is a pending request to scroll a specific chunk into view,
        // satisfy it now that wrapping is up to date for this width.
        if let Some(idx) = self.pending_scroll_chunk.take() {
            self.ensure_chunk_visible(idx, content_area);
        }
        self.scroll_offset = self
            .scroll_offset
            .min(content_height.saturating_sub(content_area.height as usize));

        self.render_content(content_area, buf);
        self.apply_selection_highlight(content_area, buf);
        self.rescan_links(content_area, buf);

        self.render_bottom_bar(area, content_area, buf, content_height);
    }

    /// Rebuild the screen-cell -> link destination map by scanning the buffer
    /// for OSC 8 hyperlink sequences embedded in cell symbols.
    fn rescan_links(&mut self, content_area: Rect, buf: &Buffer) {
        self.last_links.clear();
        for y in content_area.y..content_area.bottom() {
            for x in content_area.x..content_area.right() {
                if let Some(destination) = osc8_destination(buf[(x, y)].symbol()) {
                    self.last_links.insert((x, y), destination.to_string());
                }
            }
        }
    }

    /// Apply reverse-video styling to the selected cell range. Selection
    /// coordinates are content coordinates; only visible rows are touched.
    fn apply_selection_highlight(&self, content_area: Rect, buf: &mut Buffer) {
        let Some(selection) = self.selection else {
            return;
        };
        let ((start_row, start_col), (end_row, end_col)) = selection.ordered_bounds();
        let scroll = self.clamped_scroll_offset();
        for content_row in start_row..=end_row {
            if content_row < scroll {
                continue;
            }
            let screen_row = content_area.y + (content_row - scroll) as u16;
            if screen_row >= content_area.bottom() {
                break;
            }
            let col_start = if content_row == start_row {
                start_col
            } else {
                0
            };
            let col_end = if content_row == end_row {
                end_col.min(content_area.width.saturating_sub(1))
            } else {
                content_area.width.saturating_sub(1)
            };
            for col in col_start..=col_end {
                let x = content_area.x + col;
                buf[(x, screen_row)].set_style(Style::default().add_modifier(Modifier::REVERSED));
            }
        }
    }

    fn render_header(&self, area: Rect, buf: &mut Buffer) {
        Span::from("/ ".repeat(area.width as usize / 2))
            .dim()
            .render_ref(area, buf);
        let header = format!("/ {}", self.title);
        header.dim().render_ref(area, buf);
    }

    fn render_content(&self, area: Rect, buf: &mut Buffer) {
        let mut y = -(self.scroll_offset as isize);
        let mut drawn_bottom = area.y;
        for renderable in &self.renderables {
            let top = y;
            let height = renderable.desired_height(area.width) as isize;
            y += height;
            let bottom = y;
            if bottom < area.y as isize {
                continue;
            }
            if top > area.y as isize + area.height as isize {
                break;
            }
            if top < 0 {
                let drawn = render_offset_content(area, buf, &**renderable, (-top) as u16);
                drawn_bottom = drawn_bottom.max(area.y + drawn);
            } else {
                let draw_height = (height as u16).min(area.height.saturating_sub(top as u16));
                let draw_area = Rect::new(area.x, area.y + top as u16, area.width, draw_height);
                renderable.render(draw_area, buf);
                drawn_bottom = drawn_bottom.max(draw_area.y.saturating_add(draw_area.height));
            }
        }

        for y in drawn_bottom..area.bottom() {
            if area.width == 0 {
                break;
            }
            buf[(area.x, y)] = Cell::from('~');
            for x in area.x + 1..area.right() {
                buf[(x, y)] = Cell::from(' ');
            }
        }
    }

    fn render_bottom_bar(
        &self,
        full_area: Rect,
        content_area: Rect,
        buf: &mut Buffer,
        total_len: usize,
    ) {
        let sep_y = content_area.bottom();
        let sep_rect = Rect::new(full_area.x, sep_y, full_area.width, 1);

        Span::from("─".repeat(sep_rect.width as usize))
            .dim()
            .render_ref(sep_rect, buf);
        let percent = if total_len == 0 {
            100
        } else {
            let max_scroll = total_len.saturating_sub(content_area.height as usize);
            if max_scroll == 0 {
                100
            } else {
                (((self.scroll_offset.min(max_scroll)) as f32 / max_scroll as f32) * 100.0).round()
                    as u8
            }
        };
        let pct_text = format!(" {percent}% ");
        let pct_w = pct_text.chars().count() as u16;
        let pct_x = sep_rect.x + sep_rect.width - pct_w - 1;
        Span::from(pct_text)
            .dim()
            .render_ref(Rect::new(pct_x, sep_rect.y, pct_w, 1), buf);
        if let Some(notice) = &self.notice {
            let notice_text = format!(" {notice} ");
            let notice_w = (notice_text.chars().count() as u16).min(sep_rect.width);
            Span::from(notice_text)
                .dim()
                .render_ref(Rect::new(sep_rect.x, sep_rect.y, notice_w, 1), buf);
        }
    }

    fn handle_key_event(&mut self, tui: &mut tui::Tui, key_event: KeyEvent) -> Result<()> {
        match key_event {
            KeyEvent {
                code: KeyCode::Esc,
                kind: KeyEventKind::Press | KeyEventKind::Repeat,
                ..
            } if self.selection.is_some() => {
                self.clear_selection();
            }
            e if self.keymap.scroll_up.is_pressed(e) => {
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
            }
            e if self.keymap.scroll_down.is_pressed(e) => {
                self.scroll_offset = self.scroll_offset.saturating_add(1);
            }
            e if self.keymap.page_up.is_pressed(e) => {
                let page_height = self.page_height(tui.terminal.viewport_area);
                self.scroll_offset = self.scroll_offset.saturating_sub(page_height);
            }
            e if self.keymap.page_down.is_pressed(e) => {
                let page_height = self.page_height(tui.terminal.viewport_area);
                self.scroll_offset = self.scroll_offset.saturating_add(page_height);
            }
            e if self.keymap.half_page_down.is_pressed(e) => {
                let area = self.content_area(tui.terminal.viewport_area);
                let half_page = (area.height as usize).saturating_add(1) / 2;
                self.scroll_offset = self.scroll_offset.saturating_add(half_page);
            }
            e if self.keymap.half_page_up.is_pressed(e) => {
                let area = self.content_area(tui.terminal.viewport_area);
                let half_page = (area.height as usize).saturating_add(1) / 2;
                self.scroll_offset = self.scroll_offset.saturating_sub(half_page);
            }
            e if self.keymap.jump_top.is_pressed(e) => {
                self.scroll_offset = 0;
            }
            e if self.keymap.jump_bottom.is_pressed(e) => {
                self.scroll_offset = usize::MAX;
            }
            _ => {
                return Ok(());
            }
        }
        tui.frame_requester()
            .schedule_frame_in(crate::tui::TARGET_FRAME_INTERVAL);
        Ok(())
    }

    /// Handle a mouse event; returns `true` when the view changed and needs a redraw.
    ///
    /// Wheel events scroll (scaled by `mouse_scroll_speed`). Left-drag within the
    /// content area selects text; a right click with an active selection queues
    /// the selection for clipboard copy; a left click (press+release without
    /// dragging) on a hyperlink queues it to be opened and clears any selection.
    fn handle_mouse_event(&mut self, event: MouseEvent) -> bool {
        match event.kind {
            MouseEventKind::ScrollUp => {
                self.apply_wheel_scroll(-1.0);
                true
            }
            MouseEventKind::ScrollDown => {
                self.apply_wheel_scroll(1.0);
                true
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let Some(coords) = self.screen_to_content(event.column, event.row) else {
                    return false;
                };
                self.selection = Some(PagerSelection {
                    anchor: coords,
                    pointer: coords,
                    dragging: true,
                });
                true
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(selection) = self.selection else {
                    return false;
                };
                if !selection.dragging {
                    return false;
                }
                let Some(coords) = self.screen_to_content_clamped(event.column, event.row) else {
                    return false;
                };
                self.selection = Some(PagerSelection {
                    pointer: coords,
                    ..selection
                });
                true
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let Some(selection) = &mut self.selection else {
                    return false;
                };
                if !selection.dragging {
                    return false;
                }
                selection.dragging = false;
                if selection.anchor == selection.pointer {
                    // Plain click: open a link if there is one under the cursor,
                    // and never leave a zero-width selection behind.
                    if let Some(destination) = self.last_links.get(&(event.column, event.row)) {
                        self.pending_link_open = Some(destination.clone());
                    }
                    self.selection = None;
                }
                true
            }
            MouseEventKind::Down(MouseButton::Right) => {
                if self.screen_to_content(event.column, event.row).is_none() {
                    return false;
                }
                self.queue_copy_selection()
            }
            _ => false,
        }
    }

    /// Translate screen coordinates to content coordinates, returning `None`
    /// when the point lies outside the last rendered content area.
    fn screen_to_content(&self, column: u16, row: u16) -> Option<(usize, u16)> {
        let area = self.last_content_area?;
        if column < area.x || column >= area.right() || row < area.y || row >= area.bottom() {
            return None;
        }
        Some((
            self.clamped_scroll_offset() + (row - area.y) as usize,
            column - area.x,
        ))
    }

    /// Like `screen_to_content`, but clamps points outside the content area to
    /// its nearest edge so drags past the top/bottom keep extending the
    /// selection.
    fn screen_to_content_clamped(&self, column: u16, row: u16) -> Option<(usize, u16)> {
        let area = self.last_content_area?;
        if area.width == 0 || area.height == 0 {
            return None;
        }
        let column = column.clamp(area.x, area.right() - 1);
        let row = row.clamp(area.y, area.bottom() - 1);
        Some((
            self.clamped_scroll_offset() + (row - area.y) as usize,
            column - area.x,
        ))
    }

    /// The effective scroll offset; `usize::MAX` means "pinned to bottom" and
    /// is only resolved during render, so fall back to the last rendered max.
    fn clamped_scroll_offset(&self) -> usize {
        if self.scroll_offset == usize::MAX {
            let total = self.last_rendered_height.unwrap_or(0);
            let height = self.last_content_height.unwrap_or(0);
            total.saturating_sub(height)
        } else {
            self.scroll_offset
        }
    }

    /// Extract the currently selected text, or `None` when there is no
    /// selection (or it selects only whitespace).
    fn selected_text(&self) -> Option<String> {
        let selection = self.selection?;
        let area = self.last_content_area?;
        if area.width == 0 {
            return None;
        }
        let ((start_row, start_col), (end_row, end_col)) = selection.ordered_bounds();
        let row_count = end_row - start_row + 1;
        let buf = self.render_rows(start_row, row_count, area.width);

        let mut lines = Vec::with_capacity(row_count);
        for (row_idx, content_row) in (start_row..=end_row).enumerate() {
            let col_start = if content_row == start_row {
                start_col as usize
            } else {
                0
            };
            let col_end = if content_row == end_row {
                end_col as usize
            } else {
                area.width as usize - 1
            };
            let mut line = String::new();
            for col in col_start..=col_end.min(area.width as usize - 1) {
                let cell = &buf[(col as u16, row_idx as u16)];
                if !cell.skip {
                    line.push_str(&strip_osc8(cell.symbol()));
                }
            }
            lines.push(line.trim_end().to_string());
        }
        while lines.first().is_some_and(|line| line.is_empty()) {
            lines.remove(0);
        }
        while lines.last().is_some_and(|line| line.is_empty()) {
            lines.pop();
        }
        if lines.is_empty() {
            None
        } else {
            Some(lines.join("\n"))
        }
    }

    /// Render `row_count` content rows starting at `first_row` into an
    /// off-screen buffer, skipping chunks that do not intersect the range.
    fn render_rows(&self, first_row: usize, row_count: usize, width: u16) -> Buffer {
        let mut buf = Buffer::empty(Rect::new(0, 0, width, row_count as u16));
        let mut top = 0usize;
        for renderable in &self.renderables {
            let height = renderable.desired_height(width) as usize;
            let bottom = top + height;
            if bottom <= first_row || top >= first_row + row_count {
                top = bottom;
                continue;
            }
            let skip = first_row.saturating_sub(top) as u16;
            let dest_y = top.saturating_sub(first_row) as u16;
            let avail = row_count.saturating_sub(dest_y as usize) as u16;
            let area = Rect::new(0, dest_y, width, avail);
            render_offset_content(area, &mut buf, &**renderable, skip);
            top = bottom;
        }
        buf
    }

    /// Move the current selection into `pending_copy` for the `App` layer to
    /// write to the system clipboard. Returns `false` when there is nothing to
    /// copy.
    fn queue_copy_selection(&mut self) -> bool {
        let Some(text) = self.selected_text() else {
            return false;
        };
        self.pending_copy = Some(text);
        self.selection = None;
        self.notice = Some("Copied selection to clipboard".to_string());
        true
    }

    /// Clear the active selection; returns `true` when there was one.
    fn clear_selection(&mut self) -> bool {
        self.selection.take().is_some()
    }

    fn take_pending_copy(&mut self) -> Option<String> {
        self.pending_copy.take()
    }

    fn take_pending_link_open(&mut self) -> Option<String> {
        self.pending_link_open.take()
    }

    /// Apply one wheel notch in `direction` (`-1.0` up, `1.0` down), scaling by
    /// `mouse_scroll_speed` and carrying fractional rows between events.
    fn apply_wheel_scroll(&mut self, direction: f64) {
        if self.pending_mouse_scroll.signum() != direction {
            self.pending_mouse_scroll = 0.0;
        }
        self.pending_mouse_scroll += direction * self.mouse_scroll_speed;
        // Decimal speeds can land just short of a whole row after repeated addition.
        let rounded = self.pending_mouse_scroll.round();
        if rounded != 0.0 && (self.pending_mouse_scroll - rounded).abs() < 1e-9 {
            self.pending_mouse_scroll = rounded;
        }
        let rows = self.pending_mouse_scroll as isize;
        self.pending_mouse_scroll = self.pending_mouse_scroll.fract();
        if rows > 0 {
            self.scroll_offset = self.scroll_offset.saturating_add(rows as usize);
        } else if rows < 0 {
            self.scroll_offset = self.scroll_offset.saturating_sub(rows.unsigned_abs());
        }
    }

    /// Returns the height of one page in content rows.
    ///
    /// Prefers the last rendered content height (excluding header/footer chrome);
    /// if no render has occurred yet, falls back to the content area height
    /// computed from the given viewport.
    fn page_height(&self, viewport_area: Rect) -> usize {
        self.last_content_height
            .unwrap_or_else(|| self.content_area(viewport_area).height as usize)
    }

    fn update_last_content_height(&mut self, height: u16) {
        self.last_content_height = Some(height as usize);
    }

    fn content_area(&self, area: Rect) -> Rect {
        let mut area = area;
        area.y = area.y.saturating_add(1);
        area.height = area.height.saturating_sub(2);
        area
    }
}

impl PagerView {
    fn is_scrolled_to_bottom(&self) -> bool {
        if self.scroll_offset == usize::MAX {
            return true;
        }
        let Some(height) = self.last_content_height else {
            return false;
        };
        if self.renderables.is_empty() {
            return true;
        }
        let Some(total_height) = self.last_rendered_height else {
            return false;
        };
        if total_height <= height {
            return true;
        }
        let max_scroll = total_height.saturating_sub(height);
        self.scroll_offset >= max_scroll
    }

    /// Request that the given text chunk index be scrolled into view on next render.
    fn scroll_chunk_into_view(&mut self, chunk_index: usize) {
        self.pending_scroll_chunk = Some(chunk_index);
    }

    fn ensure_chunk_visible(&mut self, idx: usize, area: Rect) {
        if area.height == 0 || idx >= self.renderables.len() {
            return;
        }
        let first = self
            .renderables
            .iter()
            .take(idx)
            .map(|r| r.desired_height(area.width) as usize)
            .sum();
        let last = first + self.renderables[idx].desired_height(area.width) as usize;
        let current_top = self.scroll_offset;
        let current_bottom = current_top.saturating_add(area.height.saturating_sub(1) as usize);
        if first < current_top {
            self.scroll_offset = first;
        } else if last > current_bottom {
            self.scroll_offset = last.saturating_sub(area.height.saturating_sub(1) as usize);
        }
    }
}

/// A renderable that caches its desired height.
struct CachedRenderable {
    renderable: Box<dyn Renderable>,
    height: std::cell::Cell<Option<u16>>,
    last_width: std::cell::Cell<Option<u16>>,
}

impl CachedRenderable {
    fn new(renderable: impl Into<Box<dyn Renderable>>) -> Self {
        Self {
            renderable: renderable.into(),
            height: std::cell::Cell::new(None),
            last_width: std::cell::Cell::new(None),
        }
    }
}

impl Renderable for CachedRenderable {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.renderable.render(area, buf);
    }
    fn desired_height(&self, width: u16) -> u16 {
        if self.last_width.get() != Some(width) {
            let height = self.renderable.desired_height(width);
            self.height.set(Some(height));
            self.last_width.set(Some(width));
        }
        self.height.get().unwrap_or(0)
    }
}

enum CellRenderableMode {
    Transcript,
    Display,
}

struct CellRenderable {
    cell: Arc<dyn HistoryCell>,
    style: Style,
    mode: CellRenderableMode,
}

impl Renderable for CellRenderable {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let hyperlink_lines = match self.mode {
            CellRenderableMode::Transcript => self.cell.transcript_hyperlink_lines(area.width),
            CellRenderableMode::Display => self
                .cell
                .display_hyperlink_lines_for_mode(area.width, HistoryRenderMode::Rich),
        };
        let p = Paragraph::new(Text::from(visible_lines(hyperlink_lines.clone())))
            .style(self.style)
            .wrap(Wrap { trim: false });
        p.render(area, buf);
        mark_buffer_hyperlinks(buf, area, &hyperlink_lines, /*scroll_rows*/ 0);
    }

    fn desired_height(&self, width: u16) -> u16 {
        match self.mode {
            CellRenderableMode::Transcript => self.cell.desired_transcript_height(width),
            CellRenderableMode::Display => self
                .cell
                .desired_height_for_mode(width, HistoryRenderMode::Rich),
        }
    }
}

struct HyperlinkLinesRenderable {
    lines: Vec<HyperlinkLine>,
}

impl Renderable for HyperlinkLinesRenderable {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        Paragraph::new(Text::from(visible_lines(self.lines.clone())))
            .wrap(Wrap { trim: false })
            .render(area, buf);
        mark_buffer_hyperlinks(buf, area, &self.lines, /*scroll_rows*/ 0);
    }

    fn desired_height(&self, width: u16) -> u16 {
        Paragraph::new(Text::from(visible_lines(self.lines.clone())))
            .wrap(Wrap { trim: false })
            .line_count(width)
            .try_into()
            .unwrap_or(/*default*/ 0)
    }
}

/// `/copy` navigation state: every semantic copy target in display order
/// plus the current selection. Targets are computed on entry and reference
/// cell indices, which are stable because committed cells are append-only
/// while the overlay is open.
#[derive(Debug)]
struct CopyModeState {
    targets: Vec<copy_targets::CopyTarget>,
    selected: usize,
}

pub(crate) struct TranscriptOverlay {
    /// Pager UI state and the renderables currently displayed.
    ///
    /// The invariant is that `view.renderables` is `render_cells(cells)` plus an optional trailing
    /// live-tail renderable appended after the committed cells.
    view: PagerView,
    /// Committed transcript cells (does not include the live tail).
    cells: Vec<Arc<dyn HistoryCell>>,
    highlight_cell: Option<usize>,
    /// `/copy` semantic-navigation mode: the flattened copy targets (whole
    /// responses, user messages, code blocks, blockquotes) are walked with
    /// Up/Down (or k/j); Enter (or a copy chord) copies the selected target's
    /// original Markdown payload; Esc exits the mode.
    copy_mode: Option<CopyModeState>,
    /// Cache key for the render-only live tail appended after committed cells.
    live_tail_key: Option<LiveTailKey>,
    is_done: bool,
}

/// Cache key for the active-cell "live tail" appended to the transcript overlay.
///
/// Changing any field implies a different rendered tail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LiveTailKey {
    /// Current terminal width, which affects wrapping.
    width: u16,
    /// Revision that changes on in-place active cell transcript updates.
    revision: u64,
    /// Whether the tail should be treated as a continuation for spacing.
    is_stream_continuation: bool,
    /// Optional animation tick to refresh spinners/progress indicators.
    animation_tick: Option<u64>,
}

impl TranscriptOverlay {
    /// Creates a transcript overlay for a fixed set of committed cells.
    ///
    /// This overlay does not own the "active cell"; callers may optionally append a live tail via
    /// `sync_live_tail` during draws to reflect in-flight activity.
    pub(crate) fn new(
        transcript_cells: Vec<Arc<dyn HistoryCell>>,
        keymap: PagerKeymap,
        mouse_scroll_speed: f64,
    ) -> Self {
        let mut view = PagerView::new(
            Self::render_cells(&transcript_cells, /*highlight_cell*/ None),
            "T R A N S C R I P T".to_string(),
            usize::MAX,
            keymap,
        );
        view.mouse_scroll_speed = mouse_scroll_speed;
        Self {
            view,
            cells: transcript_cells,
            highlight_cell: None,
            copy_mode: None,
            live_tail_key: None,
            is_done: false,
        }
    }

    fn render_cells(
        cells: &[Arc<dyn HistoryCell>],
        highlight_cell: Option<usize>,
    ) -> Vec<Box<dyn Renderable>> {
        cells
            .iter()
            .enumerate()
            .flat_map(|(i, c)| {
                let mut v: Vec<Box<dyn Renderable>> = Vec::new();
                let mut cell_renderable = if c.as_any().is::<UserHistoryCell>() {
                    Box::new(CachedRenderable::new(CellRenderable {
                        cell: c.clone(),
                        style: if highlight_cell == Some(i) {
                            user_message_style().reversed()
                        } else {
                            user_message_style()
                        },
                        mode: CellRenderableMode::Transcript,
                    })) as Box<dyn Renderable>
                } else {
                    Box::new(CachedRenderable::new(CellRenderable {
                        cell: c.clone(),
                        style: if highlight_cell == Some(i) {
                            Style::default().reversed()
                        } else {
                            Style::default()
                        },
                        mode: CellRenderableMode::Transcript,
                    })) as Box<dyn Renderable>
                };
                if !c.is_stream_continuation() && i > 0 {
                    cell_renderable = Box::new(InsetRenderable::new(
                        cell_renderable,
                        Insets::tlbr(
                            /*top*/ 1, /*left*/ 0, /*bottom*/ 0, /*right*/ 0,
                        ),
                    ));
                }
                v.push(cell_renderable);
                v
            })
            .collect()
    }

    /// Insert a committed history cell while keeping any cached live tail.
    ///
    /// The live tail is temporarily removed, the committed cells are rebuilt,
    /// then the tail is reattached. If the tail previously had no leading
    /// spacing because it was the only renderable, we add the missing inset
    /// when the first committed cell arrives.
    ///
    /// This expects `cell` to be a committed transcript cell (not the in-flight active cell). If
    /// the overlay was scrolled to bottom before insertion, it remains pinned to bottom after the
    /// insertion to preserve the "follow along" behavior.
    pub(crate) fn insert_cell(&mut self, cell: Arc<dyn HistoryCell>) {
        let follow_bottom = self.view.is_scrolled_to_bottom();
        let had_prior_cells = !self.cells.is_empty();
        let tail_renderable = self.take_live_tail_renderable();
        self.cells.push(cell);
        self.view.renderables = Self::render_cells(&self.cells, self.highlight_cell);
        if let Some(tail) = tail_renderable {
            let tail = if !had_prior_cells
                && self
                    .live_tail_key
                    .is_some_and(|key| !key.is_stream_continuation)
            {
                // The tail was rendered as the only entry, so it lacks a top
                // inset; add one now that it follows a committed cell.
                Box::new(InsetRenderable::new(
                    tail,
                    Insets::tlbr(
                        /*top*/ 1, /*left*/ 0, /*bottom*/ 0, /*right*/ 0,
                    ),
                )) as Box<dyn Renderable>
            } else {
                tail
            };
            self.view.renderables.push(tail);
        }
        if follow_bottom {
            self.view.scroll_offset = usize::MAX;
        }
    }

    /// Replace committed transcript cells while keeping any cached in-progress output that is
    /// currently shown at the end of the overlay.
    ///
    /// This is used when existing history is trimmed (for example after rollback) so the
    /// transcript overlay immediately reflects the same committed cells as the main transcript.
    pub(crate) fn replace_cells(&mut self, cells: Vec<Arc<dyn HistoryCell>>) {
        let follow_bottom = self.view.is_scrolled_to_bottom();
        self.cells = cells;
        if self
            .highlight_cell
            .is_some_and(|idx| idx >= self.cells.len())
        {
            self.highlight_cell = None;
        }
        self.rebuild_renderables();
        if follow_bottom {
            self.view.scroll_offset = usize::MAX;
        }
    }

    /// Replace a range of committed cells with a single consolidated cell.
    ///
    /// Mirrors the splice performed on `App::transcript_cells` during
    /// `ConsolidateAgentMessage` so the Ctrl+T overlay stays in sync with the
    /// main transcript. The range is clamped defensively: cells may have been
    /// inserted after the overlay opened, leaving it with fewer entries than
    /// the main transcript.
    pub(crate) fn consolidate_cells(
        &mut self,
        range: std::ops::Range<usize>,
        consolidated: Arc<dyn HistoryCell>,
    ) {
        let follow_bottom = self.view.is_scrolled_to_bottom();
        // Clamp the range to the overlay's cell count to avoid panic if the overlay has fewer
        // cells than the main transcript (e.g. cells were inserted after the overlay has opened).
        let clamped_end = range.end.min(self.cells.len());
        let clamped_start = range.start.min(clamped_end);
        if clamped_start < clamped_end {
            let removed = clamped_end - clamped_start;
            if let Some(highlight_cell) = self.highlight_cell.as_mut()
                && *highlight_cell >= clamped_start
            {
                if *highlight_cell < clamped_end {
                    *highlight_cell = clamped_start;
                } else {
                    *highlight_cell = highlight_cell.saturating_sub(removed.saturating_sub(1));
                }
            }
            self.cells
                .splice(clamped_start..clamped_end, std::iter::once(consolidated));
            if self
                .highlight_cell
                .is_some_and(|highlight_cell| highlight_cell >= self.cells.len())
            {
                self.highlight_cell = None;
            }
            self.rebuild_renderables();
        }
        if follow_bottom {
            self.view.scroll_offset = usize::MAX;
        }
    }

    /// Sync the active-cell live tail with the current width and cell state.
    ///
    /// Recomputes the tail only when the cache key changes, preserving scroll
    /// position and dropping the tail if there is nothing to render.
    ///
    /// The overlay owns committed transcript cells while the live tail is derived from the current
    /// active cell, which can mutate in place while streaming. `App` calls this during
    /// `TuiEvent::Draw` for `Overlay::Transcript`, passing a key that changes when the active cell
    /// mutates or animates so the cached tail stays fresh.
    ///
    /// Passing a key that does not change on in-place active-cell mutations will freeze the tail in
    /// `Ctrl+T` while the main viewport continues to update.
    pub(crate) fn sync_live_tail(
        &mut self,
        width: u16,
        active_key: Option<ActiveCellTranscriptKey>,
        compute_lines: impl FnOnce(u16) -> Option<Vec<HyperlinkLine>>,
    ) {
        let next_key = active_key.map(|key| LiveTailKey {
            width,
            revision: key.revision,
            is_stream_continuation: key.is_stream_continuation,
            animation_tick: key.animation_tick,
        });

        if self.live_tail_key == next_key {
            return;
        }
        let follow_bottom = self.view.is_scrolled_to_bottom();

        self.take_live_tail_renderable();
        self.live_tail_key = next_key;

        if let Some(key) = next_key {
            let lines = compute_lines(width).unwrap_or_default();
            if !lines.is_empty() {
                self.view.renderables.push(Self::live_tail_renderable(
                    lines,
                    !self.cells.is_empty(),
                    key.is_stream_continuation,
                ));
            }
        }
        if follow_bottom {
            self.view.scroll_offset = usize::MAX;
        }
    }

    pub(crate) fn set_highlight_cell(&mut self, cell: Option<usize>) {
        self.highlight_cell = cell;
        self.rebuild_renderables();
        if let Some(idx) = self.highlight_cell {
            self.view.scroll_chunk_into_view(idx);
        }
    }

    /// Enter `/copy` semantic-navigation mode: flatten the transcript's copy
    /// targets (whole responses, user messages, code blocks, blockquotes) and
    /// select the most recent response, so Up/Down walks semantic blocks and
    /// Enter copies the target's original Markdown payload.
    pub(crate) fn enter_copy_mode(&mut self) {
        let targets = copy_targets::copy_targets(&self.cells);
        if targets.is_empty() {
            return;
        }
        let selected = targets
            .iter()
            .rposition(|target| target.kind == copy_targets::CopyTargetKind::WholeResponse)
            .unwrap_or(targets.len() - 1);
        let cell_index = targets[selected].cell_index;
        self.copy_mode = Some(CopyModeState { targets, selected });
        self.set_highlight_cell(Some(cell_index));
    }

    /// Handle a key while copy mode is active; returns `true` when the key was
    /// consumed. Unrelated keys fall through to normal overlay handling.
    pub(crate) fn copy_mode_handle_key(&mut self, key: KeyEvent) -> bool {
        if self.copy_mode.is_none() {
            return false;
        }
        if matches!(key.kind, KeyEventKind::Release) {
            return false;
        }
        match key.code {
            KeyCode::Esc => {
                self.exit_copy_mode();
                true
            }
            KeyCode::Up | KeyCode::Char('k') => self.copy_mode_move(-1),
            KeyCode::Down | KeyCode::Char('j') => self.copy_mode_move(1),
            KeyCode::Home | KeyCode::Char('g') => self.copy_mode_jump(0),
            KeyCode::End | KeyCode::Char('G') => self.copy_mode_jump(usize::MAX),
            KeyCode::Enter => self.copy_mode_commit(),
            _ if is_copy_key(key) => self.copy_mode_commit(),
            _ => false,
        }
    }

    fn copy_mode_move(&mut self, delta: isize) -> bool {
        let Some(state) = self.copy_mode.as_mut() else {
            return false;
        };
        let next = state
            .selected
            .saturating_add_signed(delta)
            .min(state.targets.len().saturating_sub(1));
        state.selected = next;
        let cell_index = state.targets[next].cell_index;
        self.set_highlight_cell(Some(cell_index));
        true
    }

    fn copy_mode_jump(&mut self, selected: usize) -> bool {
        let Some(state) = self.copy_mode.as_mut() else {
            return false;
        };
        state.selected = selected.min(state.targets.len().saturating_sub(1));
        let cell_index = state.targets[state.selected].cell_index;
        self.set_highlight_cell(Some(cell_index));
        true
    }

    /// Copy the selected target's original Markdown payload into the view's
    /// pending copy slot, exit copy mode, and jump back to the bottom.
    fn copy_mode_commit(&mut self) -> bool {
        let Some(state) = &self.copy_mode else {
            return false;
        };
        let target = &state.targets[state.selected];
        // Copy the carried Markdown source byte-identically: trailing
        // whitespace is part of the payload and must not be trimmed.
        let text = target.source.clone();
        if text.is_empty() {
            return false;
        }
        let label = target.kind.label().to_lowercase();
        self.view.pending_copy = Some(text);
        self.exit_copy_mode();
        self.view.scroll_offset = usize::MAX;
        self.view.notice = Some(format!("Copied {label} to clipboard"));
        true
    }

    fn exit_copy_mode(&mut self) {
        self.copy_mode = None;
        self.set_highlight_cell(None);
    }

    /// Returns whether the underlying pager view is currently pinned to the bottom.
    ///
    /// The `App` draw loop uses this to decide whether to schedule animation frames for the live
    /// tail; if the user has scrolled up, we avoid driving animation work that they cannot see.
    pub(crate) fn is_scrolled_to_bottom(&self) -> bool {
        self.view.is_scrolled_to_bottom()
    }

    fn rebuild_renderables(&mut self) {
        let tail_renderable = self.take_live_tail_renderable();
        self.view.renderables = Self::render_cells(&self.cells, self.highlight_cell);
        if let Some(tail) = tail_renderable {
            self.view.renderables.push(tail);
        }
    }

    /// Removes and returns the cached live-tail renderable, if present.
    ///
    /// The live tail is represented as a single optional renderable appended after the committed
    /// cell renderables, so this relies on the live tail always being the final entry in
    /// `view.renderables` when present.
    fn take_live_tail_renderable(&mut self) -> Option<Box<dyn Renderable>> {
        (self.view.renderables.len() > self.cells.len()).then(|| self.view.renderables.pop())?
    }

    fn live_tail_renderable(
        lines: Vec<HyperlinkLine>,
        has_prior_cells: bool,
        is_stream_continuation: bool,
    ) -> Box<dyn Renderable> {
        let mut renderable: Box<dyn Renderable> =
            Box::new(CachedRenderable::new(HyperlinkLinesRenderable { lines }));
        if has_prior_cells && !is_stream_continuation {
            renderable = Box::new(InsetRenderable::new(
                renderable,
                Insets::tlbr(
                    /*top*/ 1, /*left*/ 0, /*bottom*/ 0, /*right*/ 0,
                ),
            ));
        }
        renderable
    }

    fn render_hints(&self, area: Rect, buf: &mut Buffer) {
        let line1 = Rect::new(area.x, area.y, area.width, 1);
        let line2 = Rect::new(area.x, area.y.saturating_add(1), area.width, 1);
        render_key_hints(
            line1,
            buf,
            &[
                (
                    first_or_empty(&self.view.keymap.scroll_up)
                        .into_iter()
                        .chain(first_or_empty(&self.view.keymap.scroll_down))
                        .collect(),
                    "to scroll",
                ),
                (
                    first_or_empty(&self.view.keymap.page_up)
                        .into_iter()
                        .chain(first_or_empty(&self.view.keymap.page_down))
                        .collect(),
                    "to page",
                ),
                (
                    first_or_empty(&self.view.keymap.jump_top)
                        .into_iter()
                        .chain(first_or_empty(&self.view.keymap.jump_bottom))
                        .collect(),
                    "to jump",
                ),
            ],
        );

        let select_hint;
        let mut pairs: Vec<(Vec<KeyBinding>, &str)> =
            vec![(first_or_empty(&self.view.keymap.close), "to quit")];
        if let Some(state) = &self.copy_mode {
            let target = &state.targets[state.selected];
            select_hint = format!(
                "to select — {} ({}/{})",
                target.kind.label(),
                state.selected + 1,
                state.targets.len()
            );
            pairs.push((
                vec![key_hint::plain(KeyCode::Up), key_hint::plain(KeyCode::Down)],
                &select_hint,
            ));
            pairs.push((vec![key_hint::plain(KeyCode::Enter)], "to copy"));
            pairs.push((vec![key_hint::plain(KeyCode::Esc)], "to cancel"));
        } else if self.highlight_cell.is_some() {
            pairs.push((
                vec![
                    key_hint::plain(KeyCode::Esc),
                    key_hint::plain(KeyCode::Left),
                ],
                "to edit prev",
            ));
            pairs.push((vec![key_hint::plain(KeyCode::Right)], "to edit next"));
            pairs.push((vec![key_hint::plain(KeyCode::Enter)], "to edit message"));
        } else {
            pairs.push((vec![key_hint::plain(KeyCode::Esc)], "to edit prev"));
        }
        render_key_hints(line2, buf, &pairs);
    }

    pub(crate) fn render(&mut self, area: Rect, buf: &mut Buffer) {
        let top_h = area.height.saturating_sub(3);
        let top = Rect::new(area.x, area.y, area.width, top_h);
        let bottom = Rect::new(area.x, area.y + top_h, area.width, 3);
        self.view.render(top, buf);
        self.render_hints(bottom, buf);
    }
}

impl TranscriptOverlay {
    pub(crate) fn handle_event(&mut self, tui: &mut tui::Tui, event: TuiEvent) -> Result<()> {
        match event {
            TuiEvent::Key(key_event) => match key_event {
                // Copy mode gets first dibs on navigation/copy/Esc keys.
                e if self.copy_mode_handle_key(e) => {
                    tui.frame_requester()
                        .schedule_frame_in(crate::tui::TARGET_FRAME_INTERVAL);
                    Ok(())
                }
                // Copy chords take precedence over the close keymap (which
                // includes Ctrl+C) while a selection is active.
                e if self.view.selection.is_some() && is_copy_key(e) => {
                    if self.view.queue_copy_selection() {
                        tui.frame_requester()
                            .schedule_frame_in(crate::tui::TARGET_FRAME_INTERVAL);
                    }
                    Ok(())
                }
                e if self.view.keymap.close.is_pressed(e)
                    || self.view.keymap.close_transcript.is_pressed(e) =>
                {
                    self.is_done = true;
                    Ok(())
                }
                other => self.view.handle_key_event(tui, other),
            },
            TuiEvent::Mouse(mouse_event) => {
                if self.view.handle_mouse_event(mouse_event) {
                    tui.frame_requester()
                        .schedule_frame_in(crate::tui::TARGET_FRAME_INTERVAL);
                }
                Ok(())
            }
            TuiEvent::Draw | TuiEvent::Resize => {
                tui.draw(u16::MAX, |frame| {
                    self.render(frame.area(), frame.buffer);
                })?;
                Ok(())
            }
            _ => Ok(()),
        }
    }
    pub(crate) fn is_done(&self) -> bool {
        self.is_done
    }

    pub(crate) fn wants_esc(&self) -> bool {
        self.copy_mode.is_some() || self.view.selection.is_some()
    }

    #[cfg(test)]
    pub(crate) fn committed_cell_count(&self) -> usize {
        self.cells.len()
    }

    #[cfg(test)]
    pub(crate) fn notice_text(&self) -> Option<&str> {
        self.view.notice.as_deref()
    }
}

pub(crate) struct StaticOverlay {
    view: PagerView,
    is_done: bool,
}

impl StaticOverlay {
    pub(crate) fn with_title(
        lines: Vec<Line<'static>>,
        title: String,
        keymap: PagerKeymap,
    ) -> Self {
        let paragraph = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
        Self::with_renderables(
            vec![Box::new(CachedRenderable::new(paragraph))],
            title,
            keymap,
        )
    }

    pub(crate) fn with_renderables(
        renderables: Vec<Box<dyn Renderable>>,
        title: String,
        keymap: PagerKeymap,
    ) -> Self {
        Self {
            view: PagerView::new(renderables, title, /*scroll_offset*/ 0, keymap),
            is_done: false,
        }
    }

    fn render_hints(&self, area: Rect, buf: &mut Buffer) {
        let line1 = Rect::new(area.x, area.y, area.width, 1);
        let line2 = Rect::new(area.x, area.y.saturating_add(1), area.width, 1);
        render_key_hints(
            line1,
            buf,
            &[
                (
                    first_or_empty(&self.view.keymap.scroll_up)
                        .into_iter()
                        .chain(first_or_empty(&self.view.keymap.scroll_down))
                        .collect(),
                    "to scroll",
                ),
                (
                    first_or_empty(&self.view.keymap.page_up)
                        .into_iter()
                        .chain(first_or_empty(&self.view.keymap.page_down))
                        .collect(),
                    "to page",
                ),
                (
                    first_or_empty(&self.view.keymap.jump_top)
                        .into_iter()
                        .chain(first_or_empty(&self.view.keymap.jump_bottom))
                        .collect(),
                    "to jump",
                ),
            ],
        );
        let pairs: Vec<(Vec<KeyBinding>, &str)> =
            vec![(first_or_empty(&self.view.keymap.close), "to quit")];
        render_key_hints(line2, buf, &pairs);
    }

    pub(crate) fn render(&mut self, area: Rect, buf: &mut Buffer) {
        let top_h = area.height.saturating_sub(3);
        let top = Rect::new(area.x, area.y, area.width, top_h);
        let bottom = Rect::new(area.x, area.y + top_h, area.width, 3);
        self.view.render(top, buf);
        self.render_hints(bottom, buf);
    }
}

impl StaticOverlay {
    pub(crate) fn handle_event(&mut self, tui: &mut tui::Tui, event: TuiEvent) -> Result<()> {
        match event {
            TuiEvent::Key(key_event) => match key_event {
                e if self.view.selection.is_some() && is_copy_key(e) => {
                    if self.view.queue_copy_selection() {
                        tui.frame_requester()
                            .schedule_frame_in(crate::tui::TARGET_FRAME_INTERVAL);
                    }
                    Ok(())
                }
                e if self.view.keymap.close.is_pressed(e) => {
                    self.is_done = true;
                    Ok(())
                }
                other => self.view.handle_key_event(tui, other),
            },
            TuiEvent::Mouse(mouse_event) => {
                if self.view.handle_mouse_event(mouse_event) {
                    tui.frame_requester()
                        .schedule_frame_in(crate::tui::TARGET_FRAME_INTERVAL);
                }
                Ok(())
            }
            TuiEvent::Draw | TuiEvent::Resize => {
                tui.draw(u16::MAX, |frame| {
                    self.render(frame.area(), frame.buffer);
                })?;
                Ok(())
            }
            _ => Ok(()),
        }
    }
    pub(crate) fn is_done(&self) -> bool {
        self.is_done
    }

    pub(crate) fn wants_esc(&self) -> bool {
        self.view.selection.is_some()
    }
}

fn render_offset_content(
    area: Rect,
    buf: &mut Buffer,
    renderable: &dyn Renderable,
    scroll_offset: u16,
) -> u16 {
    let height = renderable.desired_height(area.width);
    let mut tall_buf = Buffer::empty(Rect::new(
        0,
        0,
        area.width,
        height.min(area.height + scroll_offset),
    ));
    renderable.render(*tall_buf.area(), &mut tall_buf);
    let copy_height = area
        .height
        .min(tall_buf.area().height.saturating_sub(scroll_offset));
    for y in 0..copy_height {
        let src_y = y + scroll_offset;
        for x in 0..area.width {
            buf[(area.x + x, area.y + y)] = tall_buf[(x, src_y)].clone();
        }
    }

    copy_height
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history_cell::ReviewDecision;
    use insta::assert_snapshot;
    use ody_app_server_protocol::CommandExecutionSource as ExecCommandSource;
    use pretty_assertions::assert_eq;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;

    use crate::diff_model::FileChange;
    use crate::exec_cell::CommandOutput;
    use crate::history_cell;
    use crate::history_cell::HistoryCell;
    use crate::history_cell::new_patch_event;
    use ody_protocol::parse_command::ParsedCommand;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::text::Text;

    #[derive(Debug)]
    struct TestCell {
        lines: Vec<Line<'static>>,
    }

    impl crate::history_cell::HistoryCell for TestCell {
        fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
            self.lines.clone()
        }

        fn raw_lines(&self) -> Vec<Line<'static>> {
            self.lines.clone()
        }

        fn transcript_lines(&self, _width: u16) -> Vec<Line<'static>> {
            self.lines.clone()
        }
    }

    fn paragraph_block(label: &str, lines: usize) -> Box<dyn Renderable> {
        let text = Text::from(
            (0..lines)
                .map(|i| Line::from(format!("{label}{i}")))
                .collect::<Vec<_>>(),
        );
        Box::new(Paragraph::new(text)) as Box<dyn Renderable>
    }

    fn default_pager_keymap() -> crate::keymap::PagerKeymap {
        crate::keymap::RuntimeKeymap::defaults().pager
    }

    fn transcript_overlay(cells: Vec<Arc<dyn HistoryCell>>) -> TranscriptOverlay {
        TranscriptOverlay::new(
            cells,
            default_pager_keymap(),
            /*mouse_scroll_speed*/ 1.0,
        )
    }

    fn static_overlay(lines: Vec<Line<'static>>, title: &str) -> StaticOverlay {
        StaticOverlay::with_title(lines, title.to_string(), default_pager_keymap())
    }

    fn pager_view(
        renderables: Vec<Box<dyn Renderable>>,
        title: &str,
        scroll_offset: usize,
    ) -> PagerView {
        PagerView::new(
            renderables,
            title.to_string(),
            scroll_offset,
            default_pager_keymap(),
        )
    }

    #[test]
    fn edit_prev_hint_is_visible() {
        let mut overlay = transcript_overlay(vec![Arc::new(TestCell {
            lines: vec![Line::from("hello")],
        })]);

        // Render into a wide buffer so the footer hints aren't truncated.
        let area = Rect::new(0, 0, 120, 10);
        let mut buf = Buffer::empty(area);
        overlay.render(area, &mut buf);

        let s = buffer_to_text(&buf, area);
        assert!(
            s.contains("edit prev"),
            "expected 'edit prev' hint in overlay footer, got: {s:?}"
        );
    }

    #[test]
    fn edit_next_hint_is_visible_when_highlighted() {
        let mut overlay = transcript_overlay(vec![Arc::new(TestCell {
            lines: vec![Line::from("hello")],
        })]);
        overlay.set_highlight_cell(Some(0));

        // Render into a wide buffer so the footer hints aren't truncated.
        let area = Rect::new(0, 0, 120, 10);
        let mut buf = Buffer::empty(area);
        overlay.render(area, &mut buf);

        let s = buffer_to_text(&buf, area);
        assert!(
            s.contains("edit next"),
            "expected 'edit next' hint in overlay footer, got: {s:?}"
        );
    }

    #[test]
    fn transcript_overlay_snapshot_basic() {
        // Prepare a transcript overlay with a few lines
        let mut overlay = transcript_overlay(vec![
            Arc::new(TestCell {
                lines: vec![Line::from("alpha")],
            }),
            Arc::new(TestCell {
                lines: vec![Line::from("beta")],
            }),
            Arc::new(TestCell {
                lines: vec![Line::from("gamma")],
            }),
        ]);
        let mut term = Terminal::new(TestBackend::new(40, 10)).expect("term");
        term.draw(|f| overlay.render(f.area(), f.buffer_mut()))
            .expect("draw");
        assert_snapshot!(term.backend());
    }

    #[test]
    fn transcript_overlay_preserves_semantic_web_links() {
        let destination = "https://example.com/a/very/long/path";
        let mut overlay = transcript_overlay(vec![Arc::new(history_cell::AgentMarkdownCell::new(
            destination.to_string(),
            std::path::Path::new("/tmp"),
        ))]);
        let area = Rect::new(
            /*x*/ 0, /*y*/ 0, /*width*/ 24, /*height*/ 10,
        );
        let mut buf = Buffer::empty(area);

        overlay.render(area, &mut buf);

        assert!(area.positions().any(|position| {
            buf[position]
                .symbol()
                .contains(&format!("\x1b]8;;{destination}\x07"))
        }));
    }

    #[test]
    fn transcript_overlay_renders_live_tail() {
        let mut overlay = transcript_overlay(vec![Arc::new(TestCell {
            lines: vec![Line::from("alpha")],
        })]);
        overlay.sync_live_tail(
            /*width*/ 40,
            Some(ActiveCellTranscriptKey {
                revision: 1,
                is_stream_continuation: false,
                animation_tick: None,
            }),
            |_| Some(vec![HyperlinkLine::from("tail")]),
        );

        let mut term = Terminal::new(TestBackend::new(40, 10)).expect("term");
        term.draw(|f| overlay.render(f.area(), f.buffer_mut()))
            .expect("draw");
        assert_snapshot!(term.backend());
    }

    #[test]
    fn transcript_overlay_live_tail_preserves_semantic_web_links() {
        let destination = "https://example.com/a/streamed/path";
        let cell = history_cell::AgentMarkdownCell::new(
            destination.to_string(),
            std::path::Path::new("/tmp"),
        );
        let mut overlay = transcript_overlay(Vec::new());
        let area = Rect::new(
            /*x*/ 0, /*y*/ 0, /*width*/ 24, /*height*/ 10,
        );
        let mut buf = Buffer::empty(area);

        overlay.sync_live_tail(
            area.width,
            Some(ActiveCellTranscriptKey {
                revision: 1,
                is_stream_continuation: false,
                animation_tick: None,
            }),
            |width| Some(cell.transcript_hyperlink_lines(width)),
        );
        overlay.render(area, &mut buf);

        assert!(area.positions().any(|position| {
            buf[position]
                .symbol()
                .contains(&format!("\x1b]8;;{destination}\x07"))
        }));
    }

    #[test]
    fn transcript_overlay_sync_live_tail_is_noop_for_identical_key() {
        let mut overlay = transcript_overlay(vec![Arc::new(TestCell {
            lines: vec![Line::from("alpha")],
        })]);

        let calls = std::cell::Cell::new(0usize);
        let key = ActiveCellTranscriptKey {
            revision: 1,
            is_stream_continuation: false,
            animation_tick: None,
        };

        overlay.sync_live_tail(/*width*/ 40, Some(key), |_| {
            calls.set(calls.get() + 1);
            Some(vec![HyperlinkLine::from("tail")])
        });
        overlay.sync_live_tail(/*width*/ 40, Some(key), |_| {
            calls.set(calls.get() + 1);
            Some(vec![HyperlinkLine::from("tail2")])
        });

        assert_eq!(calls.get(), 1);
    }

    fn buffer_to_text(buf: &Buffer, area: Rect) -> String {
        let mut out = String::new();
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                let symbol = buf[(x, y)].symbol();
                if symbol.is_empty() {
                    out.push(' ');
                } else {
                    out.push(symbol.chars().next().unwrap_or(' '));
                }
            }
            // Trim trailing spaces for stability.
            while out.ends_with(' ') {
                out.pop();
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn transcript_overlay_apply_patch_scroll_vt100_clears_previous_page() {
        let cwd = PathBuf::from("/repo");
        let mut cells: Vec<Arc<dyn HistoryCell>> = Vec::new();

        let mut approval_changes = HashMap::new();
        approval_changes.insert(
            PathBuf::from("foo.txt"),
            FileChange::Add {
                content: "hello\nworld\n".to_string(),
            },
        );
        let approval_cell: Arc<dyn HistoryCell> = Arc::new(new_patch_event(approval_changes, &cwd));
        cells.push(approval_cell);

        let mut apply_changes = HashMap::new();
        apply_changes.insert(
            PathBuf::from("foo.txt"),
            FileChange::Add {
                content: "hello\nworld\n".to_string(),
            },
        );
        let apply_begin_cell: Arc<dyn HistoryCell> = Arc::new(new_patch_event(apply_changes, &cwd));
        cells.push(apply_begin_cell);

        let apply_end_cell: Arc<dyn HistoryCell> = history_cell::new_approval_decision_cell(
            history_cell::ApprovalDecisionSubject::Command(vec!["ls".into()]),
            ReviewDecision::Approved,
            history_cell::ApprovalDecisionActor::User,
        )
        .into();
        cells.push(apply_end_cell);

        let mut exec_cell = crate::exec_cell::new_active_exec_command(
            "exec-1".into(),
            vec!["bash".into(), "-lc".into(), "ls".into()],
            vec![ParsedCommand::Unknown { cmd: "ls".into() }],
            ExecCommandSource::Agent,
            /*interaction_input*/ None,
            /*animations_enabled*/ true,
        );
        exec_cell.complete_call(
            "exec-1",
            CommandOutput {
                exit_code: 0,
                aggregated_output: "src\nREADME.md\n".into(),
                formatted_output: "src\nREADME.md\n".into(),
            },
            Duration::from_millis(420),
        );
        let exec_cell: Arc<dyn HistoryCell> = Arc::new(exec_cell);
        cells.push(exec_cell);

        let mut overlay = transcript_overlay(cells);
        let area = Rect::new(0, 0, 80, 12);
        let mut buf = Buffer::empty(area);

        overlay.render(area, &mut buf);
        overlay.view.scroll_offset = 0;
        overlay.render(area, &mut buf);

        let snapshot = buffer_to_text(&buf, area);
        assert_snapshot!("transcript_overlay_apply_patch_scroll_vt100", snapshot);
    }

    #[test]
    fn transcript_overlay_keeps_scroll_pinned_at_bottom() {
        let mut overlay = transcript_overlay(
            (0..20)
                .map(|i| {
                    Arc::new(TestCell {
                        lines: vec![Line::from(format!("line{i}"))],
                    }) as Arc<dyn HistoryCell>
                })
                .collect(),
        );
        let mut term = Terminal::new(TestBackend::new(40, 12)).expect("term");
        term.draw(|f| overlay.render(f.area(), f.buffer_mut()))
            .expect("draw");

        assert!(
            overlay.view.is_scrolled_to_bottom(),
            "expected initial render to leave view at bottom"
        );

        overlay.insert_cell(Arc::new(TestCell {
            lines: vec!["tail".into()],
        }));

        assert_eq!(overlay.view.scroll_offset, usize::MAX);
    }

    #[test]
    fn transcript_overlay_preserves_manual_scroll_position() {
        let mut overlay = transcript_overlay(
            (0..20)
                .map(|i| {
                    Arc::new(TestCell {
                        lines: vec![Line::from(format!("line{i}"))],
                    }) as Arc<dyn HistoryCell>
                })
                .collect(),
        );
        let mut term = Terminal::new(TestBackend::new(40, 12)).expect("term");
        term.draw(|f| overlay.render(f.area(), f.buffer_mut()))
            .expect("draw");

        overlay.view.scroll_offset = 0;

        overlay.insert_cell(Arc::new(TestCell {
            lines: vec!["tail".into()],
        }));

        assert_eq!(overlay.view.scroll_offset, 0);
    }

    #[test]
    fn transcript_overlay_consolidation_remaps_highlight_inside_range() {
        let mut overlay = transcript_overlay(
            (0..6)
                .map(|i| {
                    Arc::new(TestCell {
                        lines: vec![Line::from(format!("line{i}"))],
                    }) as Arc<dyn HistoryCell>
                })
                .collect(),
        );
        overlay.set_highlight_cell(Some(3));

        overlay.consolidate_cells(
            2..5,
            Arc::new(TestCell {
                lines: vec![Line::from("consolidated")],
            }),
        );

        assert_eq!(
            overlay.highlight_cell,
            Some(2),
            "highlight inside consolidated range should point to replacement cell",
        );
    }

    #[test]
    fn transcript_overlay_consolidation_remaps_highlight_after_range() {
        let mut overlay = transcript_overlay(
            (0..7)
                .map(|i| {
                    Arc::new(TestCell {
                        lines: vec![Line::from(format!("line{i}"))],
                    }) as Arc<dyn HistoryCell>
                })
                .collect(),
        );
        overlay.set_highlight_cell(Some(6));

        overlay.consolidate_cells(
            2..5,
            Arc::new(TestCell {
                lines: vec![Line::from("consolidated")],
            }),
        );

        assert_eq!(
            overlay.highlight_cell,
            Some(4),
            "highlight after consolidated range should shift left by removed cells",
        );
    }

    #[test]
    fn static_overlay_snapshot_basic() {
        // Prepare a static overlay with a few lines and a title
        let mut overlay = static_overlay(
            vec!["one".into(), "two".into(), "three".into()],
            "S T A T I C",
        );
        let mut term = Terminal::new(TestBackend::new(40, 10)).expect("term");
        term.draw(|f| overlay.render(f.area(), f.buffer_mut()))
            .expect("draw");
        assert_snapshot!(term.backend());
    }

    /// Render transcript overlay and return visible line numbers (`line-NN`) in order.
    fn transcript_line_numbers(overlay: &mut TranscriptOverlay, area: Rect) -> Vec<usize> {
        let mut buf = Buffer::empty(area);
        overlay.render(area, &mut buf);

        let top_h = area.height.saturating_sub(3);
        let top = Rect::new(area.x, area.y, area.width, top_h);
        let content_area = overlay.view.content_area(top);

        let mut nums = Vec::new();
        for y in content_area.y..content_area.bottom() {
            let mut line = String::new();
            for x in content_area.x..content_area.right() {
                line.push(buf[(x, y)].symbol().chars().next().unwrap_or(' '));
            }
            if let Some(n) = line
                .split_whitespace()
                .find_map(|w| w.strip_prefix("line-"))
                .and_then(|s| s.parse().ok())
            {
                nums.push(n);
            }
        }
        nums
    }

    #[test]
    fn transcript_overlay_paging_is_continuous_and_round_trips() {
        let mut overlay = transcript_overlay(
            (0..50)
                .map(|i| {
                    Arc::new(TestCell {
                        lines: vec![Line::from(format!("line-{i:02}"))],
                    }) as Arc<dyn HistoryCell>
                })
                .collect(),
        );
        let area = Rect::new(0, 0, 40, 15);

        // Prime layout so last_content_height is populated and paging uses the real content height.
        let mut buf = Buffer::empty(area);
        overlay.view.scroll_offset = 0;
        overlay.render(area, &mut buf);
        let page_height = overlay.view.page_height(area);

        // Scenario 1: starting from the top, PageDown should show the next page of content.
        overlay.view.scroll_offset = 0;
        let page1 = transcript_line_numbers(&mut overlay, area);
        let page1_len = page1.len();
        let expected_page1: Vec<usize> = (0..page1_len).collect();
        assert_eq!(
            page1, expected_page1,
            "first page should start at line-00 and show a full page of content"
        );

        overlay.view.scroll_offset = overlay.view.scroll_offset.saturating_add(page_height);
        let page2 = transcript_line_numbers(&mut overlay, area);
        assert_eq!(
            page2.len(),
            page1_len,
            "second page should have the same number of visible lines as the first page"
        );
        let expected_page2_first = *page1.last().unwrap() + 1;
        assert_eq!(
            page2[0], expected_page2_first,
            "second page after PageDown should immediately follow the first page"
        );

        // Scenario 2: from an interior offset (start=3), PageDown then PageUp should round-trip.
        let interior_offset = 3usize;
        overlay.view.scroll_offset = interior_offset;
        let before = transcript_line_numbers(&mut overlay, area);
        overlay.view.scroll_offset = overlay.view.scroll_offset.saturating_add(page_height);
        let _ = transcript_line_numbers(&mut overlay, area);
        overlay.view.scroll_offset = overlay.view.scroll_offset.saturating_sub(page_height);
        let after = transcript_line_numbers(&mut overlay, area);
        assert_eq!(
            before, after,
            "PageDown+PageUp from interior offset ({interior_offset}) should round-trip"
        );

        // Scenario 3: from the top of the second page, PageUp then PageDown should round-trip.
        overlay.view.scroll_offset = page_height;
        let before2 = transcript_line_numbers(&mut overlay, area);
        overlay.view.scroll_offset = overlay.view.scroll_offset.saturating_sub(page_height);
        let _ = transcript_line_numbers(&mut overlay, area);
        overlay.view.scroll_offset = overlay.view.scroll_offset.saturating_add(page_height);
        let after2 = transcript_line_numbers(&mut overlay, area);
        assert_eq!(
            before2, after2,
            "PageUp+PageDown from the top of the second page should round-trip"
        );
    }

    #[test]
    fn static_overlay_wraps_long_lines() {
        let mut overlay = static_overlay(
            vec!["a very long line that should wrap when rendered within a narrow pager overlay width".into()],
            "S T A T I C",
        );
        let mut term = Terminal::new(TestBackend::new(24, 8)).expect("term");
        term.draw(|f| overlay.render(f.area(), f.buffer_mut()))
            .expect("draw");
        assert_snapshot!(term.backend());
    }

    #[test]
    fn pager_view_content_height_counts_renderables() {
        let pv = pager_view(
            vec![
                paragraph_block("a", /*lines*/ 2),
                paragraph_block("b", /*lines*/ 3),
            ],
            "T",
            /*scroll_offset*/ 0,
        );

        assert_eq!(pv.content_height(/*width*/ 80), 5);
    }

    #[test]
    fn pager_view_ensure_chunk_visible_scrolls_down_when_needed() {
        let mut pv = pager_view(
            vec![
                paragraph_block("a", /*lines*/ 1),
                paragraph_block("b", /*lines*/ 3),
                paragraph_block("c", /*lines*/ 3),
            ],
            "T",
            /*scroll_offset*/ 0,
        );
        let area = Rect::new(0, 0, 20, 8);

        pv.scroll_offset = 0;
        let content_area = pv.content_area(area);
        pv.ensure_chunk_visible(/*idx*/ 2, content_area);

        let mut buf = Buffer::empty(area);
        pv.render(area, &mut buf);
        let rendered = buffer_to_text(&buf, area);

        assert!(
            rendered.contains("c0"),
            "expected chunk top in view: {rendered:?}"
        );
        assert!(
            rendered.contains("c1"),
            "expected chunk middle in view: {rendered:?}"
        );
        assert!(
            rendered.contains("c2"),
            "expected chunk bottom in view: {rendered:?}"
        );
    }

    #[test]
    fn pager_view_ensure_chunk_visible_scrolls_up_when_needed() {
        let mut pv = pager_view(
            vec![
                paragraph_block("a", /*lines*/ 2),
                paragraph_block("b", /*lines*/ 3),
                paragraph_block("c", /*lines*/ 3),
            ],
            "T",
            /*scroll_offset*/ 0,
        );
        let area = Rect::new(0, 0, 20, 3);

        pv.scroll_offset = 6;
        pv.ensure_chunk_visible(/*idx*/ 0, area);

        assert_eq!(pv.scroll_offset, 0);
    }

    #[test]
    fn pager_view_is_scrolled_to_bottom_accounts_for_wrapped_height() {
        let mut pv = pager_view(
            vec![paragraph_block("a", /*lines*/ 10)],
            "T",
            /*scroll_offset*/ 0,
        );
        let area = Rect::new(0, 0, 20, 8);
        let mut buf = Buffer::empty(area);

        pv.render(area, &mut buf);

        assert!(
            !pv.is_scrolled_to_bottom(),
            "expected view to report not at bottom when offset < max"
        );

        pv.scroll_offset = usize::MAX;
        pv.render(area, &mut buf);

        assert!(
            pv.is_scrolled_to_bottom(),
            "expected view to report at bottom after scrolling to end"
        );
    }

    #[test]
    fn wheel_scroll_scales_rows_with_mouse_scroll_speed() {
        let mut pv = pager_view(
            vec![paragraph_block("a", /*lines*/ 10)],
            "T",
            /*scroll_offset*/ 0,
        );
        pv.mouse_scroll_speed = 2.0;

        pv.apply_wheel_scroll(1.0);
        assert_eq!(pv.scroll_offset, 2);
        pv.apply_wheel_scroll(1.0);
        assert_eq!(pv.scroll_offset, 4);
        pv.apply_wheel_scroll(-1.0);
        assert_eq!(pv.scroll_offset, 2);
    }

    #[test]
    fn fractional_wheel_scroll_accumulates_until_whole_row() {
        let mut pv = pager_view(
            vec![paragraph_block("a", /*lines*/ 10)],
            "T",
            /*scroll_offset*/ 0,
        );
        pv.mouse_scroll_speed = 0.5;

        pv.apply_wheel_scroll(1.0);
        assert_eq!(pv.scroll_offset, 0);
        pv.apply_wheel_scroll(1.0);
        assert_eq!(pv.scroll_offset, 1);
        pv.apply_wheel_scroll(1.0);
        assert_eq!(pv.scroll_offset, 1);
        pv.apply_wheel_scroll(1.0);
        assert_eq!(pv.scroll_offset, 2);
    }

    #[test]
    fn wheel_direction_change_discards_fractional_remainder() {
        let mut pv = pager_view(
            vec![paragraph_block("a", /*lines*/ 10)],
            "T",
            /*scroll_offset*/ 0,
        );
        pv.mouse_scroll_speed = 0.5;

        pv.apply_wheel_scroll(1.0);
        pv.apply_wheel_scroll(-1.0);
        assert_eq!(pv.scroll_offset, 0);
        pv.apply_wheel_scroll(-1.0);
        assert_eq!(pv.scroll_offset, 0);

        pv.scroll_offset = 5;
        pv.apply_wheel_scroll(1.0);
        assert_eq!(pv.scroll_offset, 5);
        pv.apply_wheel_scroll(1.0);
        assert_eq!(pv.scroll_offset, 6);
    }

    use crossterm::event::KeyEventKind;
    use crossterm::event::KeyModifiers;
    use crossterm::event::MouseButton;
    use ratatui::style::Modifier;

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    /// Render once so the view records its content area, then drag the left
    /// button from (`from_col`, `from_row`) to (`to_col`, `to_row`) in screen
    /// coordinates and release.
    fn drag_select(pv: &mut PagerView, area: Rect, from: (u16, u16), to: (u16, u16)) {
        let mut buf = Buffer::empty(area);
        pv.render(area, &mut buf);
        pv.handle_mouse_event(mouse(
            MouseEventKind::Down(MouseButton::Left),
            from.0,
            from.1,
        ));
        pv.handle_mouse_event(mouse(MouseEventKind::Drag(MouseButton::Left), to.0, to.1));
        pv.handle_mouse_event(mouse(MouseEventKind::Up(MouseButton::Left), to.0, to.1));
    }

    #[test]
    fn mouse_drag_selects_and_highlights_reversed() {
        let mut pv = pager_view(
            vec![paragraph_block("a", /*lines*/ 4)],
            "T",
            /*scroll_offset*/ 0,
        );
        // Content area starts at y=1 (below the header). Select from content
        // (row 0, col 0) through (row 1, col 1).
        let area = Rect::new(0, 0, 20, 10);
        drag_select(&mut pv, area, /*from*/ (0, 1), /*to*/ (1, 2));

        let mut buf = Buffer::empty(area);
        pv.render(area, &mut buf);

        // First selected row: from the anchor column to the end of the row.
        assert!(buf[(0, 1)].modifier.contains(Modifier::REVERSED));
        assert!(buf[(1, 1)].modifier.contains(Modifier::REVERSED));
        // Last selected row: columns up to and including the pointer column.
        assert!(buf[(0, 2)].modifier.contains(Modifier::REVERSED));
        assert!(buf[(1, 2)].modifier.contains(Modifier::REVERSED));
        assert!(!buf[(2, 2)].modifier.contains(Modifier::REVERSED));
        // Rows past the selection are untouched.
        assert!(!buf[(0, 3)].modifier.contains(Modifier::REVERSED));
    }

    #[test]
    fn mouse_selection_extracts_text_across_blocks() {
        let mut pv = pager_view(
            vec![paragraph_block("a", 2), paragraph_block("b", 2)],
            "T",
            /*scroll_offset*/ 0,
        );
        let area = Rect::new(0, 0, 20, 10);
        // Anchor on "a0" (content row 0), drag down to "b0" (content row 2).
        drag_select(&mut pv, area, /*from*/ (0, 1), /*to*/ (1, 3));

        assert_eq!(pv.selected_text().as_deref(), Some("a0\na1\nb0"));
    }

    #[test]
    fn mouse_selection_respects_scroll_offset() {
        let mut pv = pager_view(
            vec![paragraph_block("a", /*lines*/ 10)],
            "T",
            /*scroll_offset*/ 3,
        );
        let area = Rect::new(0, 0, 20, 6);
        // Screen row 1 shows content row 3 ("a3") with scroll_offset = 3.
        drag_select(&mut pv, area, /*from*/ (0, 1), /*to*/ (1, 2));

        assert_eq!(pv.selected_text().as_deref(), Some("a3\na4"));
    }

    #[test]
    fn right_click_with_active_selection_queues_copy() {
        let mut pv = pager_view(
            vec![paragraph_block("a", 2), paragraph_block("b", 2)],
            "T",
            /*scroll_offset*/ 0,
        );
        let area = Rect::new(0, 0, 20, 10);
        drag_select(&mut pv, area, /*from*/ (0, 1), /*to*/ (1, 3));

        assert!(pv.handle_mouse_event(mouse(
            MouseEventKind::Down(MouseButton::Right),
            /*column*/ 0,
            /*row*/ 1,
        )));
        assert_eq!(pv.take_pending_copy().as_deref(), Some("a0\na1\nb0"));
        assert!(pv.take_pending_copy().is_none());
    }

    #[test]
    fn right_click_without_selection_does_not_queue_copy() {
        let mut pv = pager_view(
            vec![paragraph_block("a", /*lines*/ 2)],
            "T",
            /*scroll_offset*/ 0,
        );
        let area = Rect::new(0, 0, 20, 10);
        let mut buf = Buffer::empty(area);
        pv.render(area, &mut buf);

        pv.handle_mouse_event(mouse(
            MouseEventKind::Down(MouseButton::Right),
            /*column*/ 0,
            /*row*/ 1,
        ));
        assert!(pv.take_pending_copy().is_none());
    }

    #[test]
    fn click_without_drag_on_link_queues_link_open() {
        let destination = "https://example.com/some/path";
        let mut pv = pager_view(
            vec![Box::new(crate::terminal_hyperlinks::HyperlinkText::new(
                vec![Line::from(format!("visit {destination} now"))],
            ))],
            "T",
            /*scroll_offset*/ 0,
        );
        let area = Rect::new(0, 0, 60, 10);
        let mut buf = Buffer::empty(area);
        pv.render(area, &mut buf);

        // "visit " is 6 columns wide; the URL starts at column 6 of row 1.
        let url_col = 6u16;
        assert!(buf[(url_col, 1)].symbol().contains("\x1b]8;;"));
        pv.handle_mouse_event(mouse(
            MouseEventKind::Down(MouseButton::Left),
            url_col,
            /*row*/ 1,
        ));
        pv.handle_mouse_event(mouse(MouseEventKind::Up(MouseButton::Left), url_col, 1));

        assert_eq!(pv.take_pending_link_open().as_deref(), Some(destination));
        // A plain click does not leave a selection behind.
        assert!(pv.selection.is_none());
        assert!(pv.take_pending_copy().is_none());
    }

    #[test]
    fn click_without_drag_off_link_clears_selection() {
        let mut pv = pager_view(
            vec![paragraph_block("a", /*lines*/ 4)],
            "T",
            /*scroll_offset*/ 0,
        );
        let area = Rect::new(0, 0, 20, 10);
        drag_select(&mut pv, area, /*from*/ (0, 1), /*to*/ (1, 2));
        assert!(pv.selection.is_some());

        pv.handle_mouse_event(mouse(
            MouseEventKind::Down(MouseButton::Left),
            /*column*/ 0,
            /*row*/ 4,
        ));
        pv.handle_mouse_event(mouse(MouseEventKind::Up(MouseButton::Left), 0, 4));

        assert!(pv.selection.is_none());
        assert!(pv.take_pending_link_open().is_none());
    }

    #[test]
    fn is_copy_key_matches_expected_chords() {
        assert!(is_copy_key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL
        )));
        assert!(is_copy_key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT
        )));
        assert!(is_copy_key(KeyEvent::new(
            KeyCode::Char('C'),
            KeyModifiers::CONTROL
        )));
        assert!(is_copy_key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::SUPER
        )));
        assert!(is_copy_key(KeyEvent::new(
            KeyCode::Insert,
            KeyModifiers::CONTROL
        )));

        assert!(!is_copy_key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::NONE
        )));
        assert!(!is_copy_key(KeyEvent::new(
            KeyCode::Insert,
            KeyModifiers::NONE
        )));
        assert!(!is_copy_key(KeyEvent::new_with_kind(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
            KeyEventKind::Release,
        )));
    }

    #[test]
    fn queue_copy_selection_moves_selection_to_pending_copy() {
        let mut pv = pager_view(
            vec![paragraph_block("a", 2), paragraph_block("b", 2)],
            "T",
            /*scroll_offset*/ 0,
        );
        let area = Rect::new(0, 0, 20, 10);
        drag_select(&mut pv, area, /*from*/ (0, 1), /*to*/ (1, 3));

        assert!(pv.queue_copy_selection());
        assert_eq!(pv.take_pending_copy().as_deref(), Some("a0\na1\nb0"));
        assert!(pv.selection.is_none());
        // Nothing left to copy once the selection was consumed.
        assert!(!pv.queue_copy_selection());
    }

    #[test]
    fn esc_clears_selection_before_anything_else() {
        let mut pv = pager_view(
            vec![paragraph_block("a", /*lines*/ 4)],
            "T",
            /*scroll_offset*/ 0,
        );
        let area = Rect::new(0, 0, 20, 10);
        drag_select(&mut pv, area, /*from*/ (0, 1), /*to*/ (1, 2));
        assert!(pv.selection.is_some());

        assert!(pv.clear_selection());
        assert!(pv.selection.is_none());
        // Clearing again reports there was nothing to clear.
        assert!(!pv.clear_selection());
    }

    fn plain_key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn user_cell(message: &str) -> Arc<dyn HistoryCell> {
        Arc::new(history_cell::UserHistoryCell {
            message: message.to_string(),
            text_elements: Vec::new(),
            local_image_paths: Vec::new(),
            remote_image_urls: Vec::new(),
        })
    }

    fn agent_cell(source: &str) -> Arc<dyn HistoryCell> {
        Arc::new(history_cell::AgentMarkdownCell::new(
            source.to_string(),
            std::path::Path::new("/tmp"),
        ))
    }

    fn copy_mode_state(overlay: &TranscriptOverlay) -> &CopyModeState {
        overlay.copy_mode.as_ref().expect("copy mode active")
    }

    fn selected_kind(overlay: &TranscriptOverlay) -> copy_targets::CopyTargetKind {
        let state = copy_mode_state(overlay);
        state.targets[state.selected].kind
    }

    #[test]
    fn enter_copy_mode_selects_whole_response_of_latest_agent_cell() {
        let mut overlay = transcript_overlay(vec![
            user_cell("first question"),
            agent_cell("agent answer"),
            user_cell("follow-up question"),
        ]);

        overlay.enter_copy_mode();

        assert_eq!(
            selected_kind(&overlay),
            copy_targets::CopyTargetKind::WholeResponse
        );
        assert_eq!(
            copy_mode_state(&overlay).targets[copy_mode_state(&overlay).selected].cell_index,
            1
        );
        assert_eq!(overlay.highlight_cell, Some(1));
        // Copy mode keeps Esc for itself instead of starting backtrack preview.
        assert!(overlay.wants_esc());
    }

    #[test]
    fn enter_copy_mode_without_source_cells_is_noop() {
        let mut overlay = transcript_overlay(vec![
            Arc::new(TestCell {
                lines: vec![Line::from("alpha")],
            }),
            Arc::new(TestCell {
                lines: vec![Line::from("beta")],
            }),
        ]);

        overlay.enter_copy_mode();

        assert!(overlay.copy_mode.is_none());
        assert_eq!(overlay.highlight_cell, None);
        assert!(!overlay.wants_esc());
    }

    #[test]
    fn enter_copy_mode_without_agent_cell_selects_most_recent_target() {
        let mut overlay = transcript_overlay(vec![user_cell("first"), user_cell("second")]);

        overlay.enter_copy_mode();

        let state = copy_mode_state(&overlay);
        assert_eq!(state.selected, state.targets.len() - 1);
        assert_eq!(
            selected_kind(&overlay),
            copy_targets::CopyTargetKind::UserMessage
        );
        assert_eq!(overlay.highlight_cell, Some(1));
    }

    #[test]
    fn enter_copy_mode_with_empty_transcript_is_noop() {
        let mut overlay = transcript_overlay(Vec::new());

        overlay.enter_copy_mode();

        assert!(overlay.copy_mode.is_none());
        assert_eq!(overlay.highlight_cell, None);
        assert!(!overlay.wants_esc());
    }

    #[test]
    fn copy_mode_navigation_walks_semantic_targets_with_clamping() {
        let mut overlay = transcript_overlay(vec![
            user_cell("question"),
            agent_cell("answer\n\n```\ncode line\n```\n"),
        ]);
        overlay.enter_copy_mode();
        // Targets: [UserMessage(cell 0), WholeResponse(cell 1), CodeBlock(cell 1)].
        assert_eq!(copy_mode_state(&overlay).selected, 1);

        assert!(overlay.copy_mode_handle_key(plain_key(KeyCode::Down)));
        assert_eq!(
            selected_kind(&overlay),
            copy_targets::CopyTargetKind::CodeBlock
        );
        assert_eq!(overlay.highlight_cell, Some(1));
        // Clamp at the newest target.
        assert!(overlay.copy_mode_handle_key(plain_key(KeyCode::Char('j'))));
        assert_eq!(copy_mode_state(&overlay).selected, 2);

        assert!(overlay.copy_mode_handle_key(plain_key(KeyCode::Up)));
        assert_eq!(copy_mode_state(&overlay).selected, 1);
        assert!(overlay.copy_mode_handle_key(plain_key(KeyCode::Char('k'))));
        assert_eq!(
            selected_kind(&overlay),
            copy_targets::CopyTargetKind::UserMessage
        );
        assert_eq!(overlay.highlight_cell, Some(0));
        // Clamp at the oldest target.
        assert!(overlay.copy_mode_handle_key(plain_key(KeyCode::Up)));
        assert_eq!(copy_mode_state(&overlay).selected, 0);
    }

    #[test]
    fn copy_mode_home_end_and_g_keys_jump_to_edges() {
        let mut overlay = transcript_overlay(vec![
            user_cell("question"),
            agent_cell("answer\n\n```\ncode line\n```\n"),
        ]);
        overlay.enter_copy_mode();

        assert!(overlay.copy_mode_handle_key(plain_key(KeyCode::Char('g'))));
        assert_eq!(copy_mode_state(&overlay).selected, 0);
        assert!(
            overlay.copy_mode_handle_key(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT))
        );
        assert_eq!(copy_mode_state(&overlay).selected, 2);
        assert!(overlay.copy_mode_handle_key(plain_key(KeyCode::Home)));
        assert_eq!(copy_mode_state(&overlay).selected, 0);
        assert!(overlay.copy_mode_handle_key(plain_key(KeyCode::End)));
        assert_eq!(copy_mode_state(&overlay).selected, 2);
    }

    #[test]
    fn copy_mode_enter_copies_markdown_source_and_exits() {
        let source = "# Title\n\n```rust\nfn main() {}\n```";
        let mut overlay = transcript_overlay(vec![agent_cell(source)]);
        overlay.enter_copy_mode();

        assert!(overlay.copy_mode_handle_key(plain_key(KeyCode::Enter)));

        // The whole-response copy is the untouched Markdown source: no
        // rendered bullet prefix and no width-dependent rewrapping.
        assert_eq!(overlay.view.take_pending_copy().as_deref(), Some(source));
        assert!(overlay.copy_mode.is_none());
        assert_eq!(overlay.highlight_cell, None);
        // After copying, the view jumps back to the bottom.
        assert_eq!(overlay.view.scroll_offset, usize::MAX);
        assert_eq!(
            overlay.view.notice.as_deref(),
            Some("Copied whole response to clipboard")
        );
    }

    #[test]
    fn copy_mode_commit_code_block_copies_content_only() {
        let mut overlay =
            transcript_overlay(vec![agent_cell("answer\n\n```rust\nfn main() {}\n```\n")]);
        overlay.enter_copy_mode();
        assert!(overlay.copy_mode_handle_key(plain_key(KeyCode::Down)));
        assert_eq!(
            selected_kind(&overlay),
            copy_targets::CopyTargetKind::CodeBlock
        );

        assert!(overlay.copy_mode_handle_key(plain_key(KeyCode::Enter)));

        // The fence and info string are stripped; only the code is copied,
        // byte-identical to the carried source including its trailing newline.
        assert_eq!(
            overlay.view.take_pending_copy().as_deref(),
            Some("fn main() {}\n")
        );
        assert_eq!(
            overlay.view.notice.as_deref(),
            Some("Copied code block to clipboard")
        );
    }

    #[test]
    fn copy_mode_commit_blockquote_copies_text_without_markers() {
        let mut overlay = transcript_overlay(vec![agent_cell("intro\n\n> line one\n> line two\n")]);
        overlay.enter_copy_mode();
        assert!(overlay.copy_mode_handle_key(plain_key(KeyCode::Down)));
        assert_eq!(
            selected_kind(&overlay),
            copy_targets::CopyTargetKind::Blockquote
        );

        assert!(overlay.copy_mode_handle_key(plain_key(KeyCode::Enter)));

        assert_eq!(
            overlay.view.take_pending_copy().as_deref(),
            Some("line one\nline two\n")
        );
    }

    #[test]
    fn copy_mode_commit_preserves_trailing_whitespace_of_markdown_source() {
        let source = "answer with trailing newline\n\n";
        let mut overlay = transcript_overlay(vec![agent_cell(source)]);
        overlay.enter_copy_mode();

        assert!(overlay.copy_mode_handle_key(plain_key(KeyCode::Enter)));

        // The copy is byte-identical to the carried Markdown source: trailing
        // whitespace is part of the payload and must not be trimmed.
        assert_eq!(overlay.view.take_pending_copy().as_deref(), Some(source));
    }

    #[test]
    fn copy_mode_ctrl_insert_copies_current_target() {
        let mut overlay = transcript_overlay(vec![user_cell("alpha")]);
        overlay.enter_copy_mode();

        assert!(
            overlay.copy_mode_handle_key(KeyEvent::new(KeyCode::Insert, KeyModifiers::CONTROL))
        );
        assert_eq!(overlay.view.take_pending_copy().as_deref(), Some("alpha"));
        assert!(overlay.copy_mode.is_none());
    }

    #[test]
    fn copy_mode_esc_exits_without_copying() {
        let mut overlay = transcript_overlay(vec![user_cell("alpha")]);
        overlay.enter_copy_mode();

        assert!(overlay.copy_mode_handle_key(plain_key(KeyCode::Esc)));

        assert!(overlay.copy_mode.is_none());
        assert_eq!(overlay.highlight_cell, None);
        assert!(overlay.view.take_pending_copy().is_none());
        assert!(!overlay.wants_esc());
    }

    #[test]
    fn copy_mode_ignores_unrelated_keys() {
        let mut overlay = transcript_overlay(vec![user_cell("alpha")]);
        overlay.enter_copy_mode();

        assert!(!overlay.copy_mode_handle_key(plain_key(KeyCode::Char('x'))));
        assert!(overlay.copy_mode.is_some());
    }

    #[test]
    fn copy_mode_hints_show_current_target_label_and_position() {
        let mut overlay = transcript_overlay(vec![
            user_cell("question"),
            agent_cell("answer\n\n```\ncode line\n```\n"),
        ]);
        overlay.enter_copy_mode();
        assert!(overlay.copy_mode_handle_key(plain_key(KeyCode::End)));

        let area = Rect::new(0, 0, 80, 12);
        let mut buf = Buffer::empty(area);
        overlay.render(area, &mut buf);
        let rendered: String = (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect();

        assert!(rendered.contains("Code block"));
        assert!(rendered.contains("3/3"));
    }

    #[test]
    fn copy_mode_highlight_renders_non_user_cell_reversed() {
        let mut overlay = transcript_overlay(vec![
            Arc::new(TestCell {
                lines: vec![Line::from("alpha")],
            }),
            agent_cell("beta"),
        ]);
        overlay.enter_copy_mode();

        let area = Rect::new(0, 0, 40, 12);
        let mut buf = Buffer::empty(area);
        overlay.render(area, &mut buf);

        let find_row = |needle: &str| {
            (0..area.height).find(|&y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .contains(needle)
            })
        };
        let alpha_row = find_row("alpha").expect("alpha row");
        let beta_row = find_row("beta").expect("beta row");
        assert!(!buf[(0, alpha_row)].modifier.contains(Modifier::REVERSED));
        assert!(buf[(0, beta_row)].modifier.contains(Modifier::REVERSED));
    }
}
