//! Tests for the interactive TUI `/login` flow.

use std::time::Duration;

use assert_matches::assert_matches;
use crossterm::event::{KeyCode, KeyEvent};
use ody_model_provider::login::LoginModelInfo;
use ody_model_provider_info::LoginProvider;
use tokio::time::timeout;

use super::*;
use crate::chatwidget::tests::drain_insert_history;
use crate::chatwidget::tests::make_chatwidget_manual;

fn type_text(chat: &mut ChatWidget, text: &str) {
    chat.handle_paste(text.to_string());
}

fn submit_active_prompt(chat: &mut ChatWidget) {
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));
}

#[tokio::test]
async fn start_login_flow_with_provider_shows_prompt() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(None).await;

    chat.start_login_flow(Some(LoginProvider::Kimi));

    assert!(
        !chat.bottom_pane.no_modal_or_popup_active(),
        "expected a prompt to be active"
    );
}

#[tokio::test]
async fn start_login_flow_without_provider_shows_provider_picker() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(None).await;

    chat.start_login_flow(None);

    assert!(
        !chat.bottom_pane.no_modal_or_popup_active(),
        "expected provider picker to be active"
    );
}

#[tokio::test]
async fn alias_prompt_submits_login_alias_event() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(None).await;

    chat.start_login_flow(Some(LoginProvider::Kimi));
    type_text(&mut chat, "my-kimi");
    submit_active_prompt(&mut chat);

    let event = rx.try_recv().expect("expected LoginAliasSubmitted event");
    assert_matches!(
        event,
        AppEvent::LoginAliasSubmitted {
            provider: LoginProvider::Kimi,
            alias,
        } if alias == "my-kimi"
    );
}

#[tokio::test]
async fn invalid_alias_re_prompts_and_adds_error() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(None).await;

    chat.start_login_flow(Some(LoginProvider::Kimi));
    type_text(&mut chat, "kimi");
    submit_active_prompt(&mut chat);

    // The callback tries to send an event even for an invalid alias, but
    // on_login_alias_submitted validates it and re-shows the prompt.
    let _ = rx.try_recv();
    chat.on_login_alias_submitted(LoginProvider::Kimi, "kimi".to_string());

    let cells = drain_insert_history(&mut rx);
    assert!(
        cells.iter().any(|cell| {
            cell.iter()
                .any(|line| line.to_string().contains("reserved"))
        }),
        "expected an error about the reserved alias"
    );
    assert!(
        !chat.bottom_pane.no_modal_or_popup_active(),
        "expected the alias prompt to be re-shown"
    );
}

#[tokio::test]
async fn api_key_prompt_submits_login_api_key_event() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(None).await;

    chat.on_login_alias_submitted(LoginProvider::Kimi, "my-kimi".to_string());
    type_text(&mut chat, "secret-api-key");
    submit_active_prompt(&mut chat);

    let event = rx.try_recv().expect("expected LoginApiKeySubmitted event");
    assert_matches!(
        event,
        AppEvent::LoginApiKeySubmitted {
            provider: LoginProvider::Kimi,
            alias,
            api_key,
        } if alias == "my-kimi" && api_key == "secret-api-key"
    );
}

#[tokio::test]
async fn empty_api_key_re_prompts_and_adds_error() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(None).await;

    chat.on_login_alias_submitted(LoginProvider::Kimi, "my-kimi".to_string());
    submit_active_prompt(&mut chat);

    let _ = rx.try_recv();
    chat.on_login_api_key_submitted(LoginProvider::Kimi, "my-kimi".to_string(), "".to_string());

    let cells = drain_insert_history(&mut rx);
    assert!(
        cells.iter().any(|cell| {
            cell.iter()
                .any(|line| line.to_string().contains("API key cannot be empty"))
        }),
        "expected an empty API key error"
    );
    assert!(
        !chat.bottom_pane.no_modal_or_popup_active(),
        "expected the API key prompt to be re-shown"
    );
}

#[tokio::test]
async fn base_url_prompt_submits_login_base_url_event() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(None).await;

    chat.on_login_api_key_submitted(
        LoginProvider::Kimi,
        "my-kimi".to_string(),
        "secret-api-key".to_string(),
    );
    // Press Enter to accept the default base URL shown in the prompt.
    submit_active_prompt(&mut chat);

    let event = rx.try_recv().expect("expected LoginBaseUrlSubmitted event");
    assert_matches!(
        event,
        AppEvent::LoginBaseUrlSubmitted {
            provider: LoginProvider::Kimi,
            alias,
            api_key,
            base_url,
        } if alias == "my-kimi" && api_key == "secret-api-key" && base_url == LoginProvider::Kimi.default_base_url()
    );
}

#[tokio::test]
async fn on_login_base_url_submitted_spawns_models_fetch() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(None).await;

    chat.on_login_base_url_submitted(
        LoginProvider::Kimi,
        "my-kimi".to_string(),
        "secret-api-key".to_string(),
        "http://127.0.0.1:1".to_string(),
    );

    // The method first emits an info message, then the async fetch emits the result.
    let _ = drain_insert_history(&mut rx);

    let event = timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("timed out waiting for LoginModelsFetched")
        .expect("channel closed");

    assert_matches!(
        event,
        AppEvent::LoginModelsFetched {
            provider: LoginProvider::Kimi,
            alias,
            api_key,
            base_url,
            result: Err(_),
        } if alias == "my-kimi" && api_key == "secret-api-key" && base_url == "http://127.0.0.1:1"
    );
}

#[tokio::test]
async fn on_login_models_fetched_shows_model_picker() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(None).await;

    chat.on_login_models_fetched(
        LoginProvider::Kimi,
        "my-kimi".to_string(),
        "secret-api-key".to_string(),
        "https://api.example.com/v1".to_string(),
        vec![
            LoginModelInfo {
                id: "model-a".to_string(),
                display_name: "Model A".to_string(),
            },
            LoginModelInfo {
                id: "model-b".to_string(),
                display_name: "Model B".to_string(),
            },
        ],
    );

    assert!(
        !chat.bottom_pane.no_modal_or_popup_active(),
        "expected model picker to be active"
    );
}

#[tokio::test]
async fn on_login_models_fetched_empty_returns_to_base_url_prompt() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(None).await;

    chat.on_login_models_fetched(
        LoginProvider::Kimi,
        "my-kimi".to_string(),
        "secret-api-key".to_string(),
        "https://api.example.com/v1".to_string(),
        vec![],
    );

    let cells = drain_insert_history(&mut rx);
    assert!(
        cells.iter().any(|cell| {
            cell.iter()
                .any(|line| line.to_string().contains("No models returned"))
        }),
        "expected a no-models error"
    );
    assert!(
        !chat.bottom_pane.no_modal_or_popup_active(),
        "expected base URL prompt to be re-shown"
    );
}
