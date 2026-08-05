use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use ody_browser_control::{
    discover_chrome, BrowserControlConfig, BrowserControlMode, BrowserSession,
};

fn test_config() -> BrowserControlConfig {
    BrowserControlConfig {
        headless: true,
        sandbox: false,
        disable_extensions: true,
        launch_timeout_ms: 30_000,
        command_timeout_ms: 30_000,
        navigation_timeout_ms: 30_000,
        connect_timeout_ms: 500,
        allow_local_network: true,
        ..BrowserControlConfig::default()
    }
}

fn skip_if_no_chrome() -> bool {
    if discover_chrome(&BrowserControlConfig::default()).is_err() {
        eprintln!("skipping integration test: no Chrome executable found");
        return true;
    }
    false
}

async fn start_test_server_with_body(body: &'static [u8]) -> (tokio::task::JoinHandle<()>, String) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.write_all(body).await;
            let _ = socket.shutdown().await;
        }
    });
    (handle, format!("http://127.0.0.1:{port}"))
}

async fn start_test_server() -> (tokio::task::JoinHandle<()>, String) {
    let body = b"<html><head><title>end-to-end</title></head><body><div id=\"result\">ODY</div></body></html>";
    start_test_server_with_body(body).await
}

/// Full chain: navigate to a real local server, evaluate JS, take a screenshot.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn navigate_evaluate_screenshot_full_chain() {
    if skip_if_no_chrome() {
        return;
    }

    let (_server, url) = start_test_server().await;
    let cfg = test_config();
    let session = BrowserSession::launch(cfg).await.unwrap();

    let page = session.new_page().await.unwrap();
    page.navigate(&url).await.unwrap();

    // Wait a short moment for rendering to settle before screenshot.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let title = page.evaluate("document.title").await.unwrap();
    assert_eq!(title.as_str(), Some("end-to-end"));

    let value = page.evaluate("document.getElementById('result').textContent").await.unwrap();
    assert_eq!(value.as_str(), Some("ODY"));

    let screenshot = page.screenshot(false).await.unwrap();
    assert!(!screenshot.is_empty(), "screenshot should not be empty");
    assert_eq!(&screenshot[..4], &[0x89, 0x50, 0x4E, 0x47], "screenshot should be a PNG");

    let _ = page.close().await;
    session.close().await.unwrap();
}

/// Full-page screenshot should still be a PNG and contain the tall content.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_page_screenshot_is_png() {
    if skip_if_no_chrome() {
        return;
    }

    let body = br#"<html><head><title>full-page</title></head><body><div id="result" style="height:2000px;background:linear-gradient(#fff,#000);">ODY</div></body></html>"#;
    let (_server, url) = start_test_server_with_body(body).await;
    let cfg = test_config();
    let session = BrowserSession::launch(cfg).await.unwrap();

    let page = session.new_page().await.unwrap();
    page.navigate(&url).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    let screenshot = page.screenshot(true).await.unwrap();
    assert!(!screenshot.is_empty(), "full-page screenshot should not be empty");
    assert_eq!(&screenshot[..4], &[0x89, 0x50, 0x4E, 0x47], "full-page screenshot should be a PNG");

    let _ = page.close().await;
    session.close().await.unwrap();
}

/// Console logs emitted by the page are captured by the event buffer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn console_logs_are_collected() {
    if skip_if_no_chrome() {
        return;
    }

    let body = br#"<html><head><title>console</title></head><body><script>console.log("ODY_CONSOLE");</script></body></html>"#;
    let (_server, url) = start_test_server_with_body(body).await;
    let cfg = test_config();
    let session = BrowserSession::launch(cfg).await.unwrap();

    let page = session.new_page().await.unwrap();
    page.navigate(&url).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    let logs = page.read_logs().await.unwrap();
    let found = logs
        .console
        .iter()
        .any(|entry| entry.text.contains("ODY_CONSOLE"));
    assert!(found, "console log should contain ODY_CONSOLE: {logs:?}");

    let _ = page.close().await;
    session.close().await.unwrap();
}

/// get_dom returns the full document tree and scoped element HTML.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_dom_returns_document_and_element() {
    if skip_if_no_chrome() {
        return;
    }

    let (_server, url) = start_test_server().await;
    let cfg = test_config();
    let session = BrowserSession::launch(cfg).await.unwrap();

    let page = session.new_page().await.unwrap();
    page.navigate(&url).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    let full = page.get_dom(None).await.unwrap();
    assert!(full.is_object(), "full DOM should be a JSON object: {full:?}");

    let element = page.get_dom(Some("#result")).await.unwrap();
    let html = element.as_str().unwrap_or_default();
    assert!(html.contains("ODY"), "element DOM should contain ODY: {element:?}");

    let _ = page.close().await;
    session.close().await.unwrap();
}

/// Click and type on the page update the DOM as expected.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn click_and_type_updates_dom() {
    if skip_if_no_chrome() {
        return;
    }

    let body = br#"<html><head><title>input</title><style>body{margin:0;padding:0;}</style></head><body>
<input id="name" type="text" value="" />
<button id="btn" style="display:block;width:100vw;height:100vh;margin:0;padding:0;border:none;background:transparent;">Submit</button>
<div id="output"></div>
<script>
document.getElementById('btn').addEventListener('click', function() {
    document.getElementById('output').textContent = document.getElementById('name').value;
});
</script>
</body></html>"#;
    let (_server, url) = start_test_server_with_body(body).await;
    let cfg = test_config();
    let session = BrowserSession::launch(cfg).await.unwrap();

    let page = session.new_page().await.unwrap();
    page.navigate(&url).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    page.type_text("#name", "Ody").await.unwrap();

    let input_val = page.evaluate("document.getElementById('name').value").await.unwrap();
    assert_eq!(input_val.as_str(), Some("Ody"), "type_text should populate input: {input_val:?}");

    // Click near the top-left of the viewport; the button fills the viewport.
    page.click(50.0, 50.0).await.unwrap();

    tokio::time::sleep(Duration::from_millis(300)).await;

    let output = page.evaluate("document.getElementById('output').textContent").await.unwrap();
    assert_eq!(output.as_str(), Some("Ody"), "output should reflect typed text: {output:?}");

    let _ = page.close().await;
    session.close().await.unwrap();
}

/// Network logs are captured and sensitive headers are redacted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn network_logs_are_redacted() {
    if skip_if_no_chrome() {
        return;
    }

    // Start a single server that serves both the page and the API endpoint,
    // so the fetch is same-origin and the browser will send the cookie we set.
    let api_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = api_listener.local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{port}/");
    let api_url = format!("http://127.0.0.1:{port}/api");

    let page_body = format!(
        r#"<html><head><title>network</title></head><body><script>
(async () => {{
    const res = await fetch("/api", {{
        method: "POST",
        headers: {{
            "Authorization": "Bearer secret",
            "X-Api-Key": "secret-key",
            "X-Custom": "visible"
        }},
        body: "{{}}"
    }});
    return res.ok;
}})();
</script></body></html>"#
    );
    let page_body = page_body.leak();
    let api_body = b"{\"ok\":true}";
    let api_body_static: &'static [u8] = api_body;
    let server_handle = tokio::spawn(async move {
        loop {
            let (mut socket, _) = api_listener.accept().await.unwrap();
            let mut buf = [0u8; 2048];
            let n = socket.read(&mut buf).await.unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            if req.starts_with("POST /api") || req.starts_with("OPTIONS /api") {
                let response = format!(
                    "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Headers: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                    api_body_static.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.write_all(api_body_static).await;
            } else {
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n\r\n",
                    page_body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.write_all(page_body.as_bytes()).await;
            }
            let _ = socket.shutdown().await;
        }
    });

    let cfg = test_config();
    let session = BrowserSession::launch(cfg).await.unwrap();

    let page = session.new_page().await.unwrap();
    page.navigate(&url).await.unwrap();

    // Perform the fetch inside the page and await it so the request is fully
    // sent before we read the network buffer. The page response sets a cookie
    // so the browser should include a Cookie header in the same-origin request.
    let fetch_result = page
        .evaluate("(async () => { const res = await fetch('/api', { method: 'POST', headers: { 'Authorization': 'Bearer secret', 'X-Api-Key': 'secret-key', 'X-Custom': 'visible' }, body: '{}' }); return { ok: res.ok }; })()")
        .await
        .unwrap();
    assert_eq!(fetch_result["ok"].as_bool(), Some(true), "fetch should succeed: {fetch_result:?}");

    let logs = page.read_logs().await.unwrap();
    let entry = logs
        .network
        .iter()
        .find(|e| e.url == api_url)
        .expect("network entry for API should exist");

    let req_headers = entry.request_headers.as_ref().expect("request headers should be present");
    assert_eq!(
        req_headers.get("Authorization").map(|s| s.as_str()),
        Some("[REDACTED]"),
        "Authorization header should be redacted: {req_headers:?}"
    );
    assert_eq!(
        req_headers.get("X-Api-Key").map(|s| s.as_str()),
        Some("[REDACTED]"),
        "X-Api-Key header should be redacted: {req_headers:?}"
    );
    assert_eq!(
        req_headers.get("X-Custom").map(|s| s.as_str()),
        Some("visible"),
        "non-sensitive custom header should remain visible: {req_headers:?}"
    );
    // Response body is always cleared before snapshot.
    assert!(entry.response_body.as_ref().map(|b| b.is_empty()).unwrap_or(true));

    let _ = page.close().await;
    session.close().await.unwrap();
    drop(server_handle);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_mode_rejects_navigate() {
    let cfg = BrowserControlConfig {
        mode: BrowserControlMode::External,
        ..test_config()
    };
    let err = BrowserSession::launch(cfg).await.unwrap_err();
    assert!(matches!(err, ody_browser_control::BrowserControlError::NotAllowed { .. }));
}
