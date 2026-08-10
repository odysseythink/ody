//! Settings-adjacent popup surfaces for `ChatWidget`.
//!
//! This keeps theme, personality, and experimental-feature UI out of the main
//! orchestration module without changing their event wiring.

use super::*;
use crate::bottom_pane::PreferencesContent;
use crate::bottom_pane::PreferencesView;
use crate::bottom_pane::popup_consts::standard_popup_hint_line;
use crate::config_update::DesignReviewEditState;
use ody_database::config::DatabaseConfig;
use ody_database::config::DatabaseProviderName;
use ody_protocol::config_types::ModeKind;
use ody_web_search::config::WebSearchProviderName;

impl ChatWidget {
    pub(super) fn open_theme_picker(&mut self) {
        let ody_home = ody_utils_home_dir::find_ody_home().ok();
        let terminal_width = self
            .last_rendered_width
            .get()
            .and_then(|width| u16::try_from(width).ok());
        let params = crate::theme_picker::build_theme_picker_params(
            self.config.tui_theme.as_deref(),
            ody_home.as_deref(),
            terminal_width,
        );
        self.bottom_pane.show_selection_view(params);
    }

    pub(crate) fn open_personality_popup(&mut self) {
        if !self.is_session_configured() {
            self.add_info_message(
                "Personality selection is disabled until startup completes.".to_string(),
                /*hint*/ None,
            );
            return;
        }
        if !self.current_model_supports_personality() {
            let current_model = self.current_model();
            self.add_error_message(format!(
                "Current model ({current_model}) doesn't support personalities. Try /model to pick a different model."
            ));
            return;
        }
        self.open_personality_popup_for_current_model();
    }

    fn open_personality_popup_for_current_model(&mut self) {
        let current_personality = self.config.personality.unwrap_or(Personality::Friendly);
        let personalities = [Personality::Friendly, Personality::Pragmatic];
        let supports_personality = self.current_model_supports_personality();

        let items: Vec<SelectionItem> = personalities
            .into_iter()
            .map(|personality| {
                let name = Self::personality_label(personality).to_string();
                let description = Some(Self::personality_description(personality).to_string());
                let actions: Vec<SelectionAction> = vec![Box::new(move |tx| {
                    tx.send(AppEvent::OdyOp(AppCommand::override_turn_context(
                        /*cwd*/ None,
                        /*approval_policy*/ None,
                        /*approvals_reviewer*/ None,
                        /*permission_profile*/ None,
                        /*active_permission_profile*/ None,
                        /*windows_sandbox_level*/ None,
                        /*model*/ None,
                        /*effort*/ None,
                        /*summary*/ None,
                        /*service_tier*/ None,
                        /*collaboration_mode*/ None,
                        Some(personality),
                    )));
                    tx.send(AppEvent::UpdatePersonality(personality));
                    tx.send(AppEvent::PersistPersonalitySelection { personality });
                })];
                SelectionItem {
                    name,
                    description,
                    is_current: current_personality == personality,
                    is_disabled: !supports_personality,
                    actions,
                    dismiss_on_select: true,
                    ..Default::default()
                }
            })
            .collect();

        let mut header = ColumnRenderable::new();
        header.push(Line::from("Select Personality".bold()));
        header.push(Line::from("Choose a communication style for Ody.".dim()));

        self.bottom_pane.show_selection_view(SelectionViewParams {
            header: Box::new(header),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            ..Default::default()
        });
    }

    pub(crate) fn open_experimental_popup(&mut self) {
        let features: Vec<ExperimentalFeatureItem> = FEATURES
            .iter()
            .filter_map(|spec| {
                let name = spec.stage.experimental_menu_name()?;
                let description = spec.stage.experimental_menu_description()?;
                Some(ExperimentalFeatureItem {
                    feature: spec.id,
                    name: name.to_string(),
                    description: description.to_string(),
                    enabled: self.config.features.enabled(spec.id),
                })
            })
            .collect();

        let view = ExperimentalFeaturesView::new(
            features,
            self.app_event_tx.clone(),
            self.bottom_pane.list_keymap(),
        );
        self.bottom_pane.show_view(Box::new(view));
    }

    pub(crate) fn open_preferences_popup(&mut self) {
        let mode = self.active_mode_kind();
        let content = match mode {
            ModeKind::Design => PreferencesContent::Design {
                state: DesignReviewEditState::from_config(&self.config),
            },
            _ => PreferencesContent::Placeholder,
        };
        let view = PreferencesView::new(
            mode,
            content,
            self.app_event_tx.clone(),
            self.bottom_pane.list_keymap(),
        );
        self.bottom_pane.show_view(Box::new(view));
    }

    pub(crate) fn open_websearch_popup(&mut self) {
        let current_provider = self
            .config
            .services
            .as_ref()
            .and_then(|s| s.web_search.as_ref())
            .map(|w| w.primary);

        let providers = [
            WebSearchProviderName::Duckduckgo,
            WebSearchProviderName::Bing,
            WebSearchProviderName::Serpapi,
            WebSearchProviderName::Searchapi,
            WebSearchProviderName::Serper,
            WebSearchProviderName::Baidu,
            WebSearchProviderName::Serply,
            WebSearchProviderName::Searxng,
            WebSearchProviderName::Tavily,
            WebSearchProviderName::Exa,
            WebSearchProviderName::Perplexity,
            WebSearchProviderName::Moonshot,
        ];

        let items: Vec<SelectionItem> = providers
            .into_iter()
            .map(|provider| {
                let name = provider.to_string();
                let description = Some(Self::websearch_provider_description(provider).to_string());
                let provider_for_enter = provider;
                let provider_for_tab = provider;
                let is_current = current_provider == Some(provider);
                SelectionItem {
                    name: name.clone(),
                    description,
                    is_current,
                    actions: vec![Box::new(move |tx| {
                        tx.send(AppEvent::SwitchWebSearchProvider {
                            provider: provider_for_enter,
                        });
                    })],
                    secondary_actions: vec![Box::new(move |tx| {
                        tx.send(AppEvent::WebSearchProviderSelected {
                            provider: provider_for_tab,
                        });
                    })],
                    dismiss_on_select: true,
                    ..Default::default()
                }
            })
            .collect();

        let mut header = ColumnRenderable::new();
        header.push(Line::from("Configure Web Search".bold()));
        header.push(Line::from("Choose a search provider. Press Enter to switch, Tab to configure.".dim()));

        self.bottom_pane.show_selection_view(SelectionViewParams {
            header: Box::new(header),
            footer_hint: Some(Line::from(vec![
                "Press ".into(),
                crate::key_hint::plain(KeyCode::Enter).into(),
                " to switch provider; ".into(),
                crate::key_hint::plain(KeyCode::Tab).into(),
                " to configure; ".into(),
                crate::key_hint::plain(KeyCode::Esc).into(),
                " to cancel".into(),
            ])),
            items,
            ..Default::default()
        });
        self.request_redraw();
    }

    pub(crate) fn on_websearch_provider_selected(&mut self, provider: WebSearchProviderName) {
        let current = self
            .config
            .services
            .as_ref()
            .and_then(|s| s.web_search.as_ref())
            .and_then(|w| w.provider_config(provider));
        let view = crate::bottom_pane::WebSearchProviderConfigView::new(
            provider,
            current,
            self.app_event_tx.clone(),
            self.bottom_pane.list_keymap(),
        );
        self.bottom_pane.push_view(Box::new(view));
        self.request_redraw();
    }

    fn websearch_provider_description(provider: WebSearchProviderName) -> &'static str {
        match provider {
            WebSearchProviderName::Duckduckgo => "Free, no API key required",
            WebSearchProviderName::Bing => "Microsoft Bing Web Search API",
            WebSearchProviderName::Serpapi => "SerpApi Google search results",
            WebSearchProviderName::Searchapi => "SearchAPI.io Google search results",
            WebSearchProviderName::Serper => "Serper.dev Google search results",
            WebSearchProviderName::Baidu => "Baidu search",
            WebSearchProviderName::Serply => "Serply.io search",
            WebSearchProviderName::Searxng => "Self-hosted SearXNG instance (no API key)",
            WebSearchProviderName::Tavily => "Tavily AI search",
            WebSearchProviderName::Exa => "Exa.ai neural search",
            WebSearchProviderName::Perplexity => "Perplexity Sonar API",
            WebSearchProviderName::Moonshot => "Moonshot Kimi search",
        }
    }

    fn personality_label(personality: Personality) -> &'static str {
        match personality {
            Personality::None => "None",
            Personality::Friendly => "Friendly",
            Personality::Pragmatic => "Pragmatic",
        }
    }

    fn personality_description(personality: Personality) -> &'static str {
        match personality {
            Personality::None => "No personality instructions.",
            Personality::Friendly => "Warm, collaborative, and helpful.",
            Personality::Pragmatic => "Concise, task-focused, and direct.",
        }
    }

    pub(crate) fn open_database_connections_popup(&mut self) {
        let config = self
            .config
            .services
            .as_ref()
            .and_then(|s| s.database.clone())
            .unwrap_or_else(|| DatabaseConfig {
                primary: String::new(),
                connections: std::collections::HashMap::new(),
            });
        let view = crate::bottom_pane::DatabaseConnectionsView::new(
            config,
            self.app_event_tx.clone(),
            self.bottom_pane.list_keymap(),
        );
        self.bottom_pane.show_view(Box::new(view));
        self.request_redraw();
    }

    pub(crate) fn open_database_add_connection_form(
        &mut self,
        provider: DatabaseProviderName,
    ) {
        self.open_database_connection_form(provider, None);
    }

    pub(crate) fn open_database_connection_form_for_edit(&mut self, name: String) {
        let provider = self
            .config
            .services
            .as_ref()
            .and_then(|s| s.database.as_ref())
            .and_then(|d| d.connections.get(&name))
            .map(|c| c.provider)
            .unwrap_or(DatabaseProviderName::Postgres);
        self.open_database_connection_form(provider, Some(name));
    }

    pub(crate) fn open_database_connection_form(
        &mut self,
        provider: DatabaseProviderName,
        existing_name: Option<String>,
    ) {
        let config = self
            .config
            .services
            .as_ref()
            .and_then(|s| s.database.as_ref());
        let current = existing_name.as_ref().and_then(|name| {
            config
                .and_then(|d| d.connections.get(name))
                .cloned()
        });
        let view = crate::bottom_pane::DatabaseConnectionFormView::new(
            provider,
            existing_name,
            current,
            self.app_event_tx.clone(),
            self.bottom_pane.list_keymap(),
        );
        self.bottom_pane.push_view(Box::new(view));
        self.request_redraw();
    }
}
