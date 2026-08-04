//! Cross-module security and observability integration tests for the browser
//! control layer.
//!
//! These tests act as a living security audit checklist: they verify that the
//! approval gate, expression fast-reject, raw CDP blocklist, and network
//! redaction layers compose correctly at the tool and module boundaries.

use std::collections::HashMap;
use std::sync::Arc;

use ody_browser_control::{
    is_approval_exempt, BrowserControlConfig, BrowserControlMode, BrowserThreadState, NetworkEntry,
};
use ody_browser_control::{all_tools, check_js_allowed, expression_preview};
use ody_browser_control::network_redaction::redact_network_entry;
use ody_browser_control::raw_cdp_blocklist::is_raw_cdp_blocked;
use ody_protocol::protocol::TruncationPolicy;
use ody_tools::{
    FunctionCallError, NoopTurnItemEmitter, ToolCall, ToolExecutor, ToolName, ToolPayload,
};

fn make_tool_call(
    tool_name: ToolName,
    arguments: &str,
    guardian_approved_action_id: Option<String>,
) -> ToolCall {
    ToolCall {
        turn_id: "turn-1".to_string(),
        call_id: "call-1".to_string(),
        tool_name,
        model: "test".to_string(),
        truncation_policy: TruncationPolicy::Bytes(1),
        conversation_history: Default::default(),
        turn_item_emitter: Arc::new(NoopTurnItemEmitter),
        environments: Vec::new(),
        payload: ToolPayload::Function {
            arguments: arguments.to_string(),
        },
        guardian_approved_action_id,
    }
}

fn uninitialized_state() -> Arc<BrowserThreadState> {
    Arc::new(
        BrowserThreadState::new_uninitialized_for_test(BrowserControlConfig::default())
            .expect("uninitialized state"),
    )
}

#[test]
fn approval_exemption_checklist() {
    let _cwd = std::path::PathBuf::from("/tmp/workspace");

    // Loopback hosts are exempt from the guardian approval gate.
    assert!(is_approval_exempt("http://localhost:8080", None, false).is_some());
    assert!(is_approval_exempt("http://127.0.0.1/path", None, false).is_some());
    assert!(is_approval_exempt("http://[::1]:9222/path", None, false).is_some());

    // Public URLs are never exempt.
    assert!(is_approval_exempt("https://example.com", None, false).is_none());

    // Short data URIs are exempt, long ones are not.
    assert!(is_approval_exempt("data:text/html,<h1>hi</h1>", None, false).is_some());
    let long = format!("data:text/html,{}", "a".repeat(1025));
    assert!(is_approval_exempt(&long, None, false).is_none());

    // Test builds are always exempt to avoid blocking CI/unit paths.
    assert!(is_approval_exempt("https://example.com", None, true).is_some());

    // File URLs are only exempt when inside the thread cwd.
    #[cfg(not(windows))]
    {
        assert!(
            is_approval_exempt("file:///tmp/workspace/index.html", Some(&_cwd), false).is_some()
        );
        assert!(is_approval_exempt("file:///etc/passwd", Some(&_cwd), false).is_none());
    }
}

#[test]
fn js_expression_fast_reject_checklist() {
    // Reading cookies or storage is rejected at the tool layer before guardian.
    assert!(check_js_allowed("document.cookie").is_err());
    assert!(check_js_allowed("window.localStorage.getItem('x')").is_err());
    assert!(check_js_allowed("sessionStorage['x']").is_err());
    assert!(check_js_allowed("indexedDB.open('db')").is_err());

    // Common obfuscation vectors are caught as a fast-reject signal.
    assert!(check_js_allowed("eval('document.cookie')").is_err());
    assert!(check_js_allowed("setTimeout('document.cookie', 0)").is_err());
    assert!(check_js_allowed("iframe.contentWindow.document.cookie").is_err());
    assert!(check_js_allowed("atob('ZG9jdW1lbnQuY29va2ll')").is_err());

    // Benign DOM reads remain allowed.
    assert!(check_js_allowed("document.querySelector('h1').innerText").is_ok());
    assert!(check_js_allowed("window.location.href").is_ok());
    assert!(check_js_allowed("1 + 1").is_ok());
}

#[test]
fn expression_preview_truncates_for_logging() {
    assert_eq!(expression_preview("1 + 1"), "1 + 1");
    let long = "a".repeat(500);
    let preview = expression_preview(&long);
    assert!(preview.len() < 300);
    assert!(preview.contains("truncated"));
}

#[test]
fn raw_cdp_blocklist_checklist() {
    // Cookie, storage, and fetch interception methods are blocked.
    assert!(is_raw_cdp_blocked("Storage.getCookies").is_some());
    assert!(is_raw_cdp_blocked("Storage.setCookies").is_some());
    assert!(is_raw_cdp_blocked("Network.getAllCookies").is_some());
    assert!(is_raw_cdp_blocked("Fetch.continueRequest").is_some());

    // Any method containing "getCookies" is blocked by the wildcard rule.
    assert!(is_raw_cdp_blocked("SomeDomain.getCookies").is_some());

    // Safe methods are allowed through the blocklist.
    assert!(is_raw_cdp_blocked("Runtime.evaluate").is_none());
    assert!(is_raw_cdp_blocked("DOM.querySelector").is_none());
}

#[test]
fn network_redaction_checklist() {
    let mut entry = NetworkEntry {
        request_id: "req-1".to_string(),
        url: "https://example.com/api".to_string(),
        method: Some("POST".to_string()),
        status: Some(200),
        status_text: Some("OK".to_string()),
        resource_type: Some("xhr".to_string()),
        timestamp: 0.0,
        request_body: Some("a".repeat(2000)),
        response_body: Some("secret response body".to_string()),
        request_headers: Some(HashMap::from([
            ("Authorization".to_string(), "Bearer token".to_string()),
            ("Accept".to_string(), "application/json".to_string()),
        ])),
        response_headers: Some(HashMap::from([
            ("Set-Cookie".to_string(), "session=abc".to_string()),
            ("Content-Type".to_string(), "application/json".to_string()),
        ])),
        request_headers_size: 0,
        response_headers_size: 0,
        from_cache: None,
    };

    redact_network_entry(&mut entry);

    let req = entry.request_headers.unwrap();
    assert_eq!(req.get("Authorization").unwrap(), "[REDACTED]");
    assert_eq!(req.get("Accept").unwrap(), "application/json");
    assert_eq!(entry.request_body, Some("[truncated]".to_string()));

    let res = entry.response_headers.unwrap();
    assert_eq!(res.get("Set-Cookie").unwrap(), "[REDACTED]");
    assert_eq!(res.get("Content-Type").unwrap(), "application/json");
    assert!(entry.response_body.is_none());
}

#[tokio::test]
async fn navigate_loopback_is_exempt_from_approval() {
    let state = uninitialized_state();
    let tool = all_tools(state)
        .into_iter()
        .find(|t| t.tool_name().name == "navigate")
        .expect("navigate tool exists");
    let call = make_tool_call(
        ToolName::namespaced("browser", "navigate"),
        r#"{"url": "http://localhost:8080/index.html"}"#,
        None,
    );
    let result = ToolExecutor::handle(&*tool, call).await;
    let Err(err) = result else {
        panic!("expected an error from uninitialized state");
    };
    assert!(
        !matches!(err, FunctionCallError::NeedsApproval { .. }),
        "loopback navigate should be exempt from approval: {err:?}"
    );
}

#[tokio::test]
async fn evaluate_rejects_cookie_expression_before_approval() {
    let state = uninitialized_state();
    let tool = all_tools(state)
        .into_iter()
        .find(|t| t.tool_name().name == "evaluate")
        .expect("evaluate tool exists");
    let call = make_tool_call(
        ToolName::namespaced("browser", "evaluate"),
        r#"{"expression": "document.cookie"}"#,
        None,
    );
    let result = ToolExecutor::handle(&*tool, call).await;
    let Err(err) = result else {
        panic!("expected an error");
    };
    assert!(
        !matches!(err, FunctionCallError::NeedsApproval { .. }),
        "forbidden expression should be rejected before approval: {err:?}"
    );
    assert!(err.to_string().contains("document.cookie"));
}

#[tokio::test]
async fn evaluate_approval_ticket_truncates_long_expression() {
    let state = uninitialized_state();
    let tool = all_tools(state)
        .into_iter()
        .find(|t| t.tool_name().name == "evaluate")
        .expect("evaluate tool exists");
    let long = "a".repeat(1000);
    let arguments = format!(r#"{{"expression": "{long}"}}"#);
    let call = make_tool_call(
        ToolName::namespaced("browser", "evaluate"),
        &arguments,
        None,
    );
    let result = ToolExecutor::handle(&*tool, call).await;
    let Err(err) = result else {
        panic!("expected an error");
    };
    let FunctionCallError::NeedsApproval { ticket } = err else {
        panic!("expected NeedsApproval, got {err:?}");
    };
    let details = ticket.get("details").expect("details");
    let expr = details
        .get("expression")
        .and_then(serde_json::Value::as_str)
        .expect("expression");
    assert!(expr.len() <= 600, "expression should be truncated: {expr}");
    assert_eq!(
        details.get("expression_truncated").and_then(serde_json::Value::as_bool),
        Some(true)
    );
}

#[tokio::test]
async fn raw_cdp_blocked_method_is_rejected_before_approval() {
    let state = uninitialized_state();
    let tool = all_tools(state)
        .into_iter()
        .find(|t| t.tool_name().name == "execute_raw_cdp")
        .expect("execute_raw_cdp tool exists");
    let call = make_tool_call(
        ToolName::namespaced("browser", "execute_raw_cdp"),
        r#"{"method": "Storage.getCookies", "params": {}}"#,
        None,
    );
    let result = ToolExecutor::handle(&*tool, call).await;
    let Err(err) = result else {
        panic!("expected an error");
    };
    assert!(
        !matches!(err, FunctionCallError::NeedsApproval { .. }),
        "blocked raw CDP should be rejected before approval: {err:?}"
    );
    assert!(err.to_string().contains("Storage.getCookies"));
}

#[tokio::test]
async fn raw_cdp_is_disabled_in_external_mode() {
    let mut config = BrowserControlConfig::default();
    config.mode = BrowserControlMode::External;
    let state = Arc::new(
        BrowserThreadState::new_uninitialized_for_test(config).expect("uninitialized state"),
    );
    let tool = all_tools(state)
        .into_iter()
        .find(|t| t.tool_name().name == "execute_raw_cdp")
        .expect("execute_raw_cdp tool exists");
    let call = make_tool_call(
        ToolName::namespaced("browser", "execute_raw_cdp"),
        r#"{"method": "Runtime.evaluate", "params": {}}"#,
        None,
    );
    let result = ToolExecutor::handle(&*tool, call).await;
    let Err(err) = result else {
        panic!("expected an error");
    };
    assert!(err.to_string().contains("external browser mode"));
}

#[tokio::test]
async fn read_only_screenshot_does_not_require_approval() {
    let state = uninitialized_state();
    let tool = all_tools(state)
        .into_iter()
        .find(|t| t.tool_name().name == "screenshot")
        .expect("screenshot tool exists");
    let call = make_tool_call(
        ToolName::namespaced("browser", "screenshot"),
        r#"{"full_page": true}"#,
        None,
    );
    let result = ToolExecutor::handle(&*tool, call).await;
    let Err(err) = result else {
        panic!("expected an error from uninitialized state");
    };
    assert!(
        !matches!(err, FunctionCallError::NeedsApproval { .. }),
        "screenshot should not require approval: {err:?}"
    );
}

#[test]
fn all_tools_exposes_expected_surface() {
    let state = uninitialized_state();
    let names: Vec<String> = all_tools(state)
        .iter()
        .map(|t| t.tool_name().name.clone())
        .collect();
    assert!(names.contains(&"navigate".to_string()));
    assert!(names.contains(&"evaluate".to_string()));
    assert!(names.contains(&"screenshot".to_string()));
    assert!(names.contains(&"execute_raw_cdp".to_string()));
    assert!(names.contains(&"read_logs".to_string()));
}
