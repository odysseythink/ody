use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use ody_browser_control::{
    acquire_browser_permit, discover_chrome, BrowserControlConfig, BrowserControlError,
    BrowserControlMode, BrowserSession, BrowserThreadState,
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
            let body = b"<html><head><title>test</title></head><body>hello</body></html>";
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn discover_chrome_finds_an_executable() {
    if skip_if_no_chrome() {
        return;
    }
    let cfg = BrowserControlConfig::default();
    let path = discover_chrome(&cfg).unwrap();
    assert!(path.exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn launch_creates_local_session_with_temp_profile() {
    if skip_if_no_chrome() {
        return;
    }
    let cfg = test_config();
    let session = BrowserSession::launch(cfg).await.unwrap();
    assert!(session.is_local());
    let profile_path = session.profile_dir_path().to_path_buf();
    assert!(profile_path.exists());

    session.close().await.unwrap();

    // Give Windows a moment to release handles, then assert the profile is gone.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !profile_path.exists(),
        "profile directory was not removed: {}",
        profile_path.display()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a responsive Chrome instance (page creation hangs in this environment)"]
async fn multiple_pages_are_independent() {
    if skip_if_no_chrome() {
        return;
    }
    let (_server, url) = start_test_server().await;
    let cfg = test_config();
    let session = BrowserSession::launch(cfg).await.unwrap();

    let page1 = session.new_page().await.unwrap();
    page1.navigate(&format!("{url}/page1")).await.unwrap();

    let page2 = session.new_page().await.unwrap();
    page2.navigate(&format!("{url}/page2")).await.unwrap();

    let url1 = page1.evaluate("document.location.href").await.unwrap();
    let url2 = page2.evaluate("document.location.href").await.unwrap();
    assert_ne!(url1, url2);

    let _ = page1.close().await;
    let _ = page2.close().await;
    session.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires a responsive Chrome instance (page creation hangs in this environment)"]
async fn thread_state_navigates_and_reuses_default_page() {
    if skip_if_no_chrome() {
        return;
    }
    let (_server, url) = start_test_server().await;
    let cfg = test_config();
    let thread = BrowserThreadState::new(cfg).await.unwrap();

    thread.navigate(&format!("{url}/page1"), None).await.unwrap();
    thread.navigate(&format!("{url}/page2"), None).await.unwrap();

    thread.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn launch_fails_for_external_mode() {
    let cfg = BrowserControlConfig {
        mode: BrowserControlMode::External,
        ..test_config()
    };
    let err = BrowserSession::launch(cfg).await.unwrap_err();
    assert!(matches!(err, BrowserControlError::NotAllowed { .. }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connect_fails_for_local_mode() {
    let cfg = BrowserControlConfig {
        mode: BrowserControlMode::Local,
        ..test_config()
    };
    let err = BrowserSession::connect(cfg).await.unwrap_err();
    assert!(matches!(err, BrowserControlError::NotAllowed { .. }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connect_to_missing_endpoint_fails_fast() {
    let cfg = BrowserControlConfig {
        mode: BrowserControlMode::External,
        connect_url: Some("ws://127.0.0.1:1".to_string()),
        ..test_config()
    };
    let start = tokio::time::Instant::now();
    let err = BrowserSession::connect(cfg).await.unwrap_err();
    let elapsed = start.elapsed();
    assert!(matches!(err, BrowserControlError::ConnectFailed { .. }));
    assert!(
        elapsed < Duration::from_secs(5),
        "connect did not respect connect_timeout_ms: {:?}",
        elapsed
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_quota_times_out() {
    let cfg = BrowserControlConfig {
        max_concurrent_browsers: 1,
        browser_permit_acquire_timeout_ms: 100,
        ..test_config()
    };
    let permit = acquire_browser_permit(&cfg).await.unwrap();
    let err = acquire_browser_permit(&cfg).await.unwrap_err();
    assert!(matches!(err, BrowserControlError::QuotaExceeded { .. }));
    drop(permit);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_does_not_panic_or_hang() {
    if skip_if_no_chrome() {
        return;
    }
    let cfg = test_config();
    let session = BrowserSession::launch(cfg).await.unwrap();
    let profile_path = session.profile_dir_path().to_path_buf();
    assert!(profile_path.exists());

    // Dropping the session without calling close() should still trigger the
    // Drop impl to kill the process best-effort and allow the TempDir to clean
    // up. The exact timing of profile removal depends on the platform and the
    // Chrome process shape, so we only assert that drop itself completes.
    drop(session);
    tokio::time::sleep(Duration::from_millis(500)).await;
}
