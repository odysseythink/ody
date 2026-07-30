//! Login/onboarding flow widget for configuring a built-in API-key provider.
//!
//! This is a self-contained onboarding step that mirrors the ChatWidget `/login`
//! state machine but is rendered in the full-screen onboarding style. Phase 2.1
//! implemented the state machine, text editing, keyboard routing, and UI rendering.
//! Phase 2.2 integrates asynchronous model fetching (`fetch_login_models`) and
//! configuration persistence (`write_config_batch`) via the `poll_tasks` hook.

use crossterm::cursor::SetCursorStyle;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use ody_app_server_client::AppServerRequestHandle;
use ody_app_server_protocol::ConfigWriteResponse;
use ody_model_provider::login::LoginModelError;
use ody_model_provider::login::LoginModelInfo;
use ody_model_provider::login::fetch_login_models;
use ody_model_provider_info::BuiltInApiKeyProvider;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::style::Stylize as _;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Paragraph;
use ratatui::widgets::WidgetRef;
use ratatui::widgets::Wrap;
use tokio::task::JoinHandle;
use unicode_segmentation::UnicodeSegmentation as _;
use unicode_width::UnicodeWidthStr;

use crate::config_update::write_config_batch;
use crate::key_hint;
use crate::key_hint::KeyBindingListExt as _;
use crate::legacy_core::config::Config;
use crate::login::config::build_login_models_edits;
use crate::login::config::build_login_provider_edits;
use crate::login::validation::validate_custom_alias;
use crate::onboarding::keys;
use crate::onboarding::onboarding_screen::KeyboardHandler;
use crate::onboarding::onboarding_screen::StepState;
use crate::onboarding::onboarding_screen::StepStateProvider;
use crate::render::renderable::ColumnRenderable;
use crate::render::renderable::Renderable;
use crate::selection_list::selection_option_row;

const PROVIDERS: &[BuiltInApiKeyProvider] = &[
    BuiltInApiKeyProvider::Kimi,
    BuiltInApiKeyProvider::Deepseek,
    BuiltInApiKeyProvider::Glm,
];

const INPUT_PREFIX: &str = "▌ ";

/// Single-line text editor used for alias, API-key, and base-URL input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoginInput {
    value: String,
    secret: bool,
    cursor: usize,
}

impl LoginInput {
    pub(crate) fn new(initial: impl Into<String>, secret: bool) -> Self {
        let value = initial.into();
        let cursor = value.graphemes(true).count();
        Self {
            value,
            secret,
            cursor,
        }
    }

    pub(crate) fn value(&self) -> &str {
        &self.value
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.value.is_empty()
    }

    pub(crate) fn grapheme_len(&self) -> usize {
        self.value.graphemes(true).count()
    }

    fn graphemes(&self) -> Vec<&str> {
        self.value.graphemes(true).collect()
    }

    fn clamp_cursor(&mut self) {
        let len = self.grapheme_len();
        if self.cursor > len {
            self.cursor = len;
        }
    }

    pub(crate) fn move_cursor_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    pub(crate) fn move_cursor_right(&mut self) {
        let len = self.grapheme_len();
        if self.cursor < len {
            self.cursor += 1;
        }
    }

    pub(crate) fn move_cursor_home(&mut self) {
        self.cursor = 0;
    }

    pub(crate) fn move_cursor_end(&mut self) {
        self.cursor = self.grapheme_len();
    }

    pub(crate) fn insert_char(&mut self, ch: char) {
        self.insert_str(&ch.to_string());
    }

    pub(crate) fn insert_str(&mut self, s: &str) {
        let graphemes = self.graphemes();
        let before: String = graphemes.iter().take(self.cursor).copied().collect();
        let after: String = graphemes.iter().skip(self.cursor).copied().collect();
        self.value = before + s + &after;
        self.cursor += s.graphemes(true).count();
        self.clamp_cursor();
    }

    pub(crate) fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let graphemes = self.graphemes();
        let before: String = graphemes.iter().take(self.cursor - 1).copied().collect();
        let after: String = graphemes.iter().skip(self.cursor).copied().collect();
        self.value = before + &after;
        self.cursor -= 1;
    }

    pub(crate) fn delete(&mut self) {
        let graphemes = self.graphemes();
        if self.cursor >= graphemes.len() {
            return;
        }
        let before: String = graphemes.iter().take(self.cursor).copied().collect();
        let after: String = graphemes.iter().skip(self.cursor + 1).copied().collect();
        self.value = before + &after;
    }

    pub(crate) fn clear(&mut self) {
        self.value.clear();
        self.cursor = 0;
    }

    fn set_value(&mut self, value: impl Into<String>) {
        self.value = value.into();
        self.cursor = self.grapheme_len();
    }

    fn display_value(&self) -> String {
        if self.secret {
            "*".repeat(self.grapheme_len())
        } else {
            self.value.clone()
        }
    }
}

impl Renderable for &LoginInput {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        let line = Line::from(vec![
            Span::styled(INPUT_PREFIX, Style::default().cyan()),
            Span::raw(self.display_value()),
        ]);
        Paragraph::new(line).render(area, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        1
    }

    fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        if area.is_empty() {
            return None;
        }
        let before = if self.secret {
            "*".repeat(self.cursor)
        } else {
            self.value.graphemes(true).take(self.cursor).collect()
        };
        let x = area
            .x
            .saturating_add(UnicodeWidthStr::width(INPUT_PREFIX) as u16)
            .saturating_add(UnicodeWidthStr::width(before.as_str()) as u16);
        Some((x, area.y))
    }

    fn cursor_style(&self, _area: Rect) -> SetCursorStyle {
        SetCursorStyle::SteadyBar
    }
}

/// State machine for the onboarding login flow.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LoginState {
    PickProvider {
        highlighted: usize,
    },
    EnterAlias {
        provider: BuiltInApiKeyProvider,
        alias: LoginInput,
    },
    EnterApiKey {
        provider: BuiltInApiKeyProvider,
        alias: String,
        api_key: LoginInput,
    },
    EnterBaseUrl {
        provider: BuiltInApiKeyProvider,
        alias: String,
        api_key: String,
        base_url: LoginInput,
    },
    FetchingModels {
        provider: BuiltInApiKeyProvider,
        alias: String,
        api_key: String,
        base_url: String,
    },
    PickDefaultModel {
        provider: BuiltInApiKeyProvider,
        alias: String,
        api_key: String,
        base_url: String,
        models: Vec<LoginModelInfo>,
        highlighted: usize,
    },
    Done {
        provider: Option<BuiltInApiKeyProvider>,
        alias: Option<String>,
        api_key: Option<String>,
        base_url: Option<String>,
        model_id: Option<String>,
        set_as_default: bool,
        persisted: bool,
        skipped: bool,
    },
    Error {
        previous: Box<LoginState>,
        message: String,
    },
}

/// Widget that drives the onboarding login flow.
#[allow(dead_code)]
pub(crate) struct LoginFlowWidget {
    state: LoginState,
    set_as_default: bool,
    request_handle: Option<AppServerRequestHandle>,
    fetch_task: Option<JoinHandle<Result<Vec<LoginModelInfo>, LoginModelError>>>,
    persist_task: Option<JoinHandle<Result<ConfigWriteResponse, color_eyre::eyre::Error>>>,
}

impl std::fmt::Debug for LoginFlowWidget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginFlowWidget")
            .field("state", &self.state)
            .field("set_as_default", &self.set_as_default)
            .field("has_request_handle", &self.request_handle.is_some())
            .field("has_fetch_task", &self.fetch_task.is_some())
            .field("has_persist_task", &self.persist_task.is_some())
            .finish()
    }
}

impl PartialEq for LoginFlowWidget {
    fn eq(&self, other: &Self) -> bool {
        self.state == other.state && self.set_as_default == other.set_as_default
    }
}

impl Eq for LoginFlowWidget {}

#[allow(dead_code)]
impl LoginFlowWidget {
    pub(crate) fn new(config: &Config) -> Self {
        Self {
            state: LoginState::PickProvider { highlighted: 0 },
            set_as_default: !config.has_active_model,
            request_handle: None,
            fetch_task: None,
            persist_task: None,
        }
    }

    pub(crate) fn with_request_handle(mut self, request_handle: AppServerRequestHandle) -> Self {
        self.request_handle = Some(request_handle);
        self
    }

    pub(crate) fn state(&self) -> &LoginState {
        &self.state
    }

    pub(crate) fn set_as_default(&self) -> bool {
        self.set_as_default
    }

    pub(crate) fn persisted(&self) -> bool {
        matches!(
            self.state,
            LoginState::Done {
                persisted: true,
                ..
            }
        )
    }

    pub(crate) fn skipped(&self) -> bool {
        matches!(
            self.state,
            LoginState::Done {
                skipped: true,
                ..
            }
        )
    }

    pub(crate) async fn poll_tasks(&mut self) {
        if let Some(task) = self.fetch_task.as_mut() {
            if task.is_finished() {
                let task = self.fetch_task.take().expect("just checked");
                match task.await {
                    Ok(Ok(models)) => {
                        if let LoginState::FetchingModels {
                            provider,
                            alias,
                            api_key,
                            base_url,
                        } = &self.state
                        {
                            self.state = LoginState::PickDefaultModel {
                                provider: *provider,
                                alias: alias.clone(),
                                api_key: api_key.clone(),
                                base_url: base_url.clone(),
                                models,
                                highlighted: 0,
                            };
                        }
                    }
                    Ok(Err(err)) => {
                        if let LoginState::FetchingModels {
                            provider,
                            alias,
                            api_key,
                            base_url,
                        } = &self.state
                        {
                            let message = match err {
                                LoginModelError::NoModels => {
                                    "No models returned by the provider. Please check the base URL."
                                        .to_string()
                                }
                                _ => format!("Failed to fetch models: {err}"),
                            };
                            let previous = LoginState::EnterBaseUrl {
                                provider: *provider,
                                alias: alias.clone(),
                                api_key: api_key.clone(),
                                base_url: LoginInput::new(base_url.clone(), false),
                            };
                            self.state = LoginState::Error {
                                previous: Box::new(previous),
                                message,
                            };
                        }
                    }
                    Err(_) => {
                        if let LoginState::FetchingModels {
                            provider,
                            alias,
                            api_key,
                            base_url,
                        } = &self.state
                        {
                            let previous = LoginState::EnterBaseUrl {
                                provider: *provider,
                                alias: alias.clone(),
                                api_key: api_key.clone(),
                                base_url: LoginInput::new(base_url.clone(), false),
                            };
                            self.state = LoginState::Error {
                                previous: Box::new(previous),
                                message: "Failed to fetch models: task failed".to_string(),
                            };
                        }
                    }
                }
            }
        }

        if let Some(task) = self.persist_task.as_mut() {
            if task.is_finished() {
                let task = self.persist_task.take().expect("just checked");
                match task.await {
                    Ok(Ok(_response)) => {
                        if let LoginState::Done { persisted, .. } = &mut self.state {
                            *persisted = true;
                        }
                    }
                    Ok(Err(err)) => {
                        if let LoginState::Done {
                            persisted: false, ..
                        } = &self.state
                        {
                            let previous = self.state.clone();
                            self.state = LoginState::Error {
                                previous: Box::new(previous),
                                message: format!("Failed to persist configuration: {err:#}"),
                            };
                        }
                    }
                    Err(_) => {
                        if let LoginState::Done {
                            persisted: false, ..
                        } = &self.state
                        {
                            let previous = self.state.clone();
                            self.state = LoginState::Error {
                                previous: Box::new(previous),
                                message: "Configuration persistence task failed".to_string(),
                            };
                        }
                    }
                }
            }
        }
    }

    pub(crate) fn is_text_editing(&self) -> bool {
        matches!(
            self.state,
            LoginState::EnterAlias { .. }
                | LoginState::EnterApiKey { .. }
                | LoginState::EnterBaseUrl { .. }
        )
    }

    pub(crate) fn should_suppress_animations(&self) -> bool {
        self.is_text_editing()
    }

    fn active_input_mut(&mut self) -> Option<&mut LoginInput> {
        match &mut self.state {
            LoginState::EnterAlias { alias, .. } => Some(alias),
            LoginState::EnterApiKey { api_key, .. } => Some(api_key),
            LoginState::EnterBaseUrl { base_url, .. } => Some(base_url),
            _ => None,
        }
    }

    fn provider_index(&self, provider: BuiltInApiKeyProvider) -> usize {
        PROVIDERS.iter().position(|p| *p == provider).unwrap_or(0)
    }

    fn select_provider(&mut self, index: usize) {
        let provider = PROVIDERS[index.min(PROVIDERS.len().saturating_sub(1))];
        self.state = LoginState::EnterAlias {
            provider,
            alias: LoginInput::new("", false),
        };
    }

    fn start_persist(
        &mut self,
        provider: BuiltInApiKeyProvider,
        alias: String,
        api_key: String,
        base_url: String,
        model_id: String,
        models: Vec<LoginModelInfo>,
    ) {
        let persisted = if let Some(handle) = self.request_handle.clone() {
            let mut edits = build_login_provider_edits(&alias, provider, &api_key, Some(&base_url));
            edits.extend(build_login_models_edits(
                &alias,
                provider,
                &models,
                &model_id,
                self.set_as_default,
            ));
            self.persist_task = Some(tokio::spawn(async move {
                write_config_batch(handle, edits).await
            }));
            false
        } else {
            true
        };
        self.state = LoginState::Done {
            provider: Some(provider),
            alias: Some(alias),
            api_key: Some(api_key),
            base_url: Some(base_url),
            model_id: Some(model_id),
            set_as_default: self.set_as_default,
            persisted,
            skipped: false,
        };
    }

    fn go_back(&mut self) {
        let state = std::mem::replace(&mut self.state, LoginState::PickProvider { highlighted: 0 });
        match state {
            LoginState::EnterAlias {
                provider,
                mut alias,
            } => {
                if alias.is_empty() {
                    self.state = LoginState::PickProvider {
                        highlighted: self.provider_index(provider),
                    };
                } else {
                    alias.clear();
                    self.state = LoginState::EnterAlias { provider, alias };
                }
            }
            LoginState::EnterApiKey {
                provider,
                alias,
                mut api_key,
            } => {
                if api_key.is_empty() {
                    self.state = LoginState::EnterAlias {
                        provider,
                        alias: LoginInput::new(alias, false),
                    };
                } else {
                    api_key.clear();
                    self.state = LoginState::EnterApiKey {
                        provider,
                        alias,
                        api_key,
                    };
                }
            }
            LoginState::EnterBaseUrl {
                provider,
                alias,
                api_key,
                mut base_url,
            } => {
                let default = provider.default_base_url().to_string();
                if base_url.value() != default.as_str() {
                    base_url.set_value(default);
                    self.state = LoginState::EnterBaseUrl {
                        provider,
                        alias,
                        api_key,
                        base_url,
                    };
                } else {
                    self.state = LoginState::EnterApiKey {
                        provider,
                        alias,
                        api_key: LoginInput::new(api_key, true),
                    };
                }
            }
            LoginState::PickDefaultModel {
                provider,
                alias,
                api_key,
                base_url,
                ..
            } => {
                self.state = LoginState::EnterBaseUrl {
                    provider,
                    alias,
                    api_key,
                    base_url: LoginInput::new(base_url, false),
                };
            }
            state => {
                self.state = state;
            }
        }
    }

    fn submit_text_state(&mut self) {
        match &mut self.state {
            LoginState::EnterAlias { provider, alias } => {
                let alias_str = alias.value().trim().to_string();
                if let Err(err) = validate_custom_alias(&alias_str) {
                    self.state = LoginState::Error {
                        previous: Box::new(self.state.clone()),
                        message: err,
                    };
                    return;
                }
                self.state = LoginState::EnterApiKey {
                    provider: *provider,
                    alias: alias_str,
                    api_key: LoginInput::new("", true),
                };
            }
            LoginState::EnterApiKey {
                provider,
                alias,
                api_key,
            } => {
                let key = api_key.value().trim().to_string();
                if key.is_empty() {
                    self.state = LoginState::Error {
                        previous: Box::new(self.state.clone()),
                        message: "API key cannot be empty".to_string(),
                    };
                    return;
                }
                self.state = LoginState::EnterBaseUrl {
                    provider: *provider,
                    alias: alias.clone(),
                    api_key: key,
                    base_url: LoginInput::new(provider.default_base_url(), false),
                };
            }
            LoginState::EnterBaseUrl {
                provider,
                alias,
                api_key,
                base_url,
            } => {
                let url = if base_url.value().trim().is_empty() {
                    provider.default_base_url().to_string()
                } else {
                    base_url.value().trim().to_string()
                };
                let provider = *provider;
                let alias = alias.clone();
                let api_key = api_key.clone();
                self.state = LoginState::FetchingModels {
                    provider,
                    alias: alias.clone(),
                    api_key: api_key.clone(),
                    base_url: url.clone(),
                };
                self.fetch_task = Some(tokio::spawn(async move {
                    let extra_headers = match provider {
                        BuiltInApiKeyProvider::Kimi => {
                            ody_model_provider_info::create_kimi_provider().http_headers
                        }
                        _ => None,
                    };
                    fetch_login_models(provider, &url, &api_key, extra_headers).await
                }));
            }
            _ => {}
        }
    }

    fn handle_text_navigation(&mut self, key_event: KeyEvent) {
        let Some(input) = self.active_input_mut() else {
            return;
        };

        if key_hint::is_plain_text_key_event(key_event) {
            if let KeyCode::Char(ch) = key_event.code {
                input.insert_char(ch);
            }
            return;
        }

        match key_event.code {
            KeyCode::Left => input.move_cursor_left(),
            KeyCode::Right => input.move_cursor_right(),
            KeyCode::Home => input.move_cursor_home(),
            KeyCode::End => input.move_cursor_end(),
            KeyCode::Backspace => input.backspace(),
            KeyCode::Delete => input.delete(),
            _ => {}
        }
    }

    fn build_column(&self) -> ColumnRenderable<'_> {
        let mut column = ColumnRenderable::new();

        match &self.state {
            LoginState::PickProvider { highlighted } => {
                column.push(Line::from(vec!["> ".into(), "Select provider".bold()]));
                column.push("");
                column.push(
                    Paragraph::new(
                        "Choose an API-key provider to configure. Press Esc to skip this step."
                            .to_string(),
                    )
                    .wrap(Wrap { trim: true }),
                );
                column.push("");
                for (idx, provider) in PROVIDERS.iter().enumerate() {
                    column.push(selection_option_row(
                        idx,
                        provider.display_name().to_string(),
                        *highlighted == idx,
                    ));
                }
                column.push("");
                column.push(hint_line(&[
                    ("Select", &keys::CONFIRM),
                    ("Move", &keys::MOVE_UP),
                    ("Skip", &keys::CANCEL),
                ]));
            }
            LoginState::EnterAlias { provider, alias } => {
                column.push(header_line(format!("Login to {}", provider.display_name())));
                column.push("");
                column.push(Paragraph::new(
                    "Enter a custom alias for this provider. Aliases cannot be 'kimi', 'deepseek', or 'glm'."
                        .to_string(),
                ).wrap(Wrap { trim: true }));
                column.push("");
                column.push(alias);
                column.push("");
                column.push(hint_line(&[
                    ("Continue", &keys::CONFIRM),
                    ("Clear / Go back", &keys::CANCEL),
                ]));
            }
            LoginState::EnterApiKey {
                provider,
                alias,
                api_key,
            } => {
                column.push(header_line(format!("Login to {}", provider.display_name())));
                column.push("");
                column.push(
                    Paragraph::new("Paste your API key.".to_string()).wrap(Wrap { trim: true }),
                );
                column.push(
                    Paragraph::new(format!("Key will be saved in providers.{alias}.api_key").dim())
                        .wrap(Wrap { trim: true }),
                );
                column.push("");
                column.push(api_key);
                column.push("");
                column.push(hint_line(&[
                    ("Continue", &keys::CONFIRM),
                    ("Clear / Go back", &keys::CANCEL),
                ]));
            }
            LoginState::EnterBaseUrl {
                provider, base_url, ..
            } => {
                column.push(header_line(format!("Login to {}", provider.display_name())));
                column.push("");
                column.push(
                    Paragraph::new(
                        "Press Enter to use the default base URL, or edit it below.".to_string(),
                    )
                    .wrap(Wrap { trim: true }),
                );
                column.push(
                    Paragraph::new(format!("Default: {}", provider.default_base_url()).dim())
                        .wrap(Wrap { trim: true }),
                );
                column.push("");
                column.push(base_url);
                column.push("");
                column.push(hint_line(&[
                    ("Continue", &keys::CONFIRM),
                    ("Reset / Go back", &keys::CANCEL),
                ]));
            }
            LoginState::FetchingModels { provider, .. } => {
                column.push(header_line(format!(
                    "Verifying {} API key...",
                    provider.display_name()
                )));
                column.push("");
                column.push(Line::from(vec![
                    "  ".into(),
                    "⠋".cyan(),
                    " Fetching available models...".into(),
                ]));
                column.push("");
                column.push(
                    Paragraph::new("Please wait while available models are fetched.".dim())
                        .wrap(Wrap { trim: true }),
                );
            }
            LoginState::PickDefaultModel {
                provider,
                models,
                highlighted,
                ..
            } => {
                column.push(Line::from(vec!["> ".into(), "Select default model".bold()]));
                column.push("");
                column.push(
                    Paragraph::new(format!("Choose a model for {}.", provider.display_name()))
                        .wrap(Wrap { trim: true }),
                );
                column.push("");
                for (idx, model) in models.iter().enumerate() {
                    let label = if model.display_name.is_empty() {
                        model.id.clone()
                    } else {
                        format!("{} — {}", model.id, model.display_name)
                    };
                    column.push(selection_option_row(idx, label, *highlighted == idx));
                }
                column.push("");
                column.push(hint_line(&[
                    ("Select", &keys::CONFIRM),
                    ("Move", &keys::MOVE_UP),
                    ("Back", &keys::CANCEL),
                ]));
            }
            LoginState::Done {
                provider,
                alias,
                model_id,
                ..
            } => {
                column.push(Line::from(vec!["> ".into(), "Login complete".bold()]));
                column.push("");
                if let Some(provider) = provider {
                    let detail = match (alias, model_id) {
                        (Some(alias), Some(model_id)) => format!(
                            "Configured {} as {} with default model {}.",
                            provider.display_name(),
                            alias,
                            model_id
                        ),
                        (Some(alias), None) => {
                            format!("Configured {} as {}.", provider.display_name(), alias)
                        }
                        (None, Some(model_id)) => format!(
                            "Configured {} with default model {}.",
                            provider.display_name(),
                            model_id
                        ),
                        (None, None) => format!("Configured {}.", provider.display_name()),
                    };
                    column.push(Paragraph::new(detail).wrap(Wrap { trim: true }));
                } else {
                    column.push(
                        Paragraph::new("Login skipped.".to_string()).wrap(Wrap { trim: true }),
                    );
                }
                if self.set_as_default {
                    column.push(
                        Paragraph::new(
                            "This provider will be set as the default model source.".dim(),
                        )
                        .wrap(Wrap { trim: true }),
                    );
                }
            }
            LoginState::Error { message, .. } => {
                column.push(Line::from(vec!["> ".into(), "Error".bold()]));
                column.push("");
                column.push(
                    Paragraph::new(message.clone())
                        .red()
                        .wrap(Wrap { trim: true }),
                );
                column.push("");
                column.push(hint_line(&[("Go back", &keys::CANCEL)]));
            }
        }

        column
    }
}

impl KeyboardHandler for LoginFlowWidget {
    fn handle_key_event(&mut self, key_event: KeyEvent) {
        if key_event.kind == KeyEventKind::Release {
            return;
        }

        if keys::CANCEL.is_pressed(key_event) || keys::QUIT.is_pressed(key_event) {
            let state =
                std::mem::replace(&mut self.state, LoginState::PickProvider { highlighted: 0 });
            match state {
                LoginState::PickProvider { .. } => {
                    self.state = LoginState::Done {
                        provider: None,
                        alias: None,
                        api_key: None,
                        base_url: None,
                        model_id: None,
                        set_as_default: self.set_as_default,
                        persisted: false,
                        skipped: true,
                    };
                }
                LoginState::Error { previous, .. } => {
                    self.state = *previous;
                }
                state => {
                    self.state = state;
                    self.go_back();
                }
            }
            return;
        }

        match &mut self.state {
            LoginState::PickProvider { highlighted } => {
                if keys::MOVE_UP.is_pressed(key_event) && *highlighted > 0 {
                    *highlighted -= 1;
                } else if keys::MOVE_DOWN.is_pressed(key_event)
                    && *highlighted + 1 < PROVIDERS.len()
                {
                    *highlighted += 1;
                } else if keys::SELECT_FIRST.is_pressed(key_event) {
                    self.select_provider(0);
                } else if keys::SELECT_SECOND.is_pressed(key_event) {
                    self.select_provider(1);
                } else if keys::CONFIRM.is_pressed(key_event) {
                    let idx = *highlighted;
                    self.select_provider(idx);
                }
            }
            LoginState::EnterAlias { .. }
            | LoginState::EnterApiKey { .. }
            | LoginState::EnterBaseUrl { .. } => {
                if keys::CONFIRM.is_pressed(key_event) {
                    self.submit_text_state();
                } else {
                    self.handle_text_navigation(key_event);
                }
            }
            LoginState::PickDefaultModel {
                highlighted,
                models,
                ..
            } => {
                if keys::MOVE_UP.is_pressed(key_event) && *highlighted > 0 {
                    *highlighted -= 1;
                } else if keys::MOVE_DOWN.is_pressed(key_event) && *highlighted + 1 < models.len() {
                    *highlighted += 1;
                } else if keys::CONFIRM.is_pressed(key_event) {
                    let model_id = models[*highlighted].id.clone();
                    if let LoginState::PickDefaultModel {
                        provider,
                        alias,
                        api_key,
                        base_url,
                        models,
                        ..
                    } = std::mem::replace(
                        &mut self.state,
                        LoginState::PickProvider { highlighted: 0 },
                    ) {
                        self.start_persist(provider, alias, api_key, base_url, model_id, models);
                    }
                } else if keys::SELECT_FIRST.is_pressed(key_event) && !models.is_empty() {
                    *highlighted = 0;
                    let model_id = models[0].id.clone();
                    if let LoginState::PickDefaultModel {
                        provider,
                        alias,
                        api_key,
                        base_url,
                        models,
                        ..
                    } = std::mem::replace(
                        &mut self.state,
                        LoginState::PickProvider { highlighted: 0 },
                    ) {
                        self.start_persist(provider, alias, api_key, base_url, model_id, models);
                    }
                }
            }
            _ => {}
        }
    }

    fn handle_paste(&mut self, pasted: String) {
        let sanitized = sanitize_paste(pasted);
        if sanitized.is_empty() {
            return;
        }
        if let Some(input) = self.active_input_mut() {
            input.insert_str(&sanitized);
        }
    }
}

impl StepStateProvider for LoginFlowWidget {
    fn get_step_state(&self) -> StepState {
        match &self.state {
            LoginState::Done { .. } => StepState::Complete,
            _ => StepState::InProgress,
        }
    }
}

impl Renderable for LoginFlowWidget {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.build_column().render(area, buf);
    }

    fn desired_height(&self, width: u16) -> u16 {
        self.build_column().desired_height(width)
    }

    fn cursor_pos(&self, area: Rect) -> Option<(u16, u16)> {
        self.build_column().cursor_pos(area)
    }

    fn cursor_style(&self, area: Rect) -> SetCursorStyle {
        self.build_column().cursor_style(area)
    }
}

impl WidgetRef for &LoginFlowWidget {
    fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        self.render(area, buf);
    }
}

fn header_line(title: String) -> Line<'static> {
    Line::from(vec!["> ".into(), title.bold()])
}

fn hint_line(actions: &[(&str, &[key_hint::KeyBinding])]) -> Line<'static> {
    let mut spans: Vec<Span> = Vec::new();
    for (i, (label, bindings)) in actions.iter().enumerate() {
        if i > 0 {
            spans.push("  ".into());
        }
        spans.push(format!("{} ", label).dim());
        if let Some(binding) = bindings.first() {
            spans.push(binding.display_label().cyan());
        }
    }
    spans.insert(0, "  ".into());
    Line::from(spans)
}

fn sanitize_paste(pasted: String) -> String {
    pasted.lines().next().unwrap_or("").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyCode;
    use crossterm::event::KeyEvent;
    use crossterm::event::KeyModifiers;
    use pretty_assertions::assert_eq;
    use ratatui::Terminal;

    use crate::legacy_core::config::Config;
    use crate::test_backend::VT100Backend;
    use ody_app_server_protocol::WriteStatus;
    use ody_utils_absolute_path::AbsolutePathBuf;
    use std::time::Duration;

    fn default_widget() -> LoginFlowWidget {
        LoginFlowWidget {
            state: LoginState::PickProvider { highlighted: 0 },
            set_as_default: true,
            request_handle: None,
            fetch_task: None,
            persist_task: None,
        }
    }

    async fn wait_for_state<F>(widget: &mut LoginFlowWidget, predicate: F)
    where
        F: Fn(&LoginState) -> bool,
    {
        tokio::time::timeout(Duration::from_secs(1), async {
            while !predicate(widget.state()) {
                widget.poll_tasks().await;
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("state change timed out");
    }

    #[tokio::test]
    async fn new_uses_has_active_model() {
        let config = crate::legacy_core::config::ConfigBuilder::default()
            .build()
            .await
            .expect("config");
        let widget = LoginFlowWidget::new(&config);
        assert_eq!(widget.set_as_default(), !config.has_active_model);
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn char_key(ch: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)
    }

    fn press(event: KeyEvent) -> KeyEvent {
        KeyEvent {
            kind: KeyEventKind::Press,
            ..event
        }
    }

    #[test]
    fn initial_state_is_provider_picker() {
        let widget = default_widget();
        assert!(matches!(
            widget.state(),
            LoginState::PickProvider { highlighted: 0 }
        ));
    }

    #[test]
    fn provider_picker_moves_and_selects() {
        let mut widget = default_widget();
        widget.handle_key_event(press(key(KeyCode::Down)));
        assert!(matches!(
            widget.state(),
            LoginState::PickProvider { highlighted: 1 }
        ));
        widget.handle_key_event(press(key(KeyCode::Up)));
        assert!(matches!(
            widget.state(),
            LoginState::PickProvider { highlighted: 0 }
        ));
        widget.handle_key_event(press(key(KeyCode::Enter)));
        assert!(matches!(
            widget.state(),
            LoginState::EnterAlias {
                provider: BuiltInApiKeyProvider::Kimi,
                ..
            }
        ));
    }

    #[test]
    fn select_keys_pick_provider_directly() {
        let mut widget = default_widget();
        widget.handle_key_event(press(key(KeyCode::Char('2'))));
        assert!(matches!(
            widget.state(),
            LoginState::EnterAlias {
                provider: BuiltInApiKeyProvider::Deepseek,
                ..
            }
        ));
    }

    #[test]
    fn esc_skips_from_provider_picker() {
        let mut widget = default_widget();
        widget.handle_key_event(press(key(KeyCode::Esc)));
        assert!(matches!(
            widget.state(),
            LoginState::Done {
                provider: None,
                persisted: false,
                ..
            }
        ));
        assert_eq!(widget.get_step_state(), StepState::Complete);
    }

    #[test]
    fn esc_at_provider_picker_skips_login() {
        let mut widget = default_widget();
        widget.handle_key_event(press(key(KeyCode::Esc)));
        assert!(matches!(
            widget.state(),
            LoginState::Done {
                provider: None,
                persisted: false,
                skipped: true,
                ..
            }
        ));
        assert_eq!(widget.get_step_state(), StepState::Complete);
        assert!(widget.skipped());
        assert!(!widget.persisted());
    }

    #[test]
    fn alias_validation_error_then_esc_returns() {
        let mut widget = default_widget();
        widget.handle_key_event(press(key(KeyCode::Enter)));
        widget.handle_key_event(press(char_key('k')));
        widget.handle_key_event(press(char_key('i')));
        widget.handle_key_event(press(char_key('m')));
        widget.handle_key_event(press(char_key('i')));
        widget.handle_key_event(press(key(KeyCode::Enter)));
        assert!(matches!(widget.state(), LoginState::Error { .. }));
        widget.handle_key_event(press(key(KeyCode::Esc)));
        assert!(matches!(
            widget.state(),
            LoginState::EnterAlias {
                provider: BuiltInApiKeyProvider::Kimi,
                ..
            }
        ));
    }

    #[test]
    fn empty_api_key_shows_error() {
        let mut widget = LoginFlowWidget {
            state: LoginState::EnterAlias {
                provider: BuiltInApiKeyProvider::Kimi,
                alias: LoginInput::new("work-kimi", false),
            },
            set_as_default: true,
            request_handle: None,
            fetch_task: None,
            persist_task: None,
        };
        widget.handle_key_event(press(key(KeyCode::Enter)));
        assert!(matches!(widget.state(), LoginState::EnterApiKey { .. }));
        widget.handle_key_event(press(key(KeyCode::Enter)));
        assert!(matches!(widget.state(), LoginState::Error { .. }));
    }

    #[tokio::test]
    async fn base_url_enter_goes_to_fetching() {
        let mut widget = LoginFlowWidget {
            state: LoginState::EnterApiKey {
                provider: BuiltInApiKeyProvider::Kimi,
                alias: "work-kimi".to_string(),
                api_key: LoginInput::new("secret-key", true),
            },
            set_as_default: true,
            request_handle: None,
            fetch_task: None,
            persist_task: None,
        };
        widget.handle_key_event(press(key(KeyCode::Enter)));
        assert!(matches!(widget.state(), LoginState::EnterBaseUrl { .. }));
        widget.handle_key_event(press(key(KeyCode::Enter)));
        assert!(matches!(widget.state(), LoginState::FetchingModels { .. }));
    }

    #[test]
    fn esc_clears_base_url_to_default() {
        let mut widget = LoginFlowWidget {
            state: LoginState::EnterBaseUrl {
                provider: BuiltInApiKeyProvider::Kimi,
                alias: "work-kimi".to_string(),
                api_key: "secret".to_string(),
                base_url: LoginInput::new("https://custom.example.com", false),
            },
            set_as_default: true,
            request_handle: None,
            fetch_task: None,
            persist_task: None,
        };
        widget.handle_key_event(press(key(KeyCode::Esc)));
        assert!(matches!(
            widget.state(),
            LoginState::EnterBaseUrl {
                provider: BuiltInApiKeyProvider::Kimi,
                ..
            }
        ));
        if let LoginState::EnterBaseUrl { base_url, .. } = widget.state() {
            assert_eq!(
                base_url.value(),
                BuiltInApiKeyProvider::Kimi.default_base_url()
            );
        }
        widget.handle_key_event(press(key(KeyCode::Esc)));
        assert!(matches!(widget.state(), LoginState::EnterApiKey { .. }));
    }

    #[test]
    fn model_picker_selects_and_completes() {
        let models = vec![
            LoginModelInfo {
                id: "kimi-k2".to_string(),
                display_name: "Kimi K2".to_string(),
            },
            LoginModelInfo {
                id: "kimi-moonshot".to_string(),
                display_name: "Moonshot".to_string(),
            },
        ];
        let mut widget = LoginFlowWidget {
            state: LoginState::PickDefaultModel {
                provider: BuiltInApiKeyProvider::Kimi,
                alias: "work-kimi".to_string(),
                api_key: "secret".to_string(),
                base_url: "https://api.moonshot.cn".to_string(),
                models,
                highlighted: 1,
            },
            set_as_default: true,
            request_handle: None,
            fetch_task: None,
            persist_task: None,
        };
        widget.handle_key_event(press(key(KeyCode::Enter)));
        assert!(matches!(widget.state(), LoginState::Done { .. }));
        assert_eq!(widget.get_step_state(), StepState::Complete);
        assert!(widget.persisted());
        if let LoginState::Done {
            model_id: Some(id), ..
        } = widget.state()
        {
            assert_eq!(id, "kimi-moonshot");
        } else {
            panic!("expected completed model selection");
        }
    }

    #[tokio::test]
    async fn poll_tasks_fetch_success_transitions_to_model_picker() {
        let models = vec![LoginModelInfo {
            id: "kimi-k2".to_string(),
            display_name: "Kimi K2".to_string(),
        }];
        let mut widget = LoginFlowWidget {
            state: LoginState::FetchingModels {
                provider: BuiltInApiKeyProvider::Kimi,
                alias: "work-kimi".to_string(),
                api_key: "secret".to_string(),
                base_url: "https://api.moonshot.cn".to_string(),
            },
            set_as_default: true,
            request_handle: None,
            fetch_task: Some(tokio::spawn(async move { Ok(models) })),
            persist_task: None,
        };
        wait_for_state(&mut widget, |s| {
            matches!(s, LoginState::PickDefaultModel { .. })
        })
        .await;
        assert!(matches!(
            widget.state(),
            LoginState::PickDefaultModel { highlighted: 0, .. }
        ));
    }

    #[tokio::test]
    async fn poll_tasks_fetch_failure_shows_error() {
        let mut widget = LoginFlowWidget {
            state: LoginState::FetchingModels {
                provider: BuiltInApiKeyProvider::Kimi,
                alias: "work-kimi".to_string(),
                api_key: "secret".to_string(),
                base_url: "https://api.moonshot.cn".to_string(),
            },
            set_as_default: true,
            request_handle: None,
            fetch_task: Some(tokio::spawn(async move {
                Err(LoginModelError::RequestFailed("network error".to_string()))
            })),
            persist_task: None,
        };
        wait_for_state(&mut widget, |s| matches!(s, LoginState::Error { .. })).await;
        assert!(matches!(widget.state(), LoginState::Error { .. }));
    }

    #[tokio::test]
    async fn fetch_failure_returns_to_base_url() {
        let mut widget = LoginFlowWidget {
            state: LoginState::FetchingModels {
                provider: BuiltInApiKeyProvider::Kimi,
                alias: "work-kimi".to_string(),
                api_key: "secret".to_string(),
                base_url: "https://api.moonshot.cn".to_string(),
            },
            set_as_default: true,
            request_handle: None,
            fetch_task: Some(tokio::spawn(async move {
                Err(LoginModelError::RequestFailed("network error".to_string()))
            })),
            persist_task: None,
        };
        wait_for_state(&mut widget, |s| matches!(s, LoginState::Error { .. })).await;
        assert!(matches!(widget.state(), LoginState::Error { .. }));
        widget.handle_key_event(press(key(KeyCode::Esc)));
        assert!(matches!(
            widget.state(),
            LoginState::EnterBaseUrl {
                provider: BuiltInApiKeyProvider::Kimi,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn poll_tasks_persist_success_marks_done_persisted() {
        let mut widget = LoginFlowWidget {
            state: LoginState::Done {
                provider: Some(BuiltInApiKeyProvider::Kimi),
                alias: Some("work-kimi".to_string()),
                api_key: Some("secret".to_string()),
                base_url: Some("https://api.moonshot.cn".to_string()),
                model_id: Some("kimi-k2".to_string()),
                set_as_default: true,
                persisted: false,
                skipped: false,
            },
            set_as_default: true,
            request_handle: None,
            fetch_task: None,
            persist_task: Some(tokio::spawn(async move {
                Ok(ConfigWriteResponse {
                    status: WriteStatus::Ok,
                    version: "1".to_string(),
                    file_path: AbsolutePathBuf::try_from("/tmp/config.toml").unwrap(),
                    overridden_metadata: None,
                })
            })),
        };
        wait_for_state(&mut widget, |s| {
            matches!(
                s,
                LoginState::Done {
                    persisted: true,
                    ..
                }
            )
        })
        .await;
        assert!(matches!(
            widget.state(),
            LoginState::Done {
                persisted: true,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn poll_tasks_persist_failure_shows_error() {
        let mut widget = LoginFlowWidget {
            state: LoginState::Done {
                provider: Some(BuiltInApiKeyProvider::Kimi),
                alias: Some("work-kimi".to_string()),
                api_key: Some("secret".to_string()),
                base_url: Some("https://api.moonshot.cn".to_string()),
                model_id: Some("kimi-k2".to_string()),
                set_as_default: true,
                persisted: false,
                skipped: false,
            },
            set_as_default: true,
            request_handle: None,
            fetch_task: None,
            persist_task: Some(tokio::spawn(async move {
                Err(color_eyre::eyre::eyre!("config write failed"))
            })),
        };
        wait_for_state(&mut widget, |s| matches!(s, LoginState::Error { .. })).await;
        assert!(matches!(widget.state(), LoginState::Error { .. }));
    }

    #[test]
    fn build_login_config_edits() {
        let provider = BuiltInApiKeyProvider::Kimi;
        let models = vec![LoginModelInfo {
            id: "kimi-k2".to_string(),
            display_name: "Kimi K2".to_string(),
        }];
        let mut edits = build_login_provider_edits(
            "work-kimi",
            provider,
            "secret-key",
            Some("https://api.moonshot.cn"),
        );
        edits.extend(build_login_models_edits(
            "work-kimi",
            provider,
            &models,
            "kimi-k2",
            true,
        ));
        assert_eq!(edits[0].key_path, "providers.work-kimi.type");
        assert_eq!(edits[0].value, serde_json::json!("kimi"));
        assert_eq!(edits[1].key_path, "providers.work-kimi.api_key");
        assert_eq!(edits[2].key_path, "providers.work-kimi.base_url");
        let model_key = "work-kimi/kimi-k2";
        assert!(
            edits
                .iter()
                .any(|e| e.key_path == format!("models.\"{model_key}\".provider"))
        );
        assert!(edits.iter().any(|e| e.key_path == "default_model"));
    }

    #[test]
    fn input_editing_and_cursor() {
        let mut input = LoginInput::new("ab", false);
        input.move_cursor_home();
        input.insert_char('x');
        assert_eq!(input.value(), "xab");
        input.move_cursor_right();
        input.move_cursor_right();
        input.insert_char('y');
        assert_eq!(input.value(), "xaby");
        input.backspace();
        assert_eq!(input.value(), "xab");
        input.move_cursor_home();
        input.move_cursor_right();
        input.delete();
        assert_eq!(input.value(), "xb");
        input.move_cursor_end();
        input.delete();
        assert_eq!(input.value(), "xb");
    }

    #[test]
    fn secret_input_displays_bullets() {
        let input = LoginInput::new("abcd", true);
        assert_eq!(input.display_value(), "****");
    }

    #[test]
    fn paste_is_sanitized_to_single_line() {
        assert_eq!(sanitize_paste("hello\nworld\r\nextra".to_string()), "hello");
        let mut widget = default_widget();
        widget.handle_key_event(press(key(KeyCode::Enter)));
        widget.handle_paste("multi\r\nline".to_string());
        assert!(matches!(
            widget.state(),
            LoginState::EnterAlias { alias, .. } if alias.value() == "multi"
        ));
    }

    #[test]
    fn text_editing_only_in_input_states() {
        let mut widget = default_widget();
        assert!(!widget.is_text_editing());
        widget.handle_key_event(press(key(KeyCode::Enter)));
        assert!(widget.is_text_editing());
        assert!(widget.should_suppress_animations());
    }

    #[test]
    fn fetching_models_does_not_respond_to_esc() {
        let mut widget = LoginFlowWidget {
            state: LoginState::FetchingModels {
                provider: BuiltInApiKeyProvider::Kimi,
                alias: "work-kimi".to_string(),
                api_key: "secret".to_string(),
                base_url: "https://api.moonshot.cn".to_string(),
            },
            set_as_default: true,
            request_handle: None,
            fetch_task: None,
            persist_task: None,
        };
        widget.handle_key_event(press(key(KeyCode::Esc)));
        assert!(matches!(widget.state(), LoginState::FetchingModels { .. }));
    }

    fn render_snapshot(widget: &LoginFlowWidget, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(VT100Backend::new(width, height)).expect("terminal");
        terminal
            .draw(|f| widget.render_ref(f.area(), f.buffer_mut()))
            .expect("draw");
        terminal.backend().to_string()
    }

    fn render_and_assert(widget: &LoginFlowWidget, name: &str) {
        let mut terminal = Terminal::new(VT100Backend::new(80, 24)).expect("terminal");
        terminal
            .draw(|f| widget.render_ref(f.area(), f.buffer_mut()))
            .expect("draw");
        insta::assert_snapshot!(name, terminal.backend());
    }

    #[test]
    fn renders_pick_provider() {
        let widget = default_widget();
        render_and_assert(&widget, "login_flow_pick_provider");
    }

    #[test]
    fn renders_enter_alias() {
        let mut widget = default_widget();
        widget.handle_key_event(press(key(KeyCode::Enter)));
        render_and_assert(&widget, "login_flow_enter_alias");
    }

    #[test]
    fn renders_enter_api_key() {
        let mut widget = LoginFlowWidget {
            state: LoginState::EnterApiKey {
                provider: BuiltInApiKeyProvider::Kimi,
                alias: "work-kimi".to_string(),
                api_key: LoginInput::new("sk-12345", true),
            },
            set_as_default: true,
            request_handle: None,
            fetch_task: None,
            persist_task: None,
        };
        render_and_assert(&widget, "login_flow_enter_api_key");
    }

    #[test]
    fn renders_enter_base_url() {
        let mut widget = LoginFlowWidget {
            state: LoginState::EnterBaseUrl {
                provider: BuiltInApiKeyProvider::Kimi,
                alias: "work-kimi".to_string(),
                api_key: "secret".to_string(),
                base_url: LoginInput::new("https://api.moonshot.cn", false),
            },
            set_as_default: true,
            request_handle: None,
            fetch_task: None,
            persist_task: None,
        };
        render_and_assert(&widget, "login_flow_enter_base_url");
    }

    #[test]
    fn renders_fetching_models() {
        let widget = LoginFlowWidget {
            state: LoginState::FetchingModels {
                provider: BuiltInApiKeyProvider::Kimi,
                alias: "work-kimi".to_string(),
                api_key: "secret".to_string(),
                base_url: "https://api.moonshot.cn".to_string(),
            },
            set_as_default: true,
            request_handle: None,
            fetch_task: None,
            persist_task: None,
        };
        render_and_assert(&widget, "login_flow_fetching_models");
    }

    #[test]
    fn renders_pick_default_model() {
        let models = vec![
            LoginModelInfo {
                id: "kimi-k2".to_string(),
                display_name: "Kimi K2".to_string(),
            },
            LoginModelInfo {
                id: "kimi-moonshot".to_string(),
                display_name: "Moonshot".to_string(),
            },
        ];
        let widget = LoginFlowWidget {
            state: LoginState::PickDefaultModel {
                provider: BuiltInApiKeyProvider::Kimi,
                alias: "work-kimi".to_string(),
                api_key: "secret".to_string(),
                base_url: "https://api.moonshot.cn".to_string(),
                models,
                highlighted: 0,
            },
            set_as_default: true,
            request_handle: None,
            fetch_task: None,
            persist_task: None,
        };
        render_and_assert(&widget, "login_flow_pick_default_model");
    }

    #[test]
    fn renders_done() {
        let widget = LoginFlowWidget {
            state: LoginState::Done {
                provider: Some(BuiltInApiKeyProvider::Kimi),
                alias: Some("work-kimi".to_string()),
                api_key: Some("secret".to_string()),
                base_url: Some("https://api.moonshot.cn".to_string()),
                model_id: Some("kimi-k2".to_string()),
                set_as_default: true,
                persisted: false,
                skipped: false,
            },
            set_as_default: true,
            request_handle: None,
            fetch_task: None,
            persist_task: None,
        };
        render_and_assert(&widget, "login_flow_done");
    }

    #[test]
    fn renders_error() {
        let mut widget = default_widget();
        widget.handle_key_event(press(key(KeyCode::Enter)));
        widget.handle_key_event(press(char_key('k')));
        widget.handle_key_event(press(char_key('i')));
        widget.handle_key_event(press(char_key('m')));
        widget.handle_key_event(press(char_key('i')));
        widget.handle_key_event(press(key(KeyCode::Enter)));
        render_and_assert(&widget, "login_flow_error");
    }

    #[tokio::test]
    async fn full_keyboard_flow_reaches_fetching_models() {
        let mut widget = default_widget();
        // PickProvider -> EnterAlias
        widget.handle_key_event(press(key(KeyCode::Enter)));
        assert!(matches!(widget.state(), LoginState::EnterAlias { .. }));
        // Type a valid alias and submit
        for ch in "work-kimi".chars() {
            widget.handle_key_event(press(char_key(ch)));
        }
        widget.handle_key_event(press(key(KeyCode::Enter)));
        assert!(matches!(widget.state(), LoginState::EnterApiKey { .. }));
        // Type an API key and submit
        for ch in "sk-secret".chars() {
            widget.handle_key_event(press(char_key(ch)));
        }
        widget.handle_key_event(press(key(KeyCode::Enter)));
        assert!(matches!(widget.state(), LoginState::EnterBaseUrl { .. }));
        // Submit default base URL -> FetchingModels
        widget.handle_key_event(press(key(KeyCode::Enter)));
        assert!(matches!(widget.state(), LoginState::FetchingModels { .. }));
    }

    #[test]
    fn go_back_from_enter_alias_clears_then_returns_to_picker() {
        let mut widget = LoginFlowWidget {
            state: LoginState::EnterAlias {
                provider: BuiltInApiKeyProvider::Kimi,
                alias: LoginInput::new("partial", false),
            },
            set_as_default: true,
            request_handle: None,
            fetch_task: None,
            persist_task: None,
        };
        widget.handle_key_event(press(key(KeyCode::Esc)));
        // First Esc clears the alias text but stays in EnterAlias
        assert!(matches!(
            widget.state(),
            LoginState::EnterAlias {
                provider: BuiltInApiKeyProvider::Kimi,
                ..
            }
        ));
        if let LoginState::EnterAlias { alias, .. } = widget.state() {
            assert!(alias.is_empty());
        }
        // Second Esc returns to provider picker with Kimi highlighted
        widget.handle_key_event(press(key(KeyCode::Esc)));
        assert!(matches!(
            widget.state(),
            LoginState::PickProvider { highlighted: 0 }
        ));
    }

    #[test]
    fn go_back_from_enter_api_key_clears_then_returns_to_alias() {
        let mut widget = LoginFlowWidget {
            state: LoginState::EnterApiKey {
                provider: BuiltInApiKeyProvider::Kimi,
                alias: "work-kimi".to_string(),
                api_key: LoginInput::new("partial-key", true),
            },
            set_as_default: true,
            request_handle: None,
            fetch_task: None,
            persist_task: None,
        };
        widget.handle_key_event(press(key(KeyCode::Esc)));
        assert!(matches!(
            widget.state(),
            LoginState::EnterApiKey {
                provider: BuiltInApiKeyProvider::Kimi,
                ..
            }
        ));
        if let LoginState::EnterApiKey { api_key, .. } = widget.state() {
            assert!(api_key.is_empty());
        }
        widget.handle_key_event(press(key(KeyCode::Esc)));
        assert!(matches!(
            widget.state(),
            LoginState::EnterAlias {
                provider: BuiltInApiKeyProvider::Kimi,
                ..
            }
        ));
    }

    #[test]
    fn go_back_from_pick_default_model_returns_to_base_url() {
        let models = vec![LoginModelInfo {
            id: "kimi-k2".to_string(),
            display_name: "Kimi K2".to_string(),
        }];
        let mut widget = LoginFlowWidget {
            state: LoginState::PickDefaultModel {
                provider: BuiltInApiKeyProvider::Kimi,
                alias: "work-kimi".to_string(),
                api_key: "secret".to_string(),
                base_url: "https://api.moonshot.cn".to_string(),
                models,
                highlighted: 0,
            },
            set_as_default: true,
            request_handle: None,
            fetch_task: None,
            persist_task: None,
        };
        widget.handle_key_event(press(key(KeyCode::Esc)));
        assert!(matches!(
            widget.state(),
            LoginState::EnterBaseUrl {
                provider: BuiltInApiKeyProvider::Kimi,
                ..
            }
        ));
        if let LoginState::EnterBaseUrl { base_url, .. } = widget.state() {
            assert_eq!(base_url.value(), "https://api.moonshot.cn");
        }
    }

    #[test]
    fn alias_validation_errors_at_widget_layer() {
        fn assert_alias_error(alias: &str, expected_message: &str) {
            let mut widget = LoginFlowWidget {
                state: LoginState::EnterAlias {
                    provider: BuiltInApiKeyProvider::Kimi,
                    alias: LoginInput::new(alias, false),
                },
                set_as_default: true,
                request_handle: None,
                fetch_task: None,
                persist_task: None,
            };
            widget.handle_key_event(press(key(KeyCode::Enter)));
            assert!(
                matches!(widget.state(), LoginState::Error { .. }),
                "alias '{alias}' should produce an error state"
            );
            if let LoginState::Error { message, .. } = widget.state() {
                assert_eq!(message, expected_message);
            }
        }

        assert_alias_error("", "Alias cannot be empty");
        assert_alias_error("kimi", "'kimi' is a reserved provider alias");
        assert_alias_error(
            "my alias",
            "Alias may only contain letters, numbers, hyphens, and underscores",
        );
        assert_alias_error("1alias", "Alias must start with a letter");
        assert_alias_error("a".repeat(65).as_str(), "Alias must be 64 characters or fewer");
    }

    #[test]
    fn done_state_is_complete() {
        let widget = LoginFlowWidget {
            state: LoginState::Done {
                provider: Some(BuiltInApiKeyProvider::Kimi),
                alias: Some("work-kimi".to_string()),
                api_key: Some("secret".to_string()),
                base_url: Some("https://api.moonshot.cn".to_string()),
                model_id: Some("kimi-k2".to_string()),
                set_as_default: true,
                persisted: false,
                skipped: false,
            },
            set_as_default: true,
            request_handle: None,
            fetch_task: None,
            persist_task: None,
        };
        assert_eq!(widget.get_step_state(), StepState::Complete);
        assert!(widget.persisted() || !widget.skipped());
    }

    #[tokio::test]
    async fn no_models_fetch_error_shows_special_message() {
        let mut widget = LoginFlowWidget {
            state: LoginState::FetchingModels {
                provider: BuiltInApiKeyProvider::Kimi,
                alias: "work-kimi".to_string(),
                api_key: "secret".to_string(),
                base_url: "https://api.moonshot.cn".to_string(),
            },
            set_as_default: true,
            request_handle: None,
            fetch_task: Some(tokio::spawn(async move { Err(LoginModelError::NoModels) })),
            persist_task: None,
        };
        wait_for_state(&mut widget, |s| matches!(s, LoginState::Error { .. })).await;
        assert!(matches!(widget.state(), LoginState::Error { .. }));
        if let LoginState::Error { message, .. } = widget.state() {
            assert_eq!(
                message,
                "No models returned by the provider. Please check the base URL."
            );
        }
    }

    #[test]
    fn renders_done_skipped() {
        let widget = LoginFlowWidget {
            state: LoginState::Done {
                provider: None,
                alias: None,
                api_key: None,
                base_url: None,
                model_id: None,
                set_as_default: true,
                persisted: false,
                skipped: true,
            },
            set_as_default: true,
            request_handle: None,
            fetch_task: None,
            persist_task: None,
        };
        render_and_assert(&widget, "login_flow_done_skipped");
    }
}
