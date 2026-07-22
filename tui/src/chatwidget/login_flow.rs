//! Interactive `/login` flow for `ChatWidget`.
//!
//! This module drives the state machine that collects provider, alias, API key,
//! and base URL, verifies the key against the provider's `/models` endpoint,
//! and then lets the user pick a default model. Persistence is delegated to the
//! app layer via `AppEvent`.

use super::*;
use crate::app_event::AppEvent;
use crate::bottom_pane::SelectionItem;
use crate::bottom_pane::SelectionViewParams;
use crate::bottom_pane::custom_prompt_view::CustomPromptView;
use crate::bottom_pane::popup_consts::standard_popup_hint_line;
use crate::login::telemetry;
use crate::login::validation::validate_custom_alias;
use ody_model_provider::login::LoginModelInfo;
use ody_model_provider::login::fetch_login_models;
use ody_model_provider_info::LoginProvider;

const LOGIN_PROVIDER_SELECTION_VIEW_ID: &str = "login-provider-selection";
const LOGIN_MODEL_SELECTION_VIEW_ID: &str = "login-model-selection";

impl ChatWidget {
    pub(crate) fn start_login_flow(&mut self, provider: Option<LoginProvider>) {
        if let Some(provider) = provider {
            telemetry::record_login_attempted(&self.session_telemetry, provider);
            self.show_login_alias_prompt(provider);
        } else {
            self.show_login_provider_picker();
        }
    }

    fn show_login_provider_picker(&mut self) {
        let _tx = self.app_event_tx.clone();
        let items: Vec<SelectionItem> = [
            LoginProvider::Kimi,
            LoginProvider::Deepseek,
            LoginProvider::Glm,
        ]
        .into_iter()
        .map(|provider| {
            let name = provider.display_name().to_string();
            let id = provider.id().to_string();
            let provider_for_action = provider;
            SelectionItem {
                name,
                description: Some(format!("Configure {id} login")),
                actions: vec![Box::new(move |tx| {
                    tx.send(AppEvent::LoginProviderSelected {
                        provider: provider_for_action,
                    });
                })],
                dismiss_on_select: true,
                ..Default::default()
            }
        })
        .collect();

        self.bottom_pane.show_selection_view(SelectionViewParams {
            view_id: Some(LOGIN_PROVIDER_SELECTION_VIEW_ID),
            title: Some("Select provider".to_string()),
            subtitle: Some("Choose an API-key provider to configure".to_string()),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            ..Default::default()
        });
        self.request_redraw();
    }

    pub(crate) fn on_login_provider_selected(&mut self, provider: LoginProvider) {
        telemetry::record_login_attempted(&self.session_telemetry, provider);
        self.show_login_alias_prompt(provider);
    }

    fn show_login_alias_prompt(&mut self, provider: LoginProvider) {
        let provider_name = provider.display_name().to_string();
        let tx = self.app_event_tx.clone();
        let view = CustomPromptView::new(
            format!("Login to {provider_name}"),
            "Enter a custom alias for this provider".to_string(),
            String::new(),
            Some("Aliases cannot be 'kimi', 'deepseek', or 'glm'".to_string()),
            Box::new(move |alias| {
                tx.send(AppEvent::LoginAliasSubmitted { provider, alias });
            }),
        );
        self.bottom_pane.push_view(Box::new(view));
        self.request_redraw();
    }

    pub(crate) fn on_login_alias_submitted(&mut self, provider: LoginProvider, alias: String) {
        if let Err(err) = validate_custom_alias(&alias) {
            self.add_error_message(err);
            self.show_login_alias_prompt(provider);
            return;
        }
        self.show_login_api_key_prompt(provider, alias);
    }

    fn show_login_api_key_prompt(&mut self, provider: LoginProvider, alias: String) {
        let provider_name = provider.display_name().to_string();
        let tx = self.app_event_tx.clone();
        let view = CustomPromptView::new_secret(
            format!("Login to {provider_name}"),
            "Paste your API key".to_string(),
            String::new(),
            Some(format!("Key will be saved in providers.{alias}.api_key")),
            Box::new(move |api_key| {
                let alias = alias.clone();
                tx.send(AppEvent::LoginApiKeySubmitted {
                    provider,
                    alias,
                    api_key,
                });
            }),
        );
        self.bottom_pane.push_view(Box::new(view));
        self.request_redraw();
    }

    pub(crate) fn on_login_api_key_submitted(
        &mut self,
        provider: LoginProvider,
        alias: String,
        api_key: String,
    ) {
        if api_key.trim().is_empty() {
            self.add_error_message("API key cannot be empty".to_string());
            self.show_login_api_key_prompt(provider, alias);
            return;
        }
        self.show_login_base_url_prompt(provider, alias, api_key);
    }

    fn show_login_base_url_prompt(
        &mut self,
        provider: LoginProvider,
        alias: String,
        api_key: String,
    ) {
        let provider_name = provider.display_name().to_string();
        let default_base_url = provider.default_base_url().to_string();
        let tx = self.app_event_tx.clone();
        let view = CustomPromptView::new(
            format!("Login to {provider_name}"),
            "Press Enter to use the default base URL".to_string(),
            default_base_url.clone(),
            Some(format!("Default: {default_base_url}")),
            Box::new(move |base_url| {
                let alias = alias.clone();
                let api_key = api_key.clone();
                tx.send(AppEvent::LoginBaseUrlSubmitted {
                    provider,
                    alias,
                    api_key,
                    base_url,
                });
            }),
        );
        self.bottom_pane.push_view(Box::new(view));
        self.request_redraw();
    }

    pub(crate) fn on_login_base_url_submitted(
        &mut self,
        provider: LoginProvider,
        alias: String,
        api_key: String,
        base_url: String,
    ) {
        let base_url = if base_url.trim().is_empty() {
            provider.default_base_url().to_string()
        } else {
            base_url.trim().to_string()
        };

        let tx = self.app_event_tx.clone();
        tokio::spawn(async move {
            let extra_headers = match provider {
                LoginProvider::Kimi => ody_model_provider_info::create_kimi_provider().http_headers,
                _ => None,
            };
            let result = fetch_login_models(provider, &base_url, &api_key, extra_headers)
                .await
                .map_err(|e| e.to_string());
            tx.send(AppEvent::LoginModelsFetched {
                provider,
                alias,
                api_key,
                base_url,
                result,
            });
        });

        self.add_info_message(
            format!("Verifying {} API key...", provider.display_name()),
            None,
        );
        self.request_redraw();
    }

    pub(crate) fn on_login_models_fetched(
        &mut self,
        provider: LoginProvider,
        alias: String,
        api_key: String,
        base_url: String,
        models: Vec<LoginModelInfo>,
    ) {
        self.last_fetched_login_models = Some((provider, alias.clone(), models.clone()));
        if models.is_empty() {
            self.add_error_message(
                "No models returned by the provider. Please check the base URL.".to_string(),
            );
            self.show_login_base_url_prompt(provider, alias, api_key);
            return;
        }

        let items: Vec<SelectionItem> = models
            .into_iter()
            .map(|model| {
                let model_id = model.id.clone();
                let display_name = model.display_name.clone();
                let provider_for_action = provider;
                let alias_for_action = alias.clone();
                let api_key_for_action = api_key.clone();
                let base_url_for_action = base_url.clone();
                SelectionItem {
                    name: model_id.clone(),
                    description: Some(display_name),
                    actions: vec![Box::new(move |tx| {
                        tx.send(AppEvent::PersistLoginProvider {
                            provider: provider_for_action,
                            alias: alias_for_action.clone(),
                            api_key: api_key_for_action.clone(),
                            base_url: base_url_for_action.clone(),
                            model_id: model_id.clone(),
                        });
                    })],
                    dismiss_on_select: true,
                    ..Default::default()
                }
            })
            .collect();

        self.bottom_pane.show_selection_view(SelectionViewParams {
            view_id: Some(LOGIN_MODEL_SELECTION_VIEW_ID),
            title: Some("Select default model".to_string()),
            subtitle: Some(format!("Choose a model for {}", provider.display_name())),
            footer_hint: Some(standard_popup_hint_line()),
            items,
            ..Default::default()
        });
        self.request_redraw();
    }
}
#[cfg(test)]
#[path = "login_flow_tests.rs"]
mod tests;
