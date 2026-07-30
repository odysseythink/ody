//! Mode-specific preferences form for the `/preferences` slash command.
//!
//! In Design mode this renders a typed form for the nine `[design_review]` keys.
//! Other modes show a placeholder explaining that no preferences are available yet.

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Instant;

use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use ratatui::buffer::Buffer;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Block;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;

use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::config_update::DesignReviewEditState;
use crate::config_update::build_design_review_edits;
use crate::key_hint;
use crate::key_hint::KeyBindingListExt;
use crate::keymap::ListKeymap;
use crate::render::Insets;
use crate::render::RectExt as _;
use crate::render::renderable::ColumnRenderable;
use crate::render::renderable::Renderable;
use crate::style::user_message_style;

use ody_config::config_toml::DesignReviewToml;
use ody_config::config_toml::UsabilityLensToml;
use ody_protocol::config_types::ModeKind;

use super::CancellationEvent;
use super::bottom_pane_view::BottomPaneView;
use super::bottom_pane_view::ViewCompletion;
use super::custom_prompt_view::CustomPromptView;
use super::popup_consts::MAX_POPUP_ROWS;
use super::popup_consts::standard_popup_hint_line;
use super::scroll_state::ScrollState;
use super::selection_popup_common::GenericDisplayRow;
use super::selection_popup_common::measure_rows_height;
use super::selection_popup_common::render_rows;

const DESIGN_TITLE: &str = "Design review preferences";
const DESIGN_SUBTITLE: &str = "Configure the adversarial review run before finalizing a design.";
const PLACEHOLDER_TITLE: &str = "Preferences";
const PLACEHOLDER_BODY: &str = "Mode-specific preferences are not available for the current mode yet.";
const ROUNDS_ERROR: &str = "rounds must be an integer between 1 and 3";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PreferencesField {
    Enable,
    ReviewModel,
    DebateEnable,
    Rounds,
    AdvocateModel,
    SkepticModel,
    JudgeModel,
    ContestCritic,
    UsabilityLens,
}

impl PreferencesField {
    fn label(self) -> &'static str {
        match self {
            Self::Enable => "Enable design review",
            Self::ReviewModel => "Review model",
            Self::DebateEnable => "Enable debate",
            Self::Rounds => "Debate rounds",
            Self::AdvocateModel => "Advocate model",
            Self::SkepticModel => "Skeptic model",
            Self::JudgeModel => "Judge model",
            Self::ContestCritic => "Contest critic",
            Self::UsabilityLens => "Usability lens",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Enable => "Run an adversarial review before finalizing a design.",
            Self::ReviewModel => "Model override for the single-shot critic (optional).",
            Self::DebateEnable => "Use an Advocate/Skeptic/Judge debate instead of a single review.",
            Self::Rounds => "Advocate↔Skeptic back-and-forth rounds (1–3).",
            Self::AdvocateModel => "Model override for the Advocate seat (optional).",
            Self::SkepticModel => "Model override for the Skeptic seat (optional).",
            Self::JudgeModel => "Model override for the Judge seat (optional).",
            Self::ContestCritic => "Allow the Judge to refute weak critic findings.",
            Self::UsabilityLens => "Append a usability-lens Skeptic turn (off/on/ask).",
        }
    }

    fn is_bool(self) -> bool {
        matches!(self, Self::Enable | Self::DebateEnable | Self::ContestCritic)
    }

    fn is_text(self) -> bool {
        matches!(
            self,
            Self::ReviewModel | Self::AdvocateModel | Self::SkepticModel | Self::JudgeModel
        )
    }

    fn is_rounds(self) -> bool {
        self == Self::Rounds
    }

    fn is_usability_lens(self) -> bool {
        self == Self::UsabilityLens
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PreferencesContent {
    /// Show the design-mode form seeded from the given state.
    Design { state: DesignReviewEditState },
    /// Show a placeholder for modes without specific preferences.
    Placeholder,
}

pub(crate) struct PreferencesView {
    mode: ModeKind,
    edit_state: DesignReviewEditState,
    baseline_state: DesignReviewEditState,
    fields: Vec<PreferencesField>,
    state: ScrollState,
    text_editor: Option<CustomPromptView>,
    text_editor_field: Option<PreferencesField>,
    text_editor_result: Arc<Mutex<Option<String>>>,
    error_message: Option<String>,
    complete: bool,
    app_event_tx: AppEventSender,
    keymap: ListKeymap,
}

impl PreferencesView {
    pub(crate) fn new(
        mode: ModeKind,
        content: PreferencesContent,
        app_event_tx: AppEventSender,
        keymap: ListKeymap,
    ) -> Self {
        let (edit_state, fields) = match content {
            PreferencesContent::Design { state } => {
                let fields = vec![
                    PreferencesField::Enable,
                    PreferencesField::ReviewModel,
                    PreferencesField::DebateEnable,
                    PreferencesField::Rounds,
                    PreferencesField::AdvocateModel,
                    PreferencesField::SkepticModel,
                    PreferencesField::JudgeModel,
                    PreferencesField::ContestCritic,
                    PreferencesField::UsabilityLens,
                ];
                (state.clone(), fields)
            }
            PreferencesContent::Placeholder => (DesignReviewEditState::default(), Vec::new()),
        };
        let mut view = Self {
            mode,
            edit_state: edit_state.clone(),
            baseline_state: edit_state,
            fields,
            state: ScrollState::new(),
            text_editor: None,
            text_editor_field: None,
            text_editor_result: Arc::new(Mutex::new(None)),
            error_message: None,
            complete: false,
            app_event_tx,
            keymap,
        };
        view.initialize_selection();
        view
    }

    fn initialize_selection(&mut self) {
        self.state.selected_idx = if self.fields.is_empty() {
            None
        } else {
            Some(0)
        };
    }

    fn visible_len(&self) -> usize {
        self.fields.len()
    }

    fn build_rows(&self) -> Vec<GenericDisplayRow> {
        let selected_idx = self.state.selected_idx;
        self.fields
            .iter()
            .enumerate()
            .map(|(idx, field)| {
                let prefix = if selected_idx == Some(idx) { '›' } else { ' ' };
                let value = self.format_field_value(*field);
                let name = format!("{prefix} {}: {}", field.label(), value);
                GenericDisplayRow {
                    name,
                    description: Some(field.description().to_string()),
                    ..Default::default()
                }
            })
            .collect()
    }

    fn format_field_value(&self, field: PreferencesField) -> String {
        match field {
            PreferencesField::Enable => format_bool(self.edit_state.enable),
            PreferencesField::ReviewModel => format_opt_str(&self.edit_state.review_model),
            PreferencesField::DebateEnable => format_bool(self.edit_state.debate_enable),
            PreferencesField::Rounds => format_opt_rounds(self.edit_state.rounds),
            PreferencesField::AdvocateModel => format_opt_str(&self.edit_state.advocate_model),
            PreferencesField::SkepticModel => format_opt_str(&self.edit_state.skeptic_model),
            PreferencesField::JudgeModel => format_opt_str(&self.edit_state.judge_model),
            PreferencesField::ContestCritic => format_bool(self.edit_state.contest_critic),
            PreferencesField::UsabilityLens => format_usability_lens(self.edit_state.usability_lens),
        }
    }

    fn move_up(&mut self) {
        let len = self.visible_len();
        if len == 0 {
            return;
        }
        self.state.move_up_wrap(len);
        self.state.ensure_visible(len, MAX_POPUP_ROWS.min(len));
        self.error_message = None;
    }

    fn move_down(&mut self) {
        let len = self.visible_len();
        if len == 0 {
            return;
        }
        self.state.move_down_wrap(len);
        self.state.ensure_visible(len, MAX_POPUP_ROWS.min(len));
        self.error_message = None;
    }

    fn page_up(&mut self) {
        let len = self.visible_len();
        let visible = MAX_POPUP_ROWS.min(len);
        self.state.page_up_clamped(len, visible);
    }

    fn page_down(&mut self) {
        let len = self.visible_len();
        let visible = MAX_POPUP_ROWS.min(len);
        self.state.page_down_clamped(len, visible);
    }

    fn toggle_selected(&mut self) {
        let Some(selected_idx) = self.state.selected_idx else {
            return;
        };
        let Some(&field) = self.fields.get(selected_idx) else {
            return;
        };
        if field.is_bool() {
            self.toggle_bool(field);
        } else if field.is_usability_lens() {
            self.cycle_usability_lens();
        }
    }

    fn toggle_bool(&mut self, field: PreferencesField) {
        match field {
            PreferencesField::Enable => self.edit_state.enable = !self.edit_state.enable,
            PreferencesField::DebateEnable => {
                self.edit_state.debate_enable = !self.edit_state.debate_enable
            }
            PreferencesField::ContestCritic => {
                self.edit_state.contest_critic = !self.edit_state.contest_critic
            }
            _ => return,
        }
        self.send_persist_event();
    }

    fn cycle_usability_lens(&mut self) {
        self.edit_state.usability_lens = match self.edit_state.usability_lens {
            UsabilityLensToml::Off => UsabilityLensToml::On,
            UsabilityLensToml::On => UsabilityLensToml::Ask,
            UsabilityLensToml::Ask => UsabilityLensToml::Off,
        };
        self.send_persist_event();
    }

    fn edit_selected(&mut self) {
        let Some(selected_idx) = self.state.selected_idx else {
            return;
        };
        let Some(&field) = self.fields.get(selected_idx) else {
            return;
        };
        if !field.is_text() && !field.is_rounds() {
            return;
        }

        let initial = if field.is_rounds() {
            self.edit_state
                .rounds
                .map(|r| r.to_string())
                .unwrap_or_default()
        } else {
            self.text_field_value(field).unwrap_or_default()
        };

        let title = format!("Edit {}", field.label());
        let placeholder = if field.is_rounds() {
            "Type 1–3 or leave empty for default"
        } else {
            "Type a model alias or leave empty to clear"
        }
        .to_string();

        let result = self.text_editor_result.clone();
        let result_for_closure = result.clone();
        let editor = CustomPromptView::new(
            title,
            placeholder,
            initial,
            None,
            Box::new(move |text: String| {
                *result_for_closure.lock().unwrap() = Some(text);
            }),
        );
        self.text_editor = Some(editor);
        self.text_editor_field = Some(field);
        self.text_editor_result = result;
        self.error_message = None;
    }

    fn text_field_value(&self, field: PreferencesField) -> Option<String> {
        match field {
            PreferencesField::ReviewModel => self.edit_state.review_model.clone(),
            PreferencesField::AdvocateModel => self.edit_state.advocate_model.clone(),
            PreferencesField::SkepticModel => self.edit_state.skeptic_model.clone(),
            PreferencesField::JudgeModel => self.edit_state.judge_model.clone(),
            _ => None,
        }
    }

    fn apply_text_editor_result(&mut self, text: String) -> bool {
        let Some(field) = self.text_editor_field else {
            return false;
        };
        if field.is_rounds() {
            return self.apply_rounds_text(text);
        }
        self.apply_text_field(field, text)
    }

    fn apply_text_field(&mut self, field: PreferencesField, text: String) -> bool {
        let trimmed = text.trim().to_string();
        let value = if trimmed.is_empty() { None } else { Some(trimmed) };
        match field {
            PreferencesField::ReviewModel => self.edit_state.review_model = value,
            PreferencesField::AdvocateModel => self.edit_state.advocate_model = value,
            PreferencesField::SkepticModel => self.edit_state.skeptic_model = value,
            PreferencesField::JudgeModel => self.edit_state.judge_model = value,
            _ => return false,
        }
        self.send_persist_event();
        true
    }

    fn apply_rounds_text(&mut self, text: String) -> bool {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            self.edit_state.rounds = None;
            self.send_persist_event();
            return true;
        }
        match trimmed.parse::<u8>() {
            Ok(rounds) if (1..=3).contains(&rounds) => {
                self.edit_state.rounds = Some(rounds);
                self.error_message = None;
                self.send_persist_event();
                true
            }
            _ => {
                self.error_message = Some(ROUNDS_ERROR.to_string());
                false
            }
        }
    }

    fn send_persist_event(&mut self) {
        let edits = build_design_review_edits(&self.edit_state);
        self.app_event_tx
            .send(AppEvent::PersistDesignReviewPreferences { edits });
    }

    fn cancel(&mut self) {
        if self.text_editor.is_some() {
            self.text_editor = None;
            self.text_editor_field = None;
            *self.text_editor_result.lock().unwrap() = None;
            return;
        }
        self.complete = true;
    }

    fn design_header(&self) -> ColumnRenderable<'_> {
        let mut header = ColumnRenderable::new();
        header.push(Line::from(DESIGN_TITLE.bold()));
        header.push(Line::from(DESIGN_SUBTITLE.dim()));
        if let Some(error) = &self.error_message {
            header.push(Line::from(Span::styled(format!("Error: {error}"), self.error_style())));
        }
        header
    }

    fn placeholder_header(&self) -> ColumnRenderable<'_> {
        let mut header = ColumnRenderable::new();
        header.push(Line::from(PLACEHOLDER_TITLE.bold()));
        header.push(Line::from(PLACEHOLDER_BODY.dim()));
        header
    }

    fn error_style(&self) -> ratatui::style::Style {
        ratatui::style::Style::default().fg(ratatui::style::Color::Red)
    }

    fn footer_hint(&self) -> Line<'static> {
        if self.text_editor.is_some() {
            standard_popup_hint_line()
        } else if self.fields.is_empty() {
            Line::from(vec![
                "Press ".into(),
                key_hint::plain(KeyCode::Esc).into(),
                " to close".into(),
            ])
        } else {
            Line::from(vec![
                "Press ".into(),
                key_hint::plain(KeyCode::Up).into(),
                "/".into(),
                key_hint::plain(KeyCode::Down).into(),
                " to move; ".into(),
                key_hint::plain(KeyCode::Char(' ')).into(),
                " to toggle; ".into(),
                key_hint::plain(KeyCode::Enter).into(),
                " to edit".into(),
            ])
        }
    }
}

impl BottomPaneView for PreferencesView {
    fn handle_key_event(&mut self, key_event: KeyEvent) {
        if let Some(editor) = &mut self.text_editor {
            editor.handle_key_event(key_event);
            return;
        }

        match key_event {
            _ if self.keymap.move_up.is_pressed(key_event) => self.move_up(),
            _ if self.keymap.move_down.is_pressed(key_event) => self.move_down(),
            _ if self.keymap.page_up.is_pressed(key_event) => self.page_up(),
            _ if self.keymap.page_down.is_pressed(key_event) => self.page_down(),
            KeyEvent {
                code: KeyCode::Char(' '),
                modifiers: KeyModifiers::NONE,
                ..
            } => self.toggle_selected(),
            KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::NONE,
                ..
            } => self.edit_selected(),
            _ if self.keymap.accept.is_pressed(key_event) || self.keymap.cancel.is_pressed(key_event) => {
                self.cancel();
            }
            _ => {}
        }
    }

    fn is_complete(&self) -> bool {
        self.complete
    }

    fn on_ctrl_c(&mut self) -> CancellationEvent {
        self.cancel();
        CancellationEvent::Handled
    }

    fn pre_draw_tick(&mut self, _now: Instant) -> bool {
        if let Some(editor) = &self.text_editor {
            if editor.is_complete() {
                let result = self.text_editor_result.lock().unwrap().take();
                let accepted = editor.completion() == Some(ViewCompletion::Accepted);
                let text = result.unwrap_or_default();
                // Empty submission with an accepted completion means the user
                // intentionally cleared the field; a cancelled completion is ignored.
                if accepted {
                    self.apply_text_editor_result(text);
                }
                self.text_editor = None;
                self.text_editor_field = None;
                return true;
            }
        }
        false
    }

    fn handle_app_event(&mut self, event: &AppEvent) -> bool {
        let AppEvent::SyncDesignReviewPreferences { design_review, error } = event else {
            return false;
        };
        if error.is_some() {
            self.edit_state = self.baseline_state.clone();
            self.error_message = error.clone();
        } else {
            self.edit_state.apply_from_toml(design_review);
            self.baseline_state = self.edit_state.clone();
            self.error_message = None;
        }
        true
    }
}

impl Renderable for PreferencesView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        if let Some(editor) = &self.text_editor {
            editor.render(area, buf);
            return;
        }

        let [content_area, footer_area] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);

        Block::default()
            .style(user_message_style())
            .render(content_area, buf);

        let header = if self.fields.is_empty() {
            self.placeholder_header()
        } else {
            self.design_header()
        };
        let header_height = header.desired_height(content_area.width.saturating_sub(4));
        let rows = self.build_rows();
        let rows_width = Self::rows_width(content_area.width);
        let rows_height = if rows.is_empty() {
            0
        } else {
            measure_rows_height(
                &rows,
                &self.state,
                MAX_POPUP_ROWS,
                rows_width.saturating_add(1),
            )
        };
        let layout = if rows.is_empty() {
            vec![
                Constraint::Max(header_height),
                Constraint::Max(1),
                Constraint::Length(1),
                Constraint::Max(1),
                Constraint::Length(1),
            ]
        } else {
            vec![
                Constraint::Max(header_height),
                Constraint::Max(1),
                Constraint::Length(rows_height),
                Constraint::Max(1),
                Constraint::Length(1),
            ]
        };
        let [header_area, _, list_area, _, docs_area] =
            Layout::vertical(layout).areas(content_area.inset(Insets::vh(/*v*/ 1, /*h*/ 2)));

        header.render(header_area, buf);

        if !rows.is_empty() && list_area.height > 0 {
            let render_area = Rect {
                x: list_area.x.saturating_sub(2),
                y: list_area.y,
                width: rows_width.max(1),
                height: list_area.height,
            };
            render_rows(
                render_area,
                buf,
                &rows,
                &self.state,
                MAX_POPUP_ROWS,
                "  No preferences available",
            );
        }

        let mode_hint = format!("Mode: {}", format_mode(self.mode));
        Paragraph::new(Line::from(mode_hint.dim())).render(docs_area, buf);

        let hint_area = Rect {
            x: footer_area.x + 2,
            y: footer_area.y,
            width: footer_area.width.saturating_sub(2),
            height: footer_area.height,
        };
        self.footer_hint().render(hint_area, buf);
    }

    fn desired_height(&self, width: u16) -> u16 {
        let text_editor_height = self.text_editor.as_ref().map(|e| e.desired_height(width));
        if let Some(height) = text_editor_height {
            return height.saturating_add(2);
        }

        let header = if self.fields.is_empty() {
            self.placeholder_header()
        } else {
            self.design_header()
        };
        let rows = self.build_rows();
        let rows_width = Self::rows_width(width);
        let rows_height = if rows.is_empty() {
            0
        } else {
            measure_rows_height(
                &rows,
                &self.state,
                MAX_POPUP_ROWS,
                rows_width.saturating_add(1),
            )
        };
        let mut height = header.desired_height(width.saturating_sub(4));
        height = height.saturating_add(rows_height.max(1) + 6);
        height
    }

    fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        self.text_editor.as_ref().and_then(|e| e.cursor_pos(area))
    }
}

impl PreferencesView {
    fn rows_width(total_width: u16) -> u16 {
        total_width.saturating_sub(2)
    }
}

fn format_bool(value: bool) -> String {
    if value { "on".to_string() } else { "off".to_string() }
}

fn format_opt_str(value: &Option<String>) -> String {
    value.as_deref().unwrap_or("(none)").to_string()
}

fn format_opt_rounds(value: Option<u8>) -> String {
    value.map(|v| v.to_string()).unwrap_or_else(|| "default".to_string())
}

fn format_usability_lens(value: UsabilityLensToml) -> String {
    match value {
        UsabilityLensToml::Off => "off",
        UsabilityLensToml::On => "on",
        UsabilityLensToml::Ask => "ask",
    }
    .to_string()
}

fn format_mode(mode: ModeKind) -> &'static str {
    match mode {
        ModeKind::Default => "Default",
        ModeKind::Plan => "Plan",
        ModeKind::Design => "Design",
        _ => "Default",
    }
}

#[cfg(test)]
#[path = "preferences_view_tests.rs"]
mod tests;