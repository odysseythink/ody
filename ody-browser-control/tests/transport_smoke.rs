use ody_browser_control::{
    BrowserControlApprovalTicket, BrowserControlConfig, BrowserControlError, BrowserControlMode,
};

#[test]
fn config_round_trips_through_json() {
    let cfg = BrowserControlConfig {
        mode: BrowserControlMode::External,
        chrome_path: Some(std::path::PathBuf::from("/usr/bin/chrome")),
        launch_args: vec!["--window-size=1280,720".to_string()],
        max_concurrent_browsers: 8,
        command_timeout_ms: 15_000,
        navigation_timeout_ms: 30_000,
        connect_timeout_ms: 5_000,
        connect_url: Some("ws://localhost:9222".to_string()),
        allow_local_network: true,
        external_browser_allow_sensitive: true,
        max_console_message_bytes: 8192,
        max_event_entries: 500,
        max_event_buffer_bytes: 512 * 1024,
    };
    let json = serde_json::to_string(&cfg).unwrap();
    let back: BrowserControlConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(back.mode, BrowserControlMode::External);
    assert_eq!(back.max_concurrent_browsers, 8);
    assert_eq!(back.max_event_entries, 500);
}

#[test]
fn error_retryable_classification() {
    assert!(
        BrowserControlError::Timeout {
            command: "navigate".to_string(),
            elapsed_ms: 100,
        }
        .is_retryable()
    );
    assert!(
        !BrowserControlError::NotAllowed {
            reason: "external browser".to_string(),
        }
        .is_retryable()
    );
}

#[test]
fn approval_ticket_serializes_action_and_details() {
    let ticket = BrowserControlApprovalTicket {
        action: "evaluate".to_string(),
        details: serde_json::json!({"expression": "document.title"}),
    };
    let json = serde_json::to_value(&ticket).unwrap();
    assert_eq!(json["action"], "evaluate");
    assert_eq!(json["details"]["expression"], "document.title");
}

#[test]
fn sanitize_args_strips_dangerous_flags() {
    let cfg = BrowserControlConfig {
        launch_args: vec![
            "--window-size=1280,720".to_string(),
            "--user-data-dir=/evil".to_string(),
            "--remote-debugging-port=9222".to_string(),
        ],
        ..Default::default()
    };
    let sanitized = cfg.sanitize_args();
    assert!(sanitized.contains(&"--window-size=1280,720".to_string()));
    assert!(!sanitized.iter().any(|a| a.starts_with("--user-data-dir")));
    assert!(!sanitized.iter().any(|a| a.starts_with("--remote-debugging-port")));
}

#[test]
fn build_launch_args_includes_user_data_dir() {
    let cfg = BrowserControlConfig::default();
    let dir = std::env::temp_dir();
    let args = cfg.build_launch_args(&dir);
    assert!(args.iter().any(|a| a.starts_with("--user-data-dir=")));
    assert!(args.iter().any(|a| a == "--no-first-run"));
}

#[test]
fn default_config_has_expected_quotas() {
    let cfg = BrowserControlConfig::default();
    assert_eq!(cfg.max_concurrent_browsers, 4);
    assert_eq!(cfg.max_event_entries, 1000);
    assert_eq!(cfg.max_event_buffer_bytes, 1024 * 1024);
    assert_eq!(cfg.max_console_message_bytes, 4096);
}
