//! Key and mouse routing for the owned transcript viewport.
//!
//! The view only answers gestures the rest of the UI does not want. Popups, approval modals and a
//! focused composer are arbitrated by the app before it ever calls in here; what this module owns
//! is navigation, find mode, text selection and link activation.
//!
//! Priority inside the view mirrors the pager overlay: find mode first (it owns typing while the
//! query is being edited), then an active selection, then plain navigation. A selection or a find
//! query therefore keeps working while the composer holds a draft, which is what makes it possible
//! to copy old output without emptying the draft first.

use super::*;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;
use crossterm::event::MouseButton;
use crossterm::event::MouseEvent;
use crossterm::event::MouseEventKind;
use std::time::Duration;
use std::time::Instant;

use crate::key_hint::KeyBindingListExt;
use crate::key_hint::normalize_key_parts;
use crate::keymap::PagerKeymap;

/// Two clicks on the same cell within this window count as a double click.
const DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(400);

/// What the app must do after the view handled an event.
///
/// Clipboard and browser access need services the view cannot reach, so those are returned instead
/// of performed here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ViewAction {
    /// The viewport changed and needs a repaint.
    Changed,
    /// Put this text on the system clipboard.
    Copy(String),
    /// Copy and reattach the viewport to the live tail.
    CopyAndFollow(String),
    /// Open this destination in the browser.
    OpenLink(String),
}

impl TranscriptView {
    /// Whether the view owns this key while it is *not* in find mode.
    ///
    /// These are the gestures that stay with the view even when the composer holds a draft: an
    /// active selection and the find control keys. Navigation keys are gated separately by the app
    /// on an empty draft, because they overlap with typing.
    pub(crate) fn owns_interaction_key(&self, key: KeyEvent) -> bool {
        if key.kind == KeyEventKind::Release {
            return false;
        }
        let (code, modifiers) = normalize_key_parts(key.code, key.modifiers);
        if code == KeyCode::Char(' ') && modifiers == KeyModifiers::CONTROL {
            return true;
        }
        if self.search_active() {
            return matches!(code, KeyCode::Esc)
                || (code == KeyCode::Enter
                    && (self.search_editing() || modifiers == KeyModifiers::NONE))
                || (self.search_editing()
                    && !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    && matches!(code, KeyCode::Char(ch) if !ch.is_ascii_control()))
                || matches!(code, KeyCode::Backspace | KeyCode::Delete)
                || (modifiers == KeyModifiers::NONE
                    && matches!(code, KeyCode::Up | KeyCode::Down))
                || (modifiers == KeyModifiers::CONTROL
                    && matches!(code, KeyCode::Char('c' | 'n' | 'p')));
        }
        if self.selection.is_some() {
            return matches!(code, KeyCode::Esc | KeyCode::Enter)
                || (matches!(modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT)
                    && matches!(
                        code,
                        KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right
                    ))
                || (modifiers == KeyModifiers::NONE
                    && matches!(code, KeyCode::PageUp | KeyCode::PageDown));
        }
        false
    }

    /// Whether the view claims this navigation key once the draft is empty.
    ///
    /// Only keys that cannot be draft text are claimed here, so a configured `j`/`k` binding and
    /// plain space keep typing a space instead of paging.
    pub(crate) fn owns_navigation_key(&self, key: KeyEvent, _pager: &PagerKeymap) -> bool {
        if key.kind == KeyEventKind::Release {
            return false;
        }
        let (code, modifiers) = normalize_key_parts(key.code, key.modifiers);
        match modifiers {
            KeyModifiers::NONE => matches!(
                code,
                KeyCode::Up
                    | KeyCode::Down
                    | KeyCode::PageUp
                    | KeyCode::PageDown
                    | KeyCode::Home
                    | KeyCode::End
                    | KeyCode::Esc
            ),
            KeyModifiers::CONTROL => matches!(code, KeyCode::Char('b' | 'f' | 'u' | 'd')),
            _ => false,
        }
    }

    /// Whether find mode is waiting for another scan step.
    pub(crate) fn interaction_needs_frame(&self) -> bool {
        self.search_needs_work()
    }

    /// Route one key press. Returns `None` only when the key is not a transcript gesture.
    pub(crate) fn handle_key(
        &mut self,
        key: KeyEvent,
        cells: &[Arc<dyn HistoryCell>],
        pager: &PagerKeymap,
    ) -> Option<ViewAction> {
        if key.kind == KeyEventKind::Release {
            return None;
        }
        // Feedback from the previous action retires with the next transcript gesture.
        self.notice = None;
        let (code, modifiers) = normalize_key_parts(key.code, key.modifiers);

        // Find mode owns typing, so anything it does not act on is still consumed here rather than
        // leaking into the composer.
        if self.search_active() {
            return Some(self.handle_search_key(code, modifiers, cells));
        }
        if self.selection.is_some() {
            if let Some(action) = self.handle_selection_key(code, modifiers, cells) {
                return Some(action);
            }
            // Any other key drops the selection; the key itself keeps its normal meaning.
            self.clear_selection();
        }
        if modifiers == KeyModifiers::CONTROL && code == KeyCode::Char(' ') {
            self.begin_selection(cells, self.area.x, self.area.y, /*clicks*/ 1);
            return Some(ViewAction::Changed);
        }
        self.handle_scroll_key(code, modifiers, cells, pager)
    }

    /// Route one mouse event. Returns `None` when the event has no transcript meaning.
    pub(crate) fn handle_mouse(
        &mut self,
        event: MouseEvent,
        cells: &[Arc<dyn HistoryCell>],
    ) -> Option<ViewAction> {
        let inside = self.contains(event.column, event.row);
        let dragging = self.is_selecting();
        if !inside && !dragging {
            return None;
        }
        self.notice = None;
        match event.kind {
            MouseEventKind::ScrollUp => {
                self.wheel(cells, /*direction*/ -1.0);
                Some(ViewAction::Changed)
            }
            MouseEventKind::ScrollDown => {
                self.wheel(cells, /*direction*/ 1.0);
                Some(ViewAction::Changed)
            }
            MouseEventKind::Down(MouseButton::Right) if inside => {
                // Explicit copies clear the selection, matching the pager overlay; only an
                // automatic copy-on-select would keep it.
                let text = self.selected_text(cells).filter(|text| !text.is_empty());
                self.clear_selection();
                text.map(ViewAction::Copy)
            }
            MouseEventKind::Down(MouseButton::Left) => {
                // A click on the collapsed-form hint expands or collapses that one cell.
                if event.modifiers.is_empty()
                    && self.toggle_disclosure_at(cells, event.column, event.row)
                {
                    self.clear_selection();
                    return Some(ViewAction::Changed);
                }
                // Ctrl/Cmd-click follows a link without starting a selection.
                if event
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::SUPER)
                {
                    self.clear_selection();
                    return self
                        .link_at(cells, event.column, event.row)
                        .map(ViewAction::OpenLink);
                }
                let clicks = self.click_count(event.column, event.row);
                let link = (clicks == 1 && event.modifiers.is_empty())
                    .then(|| self.link_at(cells, event.column, event.row))
                    .flatten();
                self.pressed_link = link;
                if self.begin_selection(cells, event.column, event.row, clicks) {
                    Some(ViewAction::Changed)
                } else {
                    self.pressed_link = None;
                    None
                }
            }
            MouseEventKind::Drag(MouseButton::Left) if dragging => {
                self.extend_selection(cells, event.column, event.row);
                Some(ViewAction::Changed)
            }
            MouseEventKind::Up(MouseButton::Left) if dragging => {
                self.finish_selection_drag();
                if self.has_selection() {
                    // Keep the selection so the reader can still copy it; no auto-copy surprises.
                    return Some(ViewAction::Changed);
                }
                // A stationary click activates the link it landed on, if any.
                self.pressed_link.take().map(ViewAction::OpenLink)
            }
            _ => None,
        }
    }

    /// Copy the current selection, clearing it afterwards.
    ///
    /// The app calls this for its own copy binding so `Ctrl+O` copies the selection instead of the
    /// last agent message while a selection is active.
    pub(crate) fn copy_selection(&mut self, cells: &[Arc<dyn HistoryCell>]) -> Option<String> {
        let text = self.selected_text(cells)?;
        if text.is_empty() {
            return None;
        }
        self.clear_selection();
        Some(text)
    }

    fn wheel(&mut self, cells: &[Arc<dyn HistoryCell>], direction: f64) {
        self.scroll_by_wheel(cells, direction);
        // A wheel notch ends the drag, exactly like the pager: the pointer position is stale.
        self.pressed_link = None;
    }

    /// Count consecutive clicks on the same cell.
    fn click_count(&mut self, column: u16, row: u16) -> u8 {
        let now = Instant::now();
        let clicks = match self.last_click {
            Some((last_column, last_row, at))
                if last_column == column
                    && last_row == row
                    && now.duration_since(at) <= DOUBLE_CLICK_WINDOW =>
            {
                2
            }
            _ => 1,
        };
        self.last_click = Some((column, row, now));
        clicks
    }

    fn handle_selection_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
        cells: &[Arc<dyn HistoryCell>],
    ) -> Option<ViewAction> {
        if code == KeyCode::Enter && modifiers == KeyModifiers::NONE {
            let text = self.selected_text(cells).filter(|text| !text.is_empty());
            self.clear_selection();
            return Some(match text {
                Some(text) => ViewAction::CopyAndFollow(text),
                None => ViewAction::Changed,
            });
        }
        if code == KeyCode::Esc {
            self.clear_selection();
            return Some(ViewAction::Changed);
        }
        if !matches!(modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT) {
            return None;
        }
        let rows = match code {
            KeyCode::Up => -1,
            KeyCode::Down => 1,
            KeyCode::PageUp => -self.page_rows(),
            KeyCode::PageDown => self.page_rows(),
            _ => return None,
        };
        self.extend_selection_rows(cells, rows);
        Some(ViewAction::Changed)
    }

    fn handle_search_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
        cells: &[Arc<dyn HistoryCell>],
    ) -> ViewAction {
        match (code, modifiers) {
            (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                self.cancel_search();
            }
            (KeyCode::Enter, KeyModifiers::NONE) => self.accept_search(),
            (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
                self.search_advance(cells, /*newer*/ true);
            }
            (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
                self.search_advance(cells, /*newer*/ false);
            }
            (KeyCode::Backspace | KeyCode::Delete, _) => self.pop_search_char(),
            (KeyCode::Up, KeyModifiers::NONE) => self.scroll(cells, -1),
            (KeyCode::Down, KeyModifiers::NONE) => self.scroll(cells, 1),
            (KeyCode::Char(ch), modifiers)
                if self.search_editing()
                    && !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    && !ch.is_ascii_control() =>
            {
                self.push_search_char(ch);
            }
            _ => {}
        }
        ViewAction::Changed
    }

    fn handle_scroll_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
        cells: &[Arc<dyn HistoryCell>],
        pager: &PagerKeymap,
    ) -> Option<ViewAction> {
        let key = KeyEvent::new(code, modifiers);
        if pager.jump_top.is_pressed(key) {
            self.jump_to_beginning(cells);
            return Some(ViewAction::Changed);
        }
        if pager.jump_bottom.is_pressed(key) {
            self.jump_to_latest();
            return Some(ViewAction::Changed);
        }
        if code == KeyCode::Esc && modifiers == KeyModifiers::NONE {
            // Esc only means "back to the newest content" while reading back; otherwise it belongs
            // to the app's backtrack ladder.
            if self.is_following() {
                return None;
            }
            self.jump_to_latest();
            return Some(ViewAction::Changed);
        }
        // Full and half pages keep the pager's one-row overlap so a jump is readable.
        let page = self.page_rows();
        let half = (page + 1) / 2;
        let delta = [
            (&pager.scroll_up, -1),
            (&pager.scroll_down, 1),
            (&pager.page_up, -page),
            (&pager.page_down, page),
            (&pager.half_page_up, -half),
            (&pager.half_page_down, half),
        ]
        .into_iter()
        .find_map(|(bindings, delta)| bindings.is_pressed(key).then_some(delta));
        let delta = delta?;
        self.scroll(cells, delta);
        Some(ViewAction::Changed)
    }
}
