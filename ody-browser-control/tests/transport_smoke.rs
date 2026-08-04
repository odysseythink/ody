use ody_browser_control::{
    BrowserControlApprovalTicket, BrowserControlConfig, BrowserControlError, BrowserControlMode,
    ViewportConfig,
};

#[test]
fn config_round_trips_through_json() {
    let cfg = BrowserControlConfig {
        mode: BrowserControlMode::External,
        chrome_executable: Some(std::path::PathBuf::from("/usr/bin/chrome")),
        headless: false,
        viewport: ViewportConfig {
            width: 1920,
            height: 1080,
            device_scale_factor: Some(1.0),
        },
        sandbox: false,
        disable_extensions: true,
        extra_args: vec!["--window-size=1280,720".to_string()],
        max_concurrent_browsers: 8,
        browser_permit_acquire_timeout_ms: 60_000,
        command_timeout_ms: 15_000,
        navigation_timeout_ms: 30_000,
        launch_timeout_ms: 5_000,
        connect_timeout_ms: 10_000,
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
    assert_eq!(back.chrome_executable, cfg.chrome_executable);
    assert!(!back.headless);
    assert_eq!(back.viewport.width, 1920);
    assert_eq!(back.sandbox, false);
    assert!(back.disable_extensions);
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
    assert!(BrowserControlError::QuotaExceeded {
        reason: "full".to_string(),
    }
    .is_retryable());
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
        extra_args: vec![
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
fn build_launch_args_includes_defaults() {
    let cfg = BrowserControlConfig::default();
    let args = cfg.build_launch_args();
    assert!(args.iter().any(|a| a == "--no-first-run"));
    assert!(args.iter().any(|a| a == "--disable-extensions"));
    assert!(args.iter().any(|a| a == "--disable-features=Translate"));
    // user-data-dir is managed by the session layer, not the config args.
    assert!(!args.iter().any(|a| a.starts_with("--user-data-dir")));
    assert!(!args.iter().any(|a| a.starts_with("--remote-debugging-port")));
}

#[test]
fn default_config_has_expected_quotas() {
    let cfg = BrowserControlConfig::default();
    assert_eq!(cfg.max_concurrent_browsers, 4);
    assert_eq!(cfg.browser_permit_acquire_timeout_ms, 30_000);
    assert_eq!(cfg.launch_timeout_ms, 20_000);
    assert_eq!(cfg.connect_timeout_ms, 10_000);
    assert_eq!(cfg.max_event_entries, 1000);
    assert_eq!(cfg.max_event_buffer_bytes, 1024 * 1024);
    assert_eq!(cfg.max_console_message_bytes, 4096);
}

#[test]
fn viewport_config_has_defaults() {
    let vp = ViewportConfig::default();
    assert_eq!(vp.width, 1280);
    assert_eq!(vp.height, 720);
    assert_eq!(vp.device_scale_factor, None);
}

#[test]
fn viewport_converts_to_chromiumoxide_viewport() {
    let vp = ViewportConfig {
        width: 1920,
        height: 1080,
        device_scale_factor: Some(2.0),
    };
    let cdp: chromiumoxide::handler::viewport::Viewport = vp.into();
    assert_eq!(cdp.width, 1920);
    assert_eq!(cdp.height, 1080);
    assert_eq!(cdp.device_scale_factor, Some(2.0));
}
