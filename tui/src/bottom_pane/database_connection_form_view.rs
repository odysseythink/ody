//! Editable form for one database connection preset.
//!
//! The form includes the connection name, provider choice, and the provider-specific
//! fields reported by [`ody_database::provider_config_fields`].

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

use ody_database::config::DatabaseConnectionConfig;
use ody_database::config::DatabaseProviderName;
use ody_database::provider_config_fields::ProviderConfigField;
use ody_database::provider_config_fields::ProviderConfigFieldKind;
use ody_database::provider_config_fields::provider_config_fields;

const CONNECTION_KEY: &str = "__connection";
const PROVIDER_KEY: &str = "__provider";
const SAVE_KEY: &str = "__save__";
const SAVE_LABEL: &str = "Save connection";
const SAVE_DESCRIPTION: &str = "Write the connection configuration to config.toml.";

pub(crate) struct DatabaseConnectionFormView {
    provider: DatabaseProviderName,
    fields: Vec<ProviderConfigField>,
    values: HashMap<String, String>,
    existing_name: Option<String>,
    error_message: Option<String>,

    scroll_state: ScrollState,
    text_editor: Option<CustomPromptView>,
    text_editor_field: Option<String>,
    text_editor_result: Arc<Mutex<Option<String>>>,
    complete: bool,

    app_event_tx: AppEventSender,
    keymap: ListKeymap,
}

impl DatabaseConnectionFormView {
    pub(crate) fn new(
        provider: DatabaseProviderName,
        existing_name: Option<String>,
        current: Option<DatabaseConnectionConfig>,
        app_event_tx: AppEventSender,
        keymap: ListKeymap,
    ) -> Self {
        let values = Self::initial_values(existing_name.as_deref(), provider, current.as_ref());
        let mut view = Self {
            provider,
            fields: provider_config_fields(provider),
            values,
            existing_name,
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
        existing_name: Option<&str>,
        provider: DatabaseProviderName,
        current: Option<&DatabaseConnectionConfig>,
    ) -> HashMap<String, String> {
        let mut values = HashMap::new();
        values.insert(
            CONNECTION_KEY.to_string(),
            existing_name.unwrap_or("").to_string(),
        );
        values.insert(PROVIDER_KEY.to_string(), provider.to_string());
        let current = current.filter(|c| c.provider == provider);
        for field in provider_config_fields(provider) {
            let value = current.map_or_else(String::new, |c| Self::field_value(c, &field));
            values.insert(field.key.to_string(), value);
        }
        values
    }

    fn field_value(config: &DatabaseConnectionConfig, field: &ProviderConfigField) -> String {
        match field.key {
            "host" => config.host.clone(),
            "port" => config.port.to_string(),
            "database" => config.database.clone(),
            "username" => config.username.clone(),
            "password" => config.password.clone().unwrap_or_default(),
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
        // connection name, one per field, plus save action.
        self.fields.len() + 2
    }

    fn is_save_row(&self, idx: usize) -> bool {
        idx == self.fields.len() + 1
    }

    fn field_for_row(&self, idx: usize) -> Option<&ProviderConfigField> {
        if idx >= 1 && idx < self.fields.len() + 1 {
            self.fields.get(idx - 1)
        } else {
            None
        }
    }

    fn row_key(&self, idx: usize) -> Option<String> {
        match idx {
            0 => Some(CONNECTION_KEY.to_string()),
            idx if self.is_save_row(idx) => Some(SAVE_KEY.to_string()),
            idx => self.field_for_row(idx).map(|f| f.key.to_string()),
        }
    }

    fn build_rows(&self) -> Vec<GenericDisplayRow> {
        let selected = self.scroll_state.selected_idx;
        let mut rows = Vec::with_capacity(self.total_rows());

        // Connection name row.
        let idx = 0;
        let prefix = if selected == Some(idx) { '›' } else { ' ' };
        let name_value = self.values.get(CONNECTION_KEY).cloned().unwrap_or_default();
        rows.push(GenericDisplayRow {
            name: format!("{prefix} Name: {}", Self::display_or_placeholder(&name_value)),
            description: Some("Connection preset name used in /database and DatabaseQuery.".to_string()),
            ..Default::default()
        });

        // Field rows.
        for (field_idx, field) in self.fields.iter().enumerate() {
            let idx = field_idx + 1;
            let prefix = if selected == Some(idx) { '›' } else { ' ' };
            let display_value = self.format_field_value(field);
            rows.push(GenericDisplayRow {
                name: format!("{prefix} {}: {}", field.label, display_value),
                description: Some(field.description.to_string()),
                ..Default::default()
            });
        }

        // Save row.
        let save_idx = self.fields.len() + 2;
        let prefix = if selected == Some(save_idx) { '›' } else { ' ' };
        rows.push(GenericDisplayRow {
            name: format!("{prefix} {SAVE_LABEL}"),
            description: Some(SAVE_DESCRIPTION.to_string()),
            ..Default::default()
        });

        rows
    }

    fn display_or_placeholder(value: &str) -> String {
        if value.is_empty() {
            "not set".to_string()
        } else {
            value.to_string()
        }
    }

    fn format_field_value(&self, field: &ProviderConfigField) -> String {
        let raw = self.values.get(field.key).cloned().unwrap_or_default();
        if raw.is_empty() {
            return "not set".to_string();
        }
        match field.kind {
            ProviderConfigFieldKind::Password => "••••••".to_string(),
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
        let Some(key) = self.row_key(idx) else {
            return;
        };

        let initial = self.values.get(&key).cloned().unwrap_or_default();
        let title = if key == CONNECTION_KEY {
            "Connection name".to_string()
        } else {
            let field = self.field_for_row(idx).expect("field row");
            format!("Edit {}", field.label)
        };
        let result = self.text_editor_result.clone();
        let result_for_closure = result.clone();
        let on_submit = Box::new(move |text: String| {
            *result_for_closure.lock().unwrap() = Some(text);
        });

        let editor = if key == CONNECTION_KEY {
            CustomPromptView::new(
                title,
                "Preset name used to reference this connection.".to_string(),
                initial,
                None,
                on_submit,
            )
        } else {
            let field = self.field_for_row(idx).expect("field row");
            match field.kind {
                ProviderConfigFieldKind::Password => CustomPromptView::new_secret(
                    title,
                    "Paste password (leave blank for no password)".to_string(),
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
            }
        };

        self.text_editor = Some(editor);
        self.text_editor_field = Some(key);
        self.text_editor_result = result;
        self.error_message = None;
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
                    .send(AppEvent::PersistDatabaseConnection { config });
                self.complete = true;
                self.error_message = None;
            }
            Err(err) => {
                self.error_message = Some(err);
            }
        }
    }

    fn build_config(&self) -> Result<DatabaseConnectionConfig, String> {
        let connection = self
            .values
            .get(CONNECTION_KEY)
            .filter(|v| !v.is_empty())
            .cloned()
            .ok_or("Connection name is required")?;

        if let Some(existing) = &self.existing_name {
            if *existing != connection {
                return Err("Cannot rename an existing connection; delete and recreate it.".to_string());
            }
        }

        let provider = self.provider;

        let host = self
            .values
            .get("host")
            .filter(|v| !v.is_empty())
            .cloned()
            .ok_or("Host / Path is required")?;

        let port = self
            .values
            .get("port")
            .filter(|v| !v.is_empty())
            .map(|v| {
                v.parse::<u16>()
                    .map_err(|_| "Port must be a number between 1 and 65535".to_string())
            })
            .transpose()?
            .unwrap_or(0);

        let database = self.values.get("database").cloned().unwrap_or_default();
        let username = self.values.get("username").cloned().unwrap_or_default();
        let password = self.values.get("password").filter(|v| !v.is_empty()).cloned();

        let mut options = HashMap::new();
        for field in &self.fields {
            if matches!(
                field.key,
                "host" | "port" | "database" | "username" | "password"
            ) {
                continue;
            }
            if let Some(raw) = self.values.get(field.key).filter(|v| !v.is_empty()) {
                match &field.kind {
                    ProviderConfigFieldKind::U16 { .. } => {
                        let value: u16 = raw
                            .parse()
                            .map_err(|_| format!("{} must be a number", field.label))?;
                        options.insert(field.key.to_string(), Value::Number(value.into()));
                    }
                    _ => {
                        options.insert(field.key.to_string(), Value::String(raw.clone()));
                    }
                }
            }
        }

        Ok(DatabaseConnectionConfig {
            connection,
            provider,
            host,
            port,
            database,
            username,
            password,
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
        let title = if self.existing_name.is_some() {
            "Edit database connection"
        } else {
            "Add database connection"
        };
        header.push(Line::from(title.bold()));
        header.push(Line::from(
            "Edit the fields below, then choose Save connection.".dim(),
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

impl BottomPaneView for DatabaseConnectionFormView {
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

impl Renderable for DatabaseConnectionFormView {
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
mod tests {
    use super::*;
    use ody_database::config::DatabaseProviderName;
    use tokio::sync::mpsc;
    use crate::keymap::RuntimeKeymap;

    fn make_view(
        provider: DatabaseProviderName,
        existing_name: Option<&str>,
        current: Option<DatabaseConnectionConfig>,
    ) -> (DatabaseConnectionFormView, mpsc::UnboundedReceiver<AppEvent>) {
        let (tx, rx) = mpsc::unbounded_channel::<AppEvent>();
        let view = DatabaseConnectionFormView::new(
            provider,
            existing_name.map(|s| s.to_string()),
            current,
            AppEventSender::new(tx),
            RuntimeKeymap::defaults().list,
        );
        (view, rx)
    }

    fn press_enter(view: &mut DatabaseConnectionFormView) {
        view.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    }

    fn press_esc(view: &mut DatabaseConnectionFormView) {
        view.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    }

    #[test]
    fn new_view_starts_with_connection_and_field_rows() {
        let (view, _rx) = make_view(DatabaseProviderName::Postgres, None, None);
        let rows = view.build_rows();
        assert!(rows[0].name.contains("Name"));
        assert!(!rows.iter().any(|r| r.name.contains("Provider")));
        assert!(rows[rows.len() - 1].name.contains("Save"));
    }

    #[test]
    fn save_requires_connection_name() {
        let (mut view, _rx) = make_view(DatabaseProviderName::Postgres, None, None);
        view.scroll_state.selected_idx = Some(view.total_rows() - 1);
        press_enter(&mut view);
        assert!(!view.is_complete());
        assert!(view.error_message.as_ref().unwrap().contains("Connection name"));
    }

    #[test]
    fn saving_valid_connection_sends_persist_event() {
        let (mut view, mut rx) = make_view(DatabaseProviderName::Sqlite, None, None);
        view.values.insert(CONNECTION_KEY.to_string(), "local".to_string());
        view.values.insert("host".to_string(), "/tmp/test.db".to_string());
        view.scroll_state.selected_idx = Some(view.total_rows() - 1);
        press_enter(&mut view);
        assert!(view.is_complete());
        match rx.try_recv() {
            Ok(AppEvent::PersistDatabaseConnection { config }) => {
                assert_eq!(config.connection, "local");
                assert_eq!(config.provider, DatabaseProviderName::Sqlite);
                assert_eq!(config.host, "/tmp/test.db");
            }
            other => panic!("expected PersistDatabaseConnection, got {other:?}"),
        }
    }

    #[test]
    fn cancel_completes_without_sending_event() {
        let (mut view, mut rx) = make_view(DatabaseProviderName::Postgres, None, None);
        press_esc(&mut view);
        assert!(view.is_complete());
        assert!(rx.try_recv().is_err());
    }
}
