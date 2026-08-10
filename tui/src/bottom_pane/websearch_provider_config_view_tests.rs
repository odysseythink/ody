use super::*;
use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::bottom_pane_view::BottomPaneView;
use crate::keymap::RuntimeKeymap;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use ody_web_search::config::WebSearchProviderConfig;
use ody_web_search::config::WebSearchProviderName;
use serde_json::json;
use std::collections::HashMap;
use tokio::sync::mpsc::unbounded_channel;

fn make_view(
    provider: WebSearchProviderName,
    current: Option<WebSearchProviderConfig>,
) -> (
    WebSearchProviderConfigView,
    tokio::sync::mpsc::UnboundedReceiver<AppEvent>,
) {
    let (tx, rx) = unbounded_channel::<AppEvent>();
    let view = WebSearchProviderConfigView::new(
        provider,
        current.as_ref(),
        AppEventSender::new(tx),
        RuntimeKeymap::defaults().list,
    );
    (view, rx)
}

fn press_enter(view: &mut WebSearchProviderConfigView) {
    view.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
}

fn press_esc(view: &mut WebSearchProviderConfigView) {
    view.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
}

#[test]
fn every_provider_renders_config_fields_and_save_action() {
    for provider in [
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
    ] {
        let (view, _rx) = make_view(provider, None);
        let rows = view.build_rows();
        let expected_count = view.fields.len() + 1;
        assert_eq!(
            rows.len(),
            expected_count,
            "{provider}: expected {expected_count} rows (fields + save), got {}",
            rows.len()
        );

        for field in &view.fields {
            assert!(
                rows.iter().any(|r| r.name.contains(field.label)),
                "{provider}: missing row for field {}",
                field.label
            );
        }
        assert!(
            rows.iter().any(|r| r.name.contains("Save configuration")),
            "{provider}: missing save action row"
        );
    }
}

#[test]
fn prefill_existing_config_for_matching_provider() {
    let current = WebSearchProviderConfig {
        provider: WebSearchProviderName::Bing,
        api_key: Some("secret-key".to_string()),
        timeout_ms: Some(12345),
        options: {
            let mut m = HashMap::new();
            m.insert("base_url".to_string(), json!("https://bing.example"));
            m
        },
    };
    let (view, _rx) = make_view(WebSearchProviderName::Bing, Some(current));

    assert_eq!(view.values.get("api_key"), Some(&"secret-key".to_string()));
    assert_eq!(view.values.get("timeout_ms"), Some(&"12345".to_string()));
    assert_eq!(
        view.values.get("base_url"),
        Some(&"https://bing.example".to_string())
    );

    let rows = view.build_rows();
    assert!(
        rows.iter().any(|r| r.name.contains("API key: ••••••")),
        "expected API key to be masked, got rows: {:?}",
        rows.iter().map(|r| r.name.clone()).collect::<Vec<_>>()
    );
    assert!(rows.iter().any(|r| r.name.contains("Timeout (ms): 12345")));
    assert!(rows
        .iter()
        .any(|r| r.name.contains("Base URL: https://bing.example")));
}

#[test]
fn prefill_ignores_config_from_different_provider() {
    let current = WebSearchProviderConfig {
        provider: WebSearchProviderName::Duckduckgo,
        api_key: None,
        timeout_ms: Some(9999),
        options: {
            let mut m = HashMap::new();
            m.insert("proxy_url".to_string(), json!("http://proxy"));
            m
        },
    };
    let (view, _rx) = make_view(WebSearchProviderName::Bing, Some(current));

    assert_eq!(view.values.get("api_key"), Some(&"".to_string()));
    assert_eq!(view.values.get("timeout_ms"), Some(&"".to_string()));
    assert_eq!(view.values.get("base_url"), Some(&"".to_string()));
}

#[test]
fn saving_valid_config_emits_persist_event() {
    let (mut view, mut rx) = make_view(WebSearchProviderName::Searxng, None);
    view.values.insert(
        "base_url".to_string(),
        "https://searx.example/search".to_string(),
    );
    view.values.insert("timeout_ms".to_string(), "5000".to_string());
    // Selected row starts at timeout_ms (0). Move to base_url (1), then save (2).
    view.move_down();
    view.move_down();
    press_enter(&mut view);

    match rx.try_recv() {
        Ok(AppEvent::PersistWebSearchProviderConfig { config }) => {
            assert_eq!(config.provider, WebSearchProviderName::Searxng);
            assert_eq!(config.api_key, None);
            assert_eq!(config.timeout_ms, Some(5000));
            assert_eq!(
                config.options.get("base_url"),
                Some(&json!("https://searx.example/search"))
            );
        }
        other => panic!("expected PersistWebSearchProviderConfig, got {other:?}"),
    }
    assert!(rx.try_recv().is_err());
}

#[test]
fn missing_required_field_shows_error() {
    let (mut view, mut rx) = make_view(WebSearchProviderName::Searxng, None);
    // Move from timeout_ms (0) -> base_url (1) -> save (2).
    view.move_down();
    view.move_down();
    press_enter(&mut view);

    assert!(
        view.error_message
            .as_ref()
            .unwrap()
            .contains("Base URL is required"),
        "expected required-field error, got {:?}",
        view.error_message
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn invalid_timeout_format_shows_error() {
    let (mut view, mut rx) = make_view(WebSearchProviderName::Bing, None);
    view.values
        .insert("timeout_ms".to_string(), "not-a-number".to_string());
    // Bing rows: api_key (0), timeout_ms (1), base_url (2), save (3).
    for _ in 0..3 {
        view.move_down();
    }
    press_enter(&mut view);

    assert!(
        view.error_message
            .as_ref()
            .unwrap()
            .contains("Timeout (ms) must be a number"),
        "expected number-format error, got {:?}",
        view.error_message
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn invalid_timeout_range_shows_error() {
    let (mut view, mut rx) = make_view(WebSearchProviderName::Bing, None);
    view.values.insert("timeout_ms".to_string(), "500".to_string());
    for _ in 0..3 {
        view.move_down();
    }
    press_enter(&mut view);

    assert!(
        view.error_message
            .as_ref()
            .unwrap()
            .contains("between 1000 and 300000"),
        "expected range error, got {:?}",
        view.error_message
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn invalid_choice_shows_error() {
    let (mut view, mut rx) = make_view(WebSearchProviderName::Serply, None);
    view.values
        .insert("device".to_string(), "watch".to_string());
    // Serply rows: api_key, timeout, base_url, language, hl, gl, device, save.
    for _ in 0..7 {
        view.move_down();
    }
    press_enter(&mut view);

    assert!(
        view.error_message
            .as_ref()
            .unwrap()
            .contains("Device must be one of: desktop, mobile, tablet"),
        "expected choice validation error, got {:?}",
        view.error_message
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn cancel_closes_form_without_event() {
    let (mut view, mut rx) = make_view(WebSearchProviderName::Bing, None);
    press_esc(&mut view);

    assert!(view.is_complete());
    assert!(rx.try_recv().is_err());
}

#[test]
fn empty_api_key_is_not_persisted() {
    let (mut view, mut rx) = make_view(WebSearchProviderName::Bing, None);
    view.values.insert("timeout_ms".to_string(), "10000".to_string());
    // Bing rows: api_key (0), timeout_ms (1), base_url (2), save (3).
    for _ in 0..3 {
        view.move_down();
    }
    press_enter(&mut view);

    match rx.try_recv() {
        Ok(AppEvent::PersistWebSearchProviderConfig { config }) => {
            assert_eq!(config.api_key, None);
            assert_eq!(config.timeout_ms, Some(10000));
        }
        other => panic!("expected PersistWebSearchProviderConfig, got {other:?}"),
    }
}

#[test]
fn clear_optional_value_drops_it_from_config() {
    let current = WebSearchProviderConfig {
        provider: WebSearchProviderName::Bing,
        api_key: Some("key".to_string()),
        timeout_ms: Some(10000),
        options: {
            let mut m = HashMap::new();
            m.insert("base_url".to_string(), json!("https://old.example"));
            m
        },
    };
    let (mut view, mut rx) = make_view(WebSearchProviderName::Bing, Some(current));
    // Clear the optional base_url value and save.
    view.values.insert("base_url".to_string(), String::new());
    for _ in 0..3 {
        view.move_down();
    }
    press_enter(&mut view);

    match rx.try_recv() {
        Ok(AppEvent::PersistWebSearchProviderConfig { config }) => {
            assert_eq!(config.options.get("base_url"), None);
            assert_eq!(config.api_key, Some("key".to_string()));
            assert_eq!(config.timeout_ms, Some(10000));
        }
        other => panic!("expected PersistWebSearchProviderConfig, got {other:?}"),
    }
}
