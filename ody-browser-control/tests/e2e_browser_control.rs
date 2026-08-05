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

async fn start_test_server() -> (tokio::task::JoinHandle<()>, String) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;
            let body = b"<html><head><title>end-to-end</title></head><body><div id=\"result\">ODY</div></body></html>";
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_mode_rejects_navigate() {
    let cfg = BrowserControlConfig {
        mode: BrowserControlMode::External,
        ..test_config()
    };
    let err = BrowserSession::launch(cfg).await.unwrap_err();
    assert!(matches!(err, ody_browser_control::BrowserControlError::NotAllowed { .. }));
}
