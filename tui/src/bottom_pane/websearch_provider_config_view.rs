//! Per-provider configuration form for the `/websearch` slash command.
//!
//! This view lists every editable field reported by [`provider_config_fields`] and
//! lets the user edit each value before saving the complete primary provider config.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
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
use ratatui::widgets::Widget;
use serde_json::Value;

use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::CancellationEvent;
use crate::bottom_pane::bottom_pane_view::BottomPaneView;
use crate::bottom_pane::bottom_pane_view::ViewCompletion;
use crate::bottom_pane::custom_prompt_view::CustomPromptView;
use crate::bottom_pane::popup_consts::MAX_POPUP_ROWS;
use crate::bottom_pane::popup_consts::standard_popup_hint_line;
use crate::bottom_pane::scroll_state::ScrollState;
use crate::bottom_pane::selection_popup_common::GenericDisplayRow;
use crate::bottom_pane::selection_popup_common::measure_rows_height;
use crate::bottom_pane::selection_popup_common::render_rows;
use crate::key_hint::KeyBindingListExt;
use crate::keymap::ListKeymap;
use crate::render::Insets;
use crate::render::RectExt as _;
use crate::render::renderable::ColumnRenderable;
use crate::render::renderable::Renderable;
use crate::style::user_message_style;

use ody_web_search::config::WebSearchProviderConfig;
use ody_web_search::config::WebSearchProviderName;
use ody_web_search::provider_config_fields::ProviderConfigField;
use ody_web_search::provider_config_fields::ProviderConfigFieldKind;
use ody_web_search::provider_config_fields::provider_config_fields;

const SAVE_KEY: &str = "__save__";
const SAVE_LABEL: &str = "Save configuration";
const SAVE_DESCRIPTION: &str = "Write the updated provider configuration to config.toml.";

/// Editable form for one web-search provider.
pub(crate) struct WebSearchProviderConfigView {
    provider: WebSearchProviderName,
    fields: Vec<ProviderConfigField>,
    values: HashMap<String, String>,
    error_message: Option<String>,

    scroll_state: ScrollState,
    text_editor: Option<CustomPromptView>,
    text_editor_field: Option<String>,
    text_editor_result: Arc<Mutex<Option<String>>>,
    complete: bool,

    app_event_tx: AppEventSender,
    keymap: ListKeymap,
}

impl WebSearchProviderConfigView {
    pub(crate) fn new(
        provider: WebSearchProviderName,
        current: Option<&WebSearchProviderConfig>,
        app_event_tx: AppEventSender,
        keymap: ListKeymap,
    ) -> Self {
        let fields = provider_config_fields(provider);
        let values = Self::initial_values(provider, current, &fields);
        let mut view = Self {
            provider,
            fields,
            values,
            error_message: None,
            scroll_state: ScrollState::new(),
            text_editor: None,
            text_editor_field: None,
            text_editor_result: Arc::new(Mutex::new(None)),
            complete: false,
            app_event_tx,
            keymap,
        };
        view.scroll_state.selected_idx = Some(0);
        view
    }

    fn initial_values(
        provider: WebSearchProviderName,
        current: Option<&WebSearchProviderConfig>,
        fields: &[ProviderConfigField],
    ) -> HashMap<String, String> {
        let mut values = HashMap::new();
        let matched = current.filter(|c| c.provider == provider);
        for field in fields {
            let value = matched.map_or_else(String::new, |c| Self::field_value(c, field));
            values.insert(field.key.to_string(), value);
        }
        values
    }

    fn field_value(config: &WebSearchProviderConfig, field: &ProviderConfigField) -> String {
        match field.key {
            "api_key" => config.api_key.clone().unwrap_or_default(),
            "timeout_ms" => config.timeout_ms.map(|v| v.to_string()).unwrap_or_default(),
            _ => config
                .options
                .get(field.key)
                .map(|v| match v {
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.as_u64().map(|u| u.to_string()).unwrap_or_default(),
                    Value::Bool(b) => b.to_string(),
                    _ => String::new(),
                })
                .unwrap_or_default(),
        }
    }

    fn total_rows(&self) -> usize {
        // One row per field plus a final "Save configuration" action.
        self.fields.len() + 1
    }

    fn is_save_row(&self, idx: usize) -> bool {
        idx == self.fields.len()
    }

    fn selected_field_key(&self) -> Option<String> {
        let idx = self.scroll_state.selected_idx?;
        if self.is_save_row(idx) {
            return Some(SAVE_KEY.to_string());
        }
        self.fields.get(idx).map(|f| f.key.to_string())
    }

    fn build_rows(&self) -> Vec<GenericDisplayRow> {
        let selected = self.scroll_state.selected_idx;
        let mut rows: Vec<GenericDisplayRow> = self
            .fields
            .iter()
            .enumerate()
            .map(|(idx, field)| {
                let prefix = if selected == Some(idx) { '›' } else { ' ' };
                let display_value = self.format_field_value(field);
                let name = format!("{prefix} {}: {}", field.label, display_value);
                GenericDisplayRow {
                    name,
                    description: Some(field.description.to_string()),
                    ..Default::default()
                }
            })
            .collect();

        let save_idx = self.fields.len();
        let prefix = if selected == Some(save_idx) { '›' } else { ' ' };
        rows.push(GenericDisplayRow {
            name: format!("{prefix} {SAVE_LABEL}"),
            description: Some(SAVE_DESCRIPTION.to_string()),
            ..Default::default()
        });
        rows
    }

    fn format_field_value(&self, field: &ProviderConfigField) -> String {
        let raw = self.values.get(field.key).cloned().unwrap_or_default();
        if raw.is_empty() {
            return "not set".to_string();
        }
        match field.kind {
            ProviderConfigFieldKind::ApiKey => "••••••".to_string(),
            _ => raw,
        }
    }

    fn move_up(&mut self) {
        let len = self.total_rows();
        if len == 0 {
            return;
        }
        self.scroll_state.move_up_wrap(len);
        self.error_message = None;
    }

    fn move_down(&mut self) {
        let len = self.total_rows();
        if len == 0 {
            return;
        }
        self.scroll_state.move_down_wrap(len);
        self.error_message = None;
    }

    fn edit_selected(&mut self) {
        let Some(idx) = self.scroll_state.selected_idx else {
            return;
        };
        if self.is_save_row(idx) {
            self.save();
            return;
        }
        let Some(field) = self.fields.get(idx) else {
            return;
        };

        let initial = self.values.get(field.key).cloned().unwrap_or_default();
        let title = format!("Edit {}", field.label);
        let result = self.text_editor_result.clone();
        let result_for_closure = result.clone();

        let on_submit = Box::new(move |text: String| {
            *result_for_closure.lock().unwrap() = Some(text);
        });

        let editor = match field.kind {
            ProviderConfigFieldKind::ApiKey => CustomPromptView::new_secret(
                title,
                "Paste API key (leave blank to use env var)".to_string(),
                initial,
                Some(field.description.to_string()),
                on_submit,
            ),
            _ => CustomPromptView::new(
                title,
                field.description.to_string(),
                initial,
                None,
                on_submit,
            ),
        };

        self.text_editor = Some(editor);
        self.text_editor_field = Some(field.key.to_string());
        self.error_message = None;
        self.text_editor_result = result;
    }

    fn apply_editor_result(&mut self, text: String) {
        let Some(key) = self.text_editor_field.take() else {
            return;
        };
        let trimmed = text.trim().to_string();
        let value = if trimmed.is_empty() { String::new() } else { trimmed };
        self.values.insert(key, value);
    }

    fn save(&mut self) {
        match self.build_config() {
            Ok(config) => {
                self.app_event_tx
                    .send(AppEvent::PersistWebSearchProviderConfig { config });
                self.complete = true;
                self.error_message = None;
            }
            Err(err) => {
                self.error_message = Some(err);
            }
        }
    }

    fn build_config(&self) -> Result<WebSearchProviderConfig, String> {
        let mut options = HashMap::new();

        for field in &self.fields {
            let raw = self.values.get(field.key).cloned().unwrap_or_default();

            // Top-level fields are handled separately.
            if field.key == "api_key" || field.key == "timeout_ms" {
                continue;
            }

            if raw.is_empty() {
                if field.required {
                    return Err(format!("{} is required", field.label));
                }
                continue;
            }

            match &field.kind {
                ProviderConfigFieldKind::U32 { min, max } => {
                    let value: u32 = raw
                        .parse()
                        .map_err(|_| format!("{} must be a number", field.label))?;
                    if value < *min || value > *max {
                        return Err(format!(
                            "{} must be between {} and {}",
                            field.label, min, max
                        ));
                    }
                    options.insert(field.key.to_string(), Value::Number(value.into()));
                }
                ProviderConfigFieldKind::Choice { options: choices } => {
                    if !choices.iter().any(|c| c.eq_ignore_ascii_case(&raw)) {
                        return Err(format!(
                            "{} must be one of: {}",
                            field.label,
                            choices.join(", ")
                        ));
                    }
                    options.insert(field.key.to_string(), Value::String(raw));
                }
                ProviderConfigFieldKind::String | ProviderConfigFieldKind::ApiKey => {
                    options.insert(field.key.to_string(), Value::String(raw));
                }
            }
        }

        let api_key = self
            .values
            .get("api_key")
            .filter(|v| !v.is_empty())
            .cloned();

        let timeout_ms = self
            .values
            .get("timeout_ms")
            .filter(|v| !v.is_empty())
            .map(|v| {
                let field = self
                    .fields
                    .iter()
                    .find(|f| f.key == "timeout_ms")
                    .expect("timeout_ms field is always present");
                match &field.kind {
                    ProviderConfigFieldKind::U32 { min, max } => {
                        let value: u32 = v
                            .parse()
                            .map_err(|_| format!("{} must be a number", field.label))?;
                        if value < *min || value > *max {
                            return Err(format!(
                                "{} must be between {} and {}",
                                field.label, min, max
                            ));
                        }
                        Ok(value as u64)
                    }
                    _ => unreachable!("timeout_ms is always a U32 field"),
                }
            })
            .transpose()?;

        Ok(WebSearchProviderConfig {
            provider: self.provider,
            api_key,
            timeout_ms,
            options,
        })
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

    fn header(&self) -> ColumnRenderable<'_> {
        let mut header = ColumnRenderable::new();
        header.push(Line::from(
            format!("Configure {} web search", self.provider).bold(),
        ));
        header.push(Line::from(
            "Edit the fields below, then choose Save configuration.".dim(),
        ));
        if let Some(error) = &self.error_message {
            header.push(Line::from(Span::styled(
                format!("Error: {error}"),
                ratatui::style::Style::default().fg(ratatui::style::Color::Red),
            )));
        }
        header
    }

    fn footer_hint(&self) -> Line<'static> {
        if self.text_editor.is_some() {
            standard_popup_hint_line()
        } else {
            Line::from(vec![
                "Press ".into(),
                crate::key_hint::plain(KeyCode::Enter).into(),
                " to edit or save; ".into(),
                crate::key_hint::plain(KeyCode::Esc).into(),
                " to cancel".into(),
            ])
        }
    }

    fn rows_width(total_width: u16) -> u16 {
        total_width.saturating_sub(2)
    }
}

impl BottomPaneView for WebSearchProviderConfigView {
    fn handle_key_event(&mut self, key_event: KeyEvent) {
        if let Some(editor) = &mut self.text_editor {
            editor.handle_key_event(key_event);
            return;
        }

        match key_event {
            _ if self.keymap.move_up.is_pressed(key_event) => self.move_up(),
            _ if self.keymap.move_down.is_pressed(key_event) => self.move_down(),
            KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::NONE,
                ..
            } => self.edit_selected(),
            _ if self.keymap.accept.is_pressed(key_event)
                || self.keymap.cancel.is_pressed(key_event) =>
            {
                self.cancel();
            }
            _ => {}
        }
    }

    fn is_complete(&self) -> bool {
        self.complete
    }

    fn completion(&self) -> Option<ViewCompletion> {
        if self.complete {
            Some(ViewCompletion::Accepted)
        } else {
            None
        }
    }

    fn on_ctrl_c(&mut self) -> CancellationEvent {
        self.cancel();
        CancellationEvent::Handled
    }

    fn prefer_esc_to_handle_key_event(&self) -> bool {
        // We always handle Esc ourselves (to cancel the editor or the whole form).
        true
    }

    fn pre_draw_tick(&mut self, _now: Instant) -> bool {
        if let Some(editor) = self.text_editor.take() {
            if editor.is_complete() {
                let accepted = editor.completion() == Some(ViewCompletion::Accepted);
                if accepted {
                    let result = self
                        .text_editor_result
                        .lock()
                        .unwrap()
                        .take()
                        .unwrap_or_default();
                    self.apply_editor_result(result);
                }
                self.text_editor_field = None;
                self.error_message = None;
                return true;
            }
            self.text_editor = Some(editor);
        }
        false
    }

    fn handle_paste(&mut self, pasted: String) -> bool {
        if let Some(editor) = &mut self.text_editor {
            editor.handle_paste(pasted)
        } else {
            false
        }
    }
}

impl Renderable for WebSearchProviderConfigView {
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

        let header = self.header();
        let header_height = header.desired_height(content_area.width.saturating_sub(4));
        let rows = self.build_rows();
        let rows_width = Self::rows_width(content_area.width);
        let rows_height = measure_rows_height(
            &rows,
            &self.scroll_state,
            MAX_POPUP_ROWS,
            rows_width.saturating_add(1),
        );
        let layout = vec![
            Constraint::Max(header_height),
            Constraint::Max(1),
            Constraint::Length(rows_height),
            Constraint::Max(1),
            Constraint::Length(1),
        ];
        let [header_area, _, list_area, _, _docs_area] =
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
                &self.scroll_state,
                MAX_POPUP_ROWS,
                "  No configuration available",
            );
        }

        let hint_area = Rect {
            x: footer_area.x + 2,
            y: footer_area.y,
            width: footer_area.width.saturating_sub(2),
            height: footer_area.height,
        };
        self.footer_hint().render(hint_area, buf);
    }

    fn desired_height(&self, width: u16) -> u16 {
        if let Some(editor) = &self.text_editor {
            return editor.desired_height(width).saturating_add(2);
        }

        let header = self.header();
        let rows = self.build_rows();
        let rows_width = Self::rows_width(width);
        let rows_height = measure_rows_height(
            &rows,
            &self.scroll_state,
            MAX_POPUP_ROWS,
            rows_width.saturating_add(1),
        );
        let mut height = header.desired_height(width.saturating_sub(4));
        height = height.saturating_add(rows_height.max(1) + 6);
        height
    }

    fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        self.text_editor.as_ref().and_then(|e| e.cursor_pos(area))
    }
}

#[cfg(test)]
#[path = "websearch_provider_config_view_tests.rs"]
mod tests;
