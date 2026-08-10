//! Management list for configured database connections.
//!
//! Shows saved connections grouped by provider in three tabs (postgres, mysql, sqlite).
//! Use the left/right arrow keys to switch tabs, up/down to navigate connections, Enter to
//! set a connection as primary, Tab to edit, `a` to add a connection for the active tab's
//! provider, and `d` or Delete to remove a connection (with confirmation).

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
use ratatui::widgets::Block;
use ratatui::widgets::Widget;

use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::CancellationEvent;
use crate::bottom_pane::bottom_pane_view::BottomPaneView;
use crate::bottom_pane::bottom_pane_view::ViewCompletion;
use crate::bottom_pane::popup_consts::MAX_POPUP_ROWS;
use crate::bottom_pane::scroll_state::ScrollState;
use crate::bottom_pane::selection_popup_common::GenericDisplayRow;
use crate::bottom_pane::selection_popup_common::measure_rows_height;
use crate::bottom_pane::selection_popup_common::render_rows;
use crate::bottom_pane::selection_tabs::SelectionTab;
use crate::bottom_pane::selection_tabs::render_tab_bar;
use crate::bottom_pane::selection_tabs::tab_bar_height;
use crate::key_hint::KeyBindingListExt;
use crate::keymap::ListKeymap;
use crate::render::Insets;
use crate::render::RectExt as _;
use crate::render::renderable::ColumnRenderable;
use crate::render::renderable::Renderable;
use crate::style::user_message_style;

use ody_database::config::DatabaseConfig;
use ody_database::config::DatabaseProviderName;

const ALL_PROVIDERS: [DatabaseProviderName; 3] = [
    DatabaseProviderName::Postgres,
    DatabaseProviderName::Mysql,
    DatabaseProviderName::Sqlite,
];

struct TabItem {
    name: String,
    is_primary: bool,
}

struct Tab {
    provider: DatabaseProviderName,
    items: Vec<TabItem>,
}

struct DeleteConfirmation {
    name: String,
    state: ScrollState,
}

pub(crate) struct DatabaseConnectionsView {
    tabs: Vec<Tab>,
    active_tab_idx: usize,
    scroll_state: ScrollState,
    delete_confirmation: Option<DeleteConfirmation>,
    complete: bool,
    app_event_tx: AppEventSender,
    keymap: ListKeymap,
}

impl DatabaseConnectionsView {
    pub(crate) fn new(
        config: DatabaseConfig,
        app_event_tx: AppEventSender,
        keymap: ListKeymap,
    ) -> Self {
        let mut tabs = Vec::with_capacity(ALL_PROVIDERS.len());
        let mut primary_tab_idx = 0;

        for (idx, provider) in ALL_PROVIDERS.iter().copied().enumerate() {
            let mut names: Vec<String> = config
                .connections
                .values()
                .filter(|c| c.provider == provider)
                .map(|c| c.connection.clone())
                .collect();
            names.sort();

            let items = names
                .into_iter()
                .map(|name| TabItem {
                    is_primary: config.primary == name,
                    name,
                })
                .collect();

            if config
                .connections
                .values()
                .any(|c| c.provider == provider && c.connection == config.primary)
            {
                primary_tab_idx = idx;
            }

            tabs.push(Tab { provider, items });
        }

        let mut scroll_state = ScrollState::new();
        scroll_state.selected_idx = Some(0);

        let mut view = Self {
            tabs,
            active_tab_idx: primary_tab_idx,
            scroll_state,
            delete_confirmation: None,
            complete: false,
            app_event_tx,
            keymap,
        };
        view.ensure_valid_selection();
        view
    }

    fn active_tab(&self) -> &Tab {
        &self.tabs[self.active_tab_idx]
    }

    fn active_provider(&self) -> DatabaseProviderName {
        self.active_tab().provider
    }

    fn active_state(&self) -> &ScrollState {
        self.delete_confirmation
            .as_ref()
            .map(|d| &d.state)
            .unwrap_or(&self.scroll_state)
    }

    fn active_state_mut(&mut self) -> &mut ScrollState {
        self.delete_confirmation
            .as_mut()
            .map(|d| &mut d.state)
            .unwrap_or(&mut self.scroll_state)
    }

    fn visible_len(&self) -> usize {
        if self.delete_confirmation.is_some() {
            return 2;
        }
        self.active_tab().items.len()
    }

    fn selected_item(&self) -> Option<&TabItem> {
        let idx = self.scroll_state.selected_idx?;
        self.active_tab().items.get(idx)
    }

    fn selected_name(&self) -> Option<String> {
        self.selected_item().map(|item| item.name.clone())
    }

    fn ensure_valid_selection(&mut self) {
        let len = self.visible_len();
        if len == 0 {
            self.scroll_state.selected_idx = None;
            self.scroll_state.scroll_top = 0;
            return;
        }
        match self.scroll_state.selected_idx {
            None => {
                self.scroll_state.selected_idx = Some(0);
                self.scroll_state.scroll_top = 0;
            }
            Some(idx) if idx >= len => {
                self.scroll_state.selected_idx = Some(len - 1);
                self.scroll_state.scroll_top = len.saturating_sub(1);
            }
            Some(_) => {}
        }
    }

    fn build_rows(&self) -> Vec<GenericDisplayRow> {
        if let Some(confirmation) = &self.delete_confirmation {
            return self.build_delete_confirmation_rows(&confirmation.name);
        }

        let selected = self.scroll_state.selected_idx;
        self.active_tab()
            .items
            .iter()
            .enumerate()
            .map(|(idx, item)| {
                let prefix = if selected == Some(idx) { '›' } else { ' ' };
                let primary_marker = if item.is_primary { " ★" } else { "" };
                GenericDisplayRow {
                    name: format!("{prefix} {}{primary_marker}", item.name),
                    description: Some(
                        "Enter to set primary, Tab to edit, d/Del to delete.".to_string(),
                    ),
                    ..Default::default()
                }
            })
            .collect()
    }

    fn build_delete_confirmation_rows(&self, name: &str) -> Vec<GenericDisplayRow> {
        let selected = self.delete_confirmation.as_ref().and_then(|d| d.state.selected_idx);
        [format!("Delete '{name}'"), "Go back".to_string()]
            .into_iter()
            .enumerate()
            .map(|(idx, label)| {
                let prefix = if selected == Some(idx) { '›' } else { ' ' };
                GenericDisplayRow {
                    name: format!("{prefix} {label}"),
                    description: Some(match idx {
                        0 => "Permanently remove this connection preset.".to_string(),
                        1 => "Return to the connection list.".to_string(),
                        _ => unreachable!(),
                    }),
                    ..Default::default()
                }
            })
            .collect()
    }

    fn build_selection_tabs(&self) -> Vec<SelectionTab> {
        self.tabs
            .iter()
            .map(|tab| SelectionTab {
                id: tab.provider.to_string(),
                label: provider_label(tab.provider),
                header: Box::new(()),
                items: Vec::new(),
            })
            .collect()
    }

    fn header(&self) -> ColumnRenderable<'_> {
        let mut header = ColumnRenderable::new();
        header.push(Line::from("Database Connections".bold()));
        if self.delete_confirmation.is_some() {
            header.push(Line::from(
                "Confirm deletion. This cannot be undone.".dim(),
            ));
        } else {
            header.push(Line::from(
                "←→ switch provider tab · ↑↓ navigate · Enter primary · Tab edit · a add · d/Del delete · Esc cancel."
                    .dim(),
            ));
        }
        header
    }

    fn footer_hint(&self) -> Line<'static> {
        if self.delete_confirmation.is_some() {
            return Line::from(vec![
                "Press ".into(),
                crate::key_hint::plain(KeyCode::Enter).into(),
                " to confirm or ".into(),
                crate::key_hint::plain(KeyCode::Esc).into(),
                " to go back".into(),
            ]);
        }

        Line::from(vec![
            "←→ provider · ".into(),
            crate::key_hint::plain(KeyCode::Up).into(),
            crate::key_hint::plain(KeyCode::Down).into(),
            " navigate · ".into(),
            crate::key_hint::plain(KeyCode::Enter).into(),
            " primary · ".into(),
            crate::key_hint::plain(KeyCode::Tab).into(),
            " edit · ".into(),
            "a".bold().into(),
            " add · ".into(),
            "d".bold().into(),
            "/".into(),
            crate::key_hint::plain(KeyCode::Delete).into(),
            " delete".into(),
        ])
    }

    fn move_up(&mut self) {
        let len = self.visible_len();
        if len == 0 {
            return;
        }
        self.active_state_mut().move_up_wrap(len);
        self.active_state_mut()
            .ensure_visible(len, MAX_POPUP_ROWS.min(len));
    }

    fn move_down(&mut self) {
        let len = self.visible_len();
        if len == 0 {
            return;
        }
        self.active_state_mut().move_down_wrap(len);
        self.active_state_mut()
            .ensure_visible(len, MAX_POPUP_ROWS.min(len));
    }

    fn switch_tab(&mut self, step: isize) {
        if self.delete_confirmation.is_some() {
            return;
        }
        let len = self.tabs.len();
        if len == 0 {
            return;
        }
        let next_idx = if step.is_negative() {
            self.active_tab_idx
                .checked_sub(1)
                .unwrap_or(len.saturating_sub(1))
        } else {
            (self.active_tab_idx + 1) % len
        };
        self.active_tab_idx = next_idx;
        self.scroll_state.selected_idx = Some(0);
        self.scroll_state.scroll_top = 0;
        self.ensure_valid_selection();
    }

    fn set_primary(&mut self) {
        if self.delete_confirmation.is_some() {
            return;
        }
        if let Some(name) = self.selected_name() {
            self.app_event_tx
                .send(AppEvent::SwitchDatabasePrimary { name });
            self.complete = true;
        }
    }

    fn edit(&mut self) {
        if self.delete_confirmation.is_some() {
            return;
        }
        if let Some(name) = self.selected_name() {
            self.app_event_tx
                .send(AppEvent::DatabaseConnectionSelected { name });
            self.complete = true;
        }
    }

    fn add(&mut self) {
        if self.delete_confirmation.is_some() {
            return;
        }
        let provider = self.active_provider();
        self.app_event_tx
            .send(AppEvent::AddDatabaseConnection { provider });
        self.complete = true;
    }

    fn delete(&mut self) {
        if self.delete_confirmation.is_some() {
            return;
        }
        if let Some(name) = self.selected_name() {
            let mut state = ScrollState::new();
            state.selected_idx = Some(0);
            self.delete_confirmation = Some(DeleteConfirmation { name, state });
        }
    }

    fn confirm_delete(&mut self) {
        let Some(confirmation) = &self.delete_confirmation else {
            return;
        };
        match confirmation.state.selected_idx {
            Some(0) => {
                let name = confirmation.name.clone();
                self.app_event_tx
                    .send(AppEvent::DeleteDatabaseConnection { name });
                self.complete = true;
            }
            Some(1) | None => self.close_delete_confirmation(),
            Some(other) => unreachable!("unexpected delete confirmation row: {other}"),
        }
    }

    fn close_delete_confirmation(&mut self) {
        self.delete_confirmation = None;
    }

    fn cancel(&mut self) {
        if self.delete_confirmation.is_some() {
            self.close_delete_confirmation();
        } else {
            self.complete = true;
        }
    }

    fn rows_width(total_width: u16) -> u16 {
        total_width.saturating_sub(2)
    }
}

impl BottomPaneView for DatabaseConnectionsView {
    fn handle_key_event(&mut self, key_event: KeyEvent) {
        match key_event {
            _ if self.keymap.move_up.is_pressed(key_event) => self.move_up(),
            _ if self.keymap.move_down.is_pressed(key_event) => self.move_down(),
            _ if self.keymap.move_left.is_pressed(key_event) => self.switch_tab(-1),
            _ if self.keymap.move_right.is_pressed(key_event) => self.switch_tab(1),
            KeyEvent {
                code: KeyCode::Enter,
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                if self.delete_confirmation.is_some() {
                    self.confirm_delete();
                } else {
                    self.set_primary();
                }
            }
            KeyEvent {
                code: KeyCode::Tab,
                modifiers: KeyModifiers::NONE,
                ..
            } => {
                if self.delete_confirmation.is_none() {
                    self.edit();
                }
            }
            KeyEvent {
                code: KeyCode::Char('a'),
                modifiers: KeyModifiers::NONE,
                ..
            } => self.add(),
            KeyEvent {
                code: KeyCode::Char('d'),
                modifiers: KeyModifiers::NONE,
                ..
            } => self.delete(),
            KeyEvent {
                code: KeyCode::Delete,
                modifiers: KeyModifiers::NONE,
                ..
            } => self.delete(),
            _ if self.keymap.accept.is_pressed(key_event)
                || self.keymap.cancel.is_pressed(key_event) =>
            {
                if self.keymap.accept.is_pressed(key_event) && self.delete_confirmation.is_some() {
                    self.confirm_delete();
                } else {
                    self.cancel();
                }
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
        false
    }
}

impl Renderable for DatabaseConnectionsView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        let [content_area, footer_area] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);

        Block::default()
            .style(user_message_style())
            .render(content_area, buf);

        let content = content_area.inset(Insets::vh(/*v*/ 1, /*h*/ 2));
        let header = self.header();
        let header_height = header.desired_height(content.width);

        let tabs = self.build_selection_tabs();
        let tab_height = tab_bar_height(&tabs, self.active_tab_idx, content.width);
        let tab_area_height = if tabs.is_empty() {
            0
        } else {
            tab_height + u16::from(tab_height > 0)
        };

        let rows = self.build_rows();
        let rows_width = Self::rows_width(content.width);
        let rows_height = measure_rows_height(
            &rows,
            self.active_state(),
            MAX_POPUP_ROWS,
            rows_width.saturating_add(1),
        );

        let layout = vec![
            Constraint::Max(header_height),
            Constraint::Max(tab_area_height),
            Constraint::Length(rows_height),
            Constraint::Length(1),
        ];
        let [header_area, tab_area, list_area, _docs_area] =
            Layout::vertical(layout).areas(content);

        header.render(header_area, buf);

        if !tabs.is_empty() {
            render_tab_bar(&tabs, self.active_tab_idx, tab_area, buf);
        }

        if list_area.height > 0 {
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
                self.active_state(),
                MAX_POPUP_ROWS,
                "  No connections for this provider",
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
        let content_width = width.saturating_sub(4);
        let header = self.header();
        let header_height = header.desired_height(content_width);

        let tabs = self.build_selection_tabs();
        let tab_height = tab_bar_height(&tabs, self.active_tab_idx, content_width);
        let tab_area_height = if tabs.is_empty() {
            0
        } else {
            tab_height + u16::from(tab_height > 0)
        };

        let rows = self.build_rows();
        let rows_width = Self::rows_width(width);
        let rows_height = measure_rows_height(
            &rows,
            self.active_state(),
            MAX_POPUP_ROWS,
            rows_width.saturating_add(1),
        );

        let mut height = header_height;
        height = height.saturating_add(tab_area_height);
        height = height.saturating_add(rows_height.max(1) + 4);
        height
    }
}

fn provider_label(provider: DatabaseProviderName) -> String {
    match provider {
        DatabaseProviderName::Postgres => "PostgreSQL".to_string(),
        DatabaseProviderName::Mysql => "MySQL".to_string(),
        DatabaseProviderName::Sqlite => "SQLite".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ody_database::config::{DatabaseConnectionConfig, DatabaseProviderName};
    use tokio::sync::mpsc;
    use crate::keymap::RuntimeKeymap;
    use std::collections::HashMap;

    fn make_view(config: DatabaseConfig) -> (DatabaseConnectionsView, mpsc::UnboundedReceiver<AppEvent>) {
        let (tx, rx) = mpsc::unbounded_channel::<AppEvent>();
        let view = DatabaseConnectionsView::new(
            config,
            AppEventSender::new(tx),
            RuntimeKeymap::defaults().list,
        );
        (view, rx)
    }

    fn press_key(view: &mut DatabaseConnectionsView, code: KeyCode) {
        view.handle_key_event(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn make_config(primary: &str, connections: Vec<DatabaseConnectionConfig>) -> DatabaseConfig {
        let mut map = HashMap::new();
        for conn in connections {
            map.insert(conn.connection.clone(), conn);
        }
        DatabaseConfig {
            primary: primary.to_string(),
            connections: map,
        }
    }

    fn make_connection(name: &str, provider: DatabaseProviderName) -> DatabaseConnectionConfig {
        DatabaseConnectionConfig {
            connection: name.to_string(),
            provider,
            host: match provider {
                DatabaseProviderName::Sqlite => format!("/tmp/{name}.db"),
                _ => "127.0.0.1".to_string(),
            },
            port: match provider {
                DatabaseProviderName::Postgres => 5432,
                DatabaseProviderName::Mysql => 3306,
                DatabaseProviderName::Sqlite => 0,
            },
            database: match provider {
                DatabaseProviderName::Sqlite => String::new(),
                _ => "db".to_string(),
            },
            username: match provider {
                DatabaseProviderName::Sqlite => String::new(),
                _ => "user".to_string(),
            },
            password: None,
            options: HashMap::new(),
        }
    }

    #[test]
    fn empty_config_starts_on_postgres_tab_with_no_rows() {
        let config = DatabaseConfig {
            primary: String::new(),
            connections: HashMap::new(),
        };
        let (view, _rx) = make_view(config);
        assert_eq!(view.active_tab_idx, 0);
        assert_eq!(view.active_provider(), DatabaseProviderName::Postgres);
        assert_eq!(view.build_rows().len(), 0);
    }

    #[test]
    fn enter_on_connection_sets_primary() {
        let config = make_config(
            "b",
            vec![
                make_connection("a", DatabaseProviderName::Postgres),
                make_connection("b", DatabaseProviderName::Postgres),
            ],
        );
        let (mut view, mut rx) = make_view(config);
        press_key(&mut view, KeyCode::Enter);
        assert!(view.is_complete());
        match rx.try_recv() {
            Ok(AppEvent::SwitchDatabasePrimary { name }) => assert_eq!(name, "a"),
            other => panic!("expected SwitchDatabasePrimary, got {other:?}"),
        }
    }

    #[test]
    fn tab_switches_to_edit_form() {
        let config = make_config(
            "",
            vec![make_connection("local", DatabaseProviderName::Sqlite)],
        );
        let (mut view, mut rx) = make_view(config);
        // The sqlite connection is on the third tab, so switch to it first.
        press_key(&mut view, KeyCode::Right);
        press_key(&mut view, KeyCode::Right);
        assert_eq!(view.active_provider(), DatabaseProviderName::Sqlite);
        press_key(&mut view, KeyCode::Tab);
        assert!(view.is_complete());
        match rx.try_recv() {
            Ok(AppEvent::DatabaseConnectionSelected { name }) => assert_eq!(name, "local"),
            other => panic!("expected DatabaseConnectionSelected, got {other:?}"),
        }
    }

    #[test]
    fn a_key_adds_connection_for_active_provider() {
        let config = DatabaseConfig {
            primary: String::new(),
            connections: HashMap::new(),
        };
        let (mut view, mut rx) = make_view(config);
        // Switch to MySQL tab before pressing 'a'.
        press_key(&mut view, KeyCode::Right);
        assert_eq!(view.active_provider(), DatabaseProviderName::Mysql);
        press_key(&mut view, KeyCode::Char('a'));
        assert!(view.is_complete());
        match rx.try_recv() {
            Ok(AppEvent::AddDatabaseConnection { provider }) => {
                assert_eq!(provider, DatabaseProviderName::Mysql);
            }
            other => panic!("expected AddDatabaseConnection, got {other:?}"),
        }
    }

    #[test]
    fn d_key_opens_delete_confirmation() {
        let config = make_config(
            "",
            vec![make_connection("x", DatabaseProviderName::Postgres)],
        );
        let (mut view, _rx) = make_view(config);
        assert!(view.delete_confirmation.is_none());
        press_key(&mut view, KeyCode::Char('d'));
        assert!(!view.is_complete());
        assert!(view.delete_confirmation.is_some());
        let rows = view.build_rows();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].name.contains("Delete 'x'"));
        assert!(rows[1].name.contains("Go back"));
    }

    #[test]
    fn delete_key_opens_delete_confirmation() {
        let config = make_config(
            "",
            vec![make_connection("x", DatabaseProviderName::Postgres)],
        );
        let (mut view, _rx) = make_view(config);
        press_key(&mut view, KeyCode::Delete);
        assert!(view.delete_confirmation.is_some());
    }

    #[test]
    fn delete_confirmation_enter_sends_delete_event() {
        let config = make_config(
            "",
            vec![make_connection("x", DatabaseProviderName::Postgres)],
        );
        let (mut view, mut rx) = make_view(config);
        press_key(&mut view, KeyCode::Char('d'));
        press_key(&mut view, KeyCode::Enter);
        assert!(view.is_complete());
        match rx.try_recv() {
            Ok(AppEvent::DeleteDatabaseConnection { name }) => assert_eq!(name, "x"),
            other => panic!("expected DeleteDatabaseConnection, got {other:?}"),
        }
    }

    #[test]
    fn delete_confirmation_cancel_closes_confirmation() {
        let config = make_config(
            "",
            vec![make_connection("x", DatabaseProviderName::Postgres)],
        );
        let (mut view, mut rx) = make_view(config);
        press_key(&mut view, KeyCode::Char('d'));
        press_key(&mut view, KeyCode::Down);
        press_key(&mut view, KeyCode::Enter);
        assert!(!view.is_complete());
        assert!(view.delete_confirmation.is_none());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn left_right_keys_switch_tabs() {
        let config = DatabaseConfig {
            primary: String::new(),
            connections: HashMap::new(),
        };
        let (mut view, _rx) = make_view(config);
        assert_eq!(view.active_provider(), DatabaseProviderName::Postgres);
        press_key(&mut view, KeyCode::Right);
        assert_eq!(view.active_provider(), DatabaseProviderName::Mysql);
        press_key(&mut view, KeyCode::Right);
        assert_eq!(view.active_provider(), DatabaseProviderName::Sqlite);
        press_key(&mut view, KeyCode::Right);
        assert_eq!(view.active_provider(), DatabaseProviderName::Postgres);
        press_key(&mut view, KeyCode::Left);
        assert_eq!(view.active_provider(), DatabaseProviderName::Sqlite);
    }

    #[test]
    fn tab_switches_are_disabled_during_delete_confirmation() {
        let config = make_config(
            "",
            vec![make_connection("x", DatabaseProviderName::Postgres)],
        );
        let (mut view, _rx) = make_view(config);
        press_key(&mut view, KeyCode::Char('d'));
        let before = view.active_tab_idx;
        press_key(&mut view, KeyCode::Right);
        assert_eq!(view.active_tab_idx, before);
    }
}
