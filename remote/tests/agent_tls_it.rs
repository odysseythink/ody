//! TLS e2e: agent connects to a gateway fronted by a self-signed certificate
//! when (and only when) `gateway_ca_cert` points at that CA/cert PEM.

use futures::SinkExt;
use futures::StreamExt;
use ody_remote::agent::config::AgentConfig;
use ody_remote::proto::AgentToGateway;
use rustls::ServerConfig;
use rustls::pki_types::CertificateDer;
use rustls::pki_types::PrivateKeyDer;
use rustls::pki_types::PrivatePkcs8KeyDer;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::Callback;
use tokio_tungstenite::tungstenite::handshake::server::ErrorResponse;
use tokio_tungstenite::tungstenite::handshake::server::Request;
use tokio_tungstenite::tungstenite::handshake::server::Response;

type Ws = WebSocketStream<TlsStream<TcpStream>>;

const AGENT_TOKEN: &str = "agent-secret";

struct AuthCallback;

impl Callback for AuthCallback {
    fn on_request(self, request: &Request, response: Response) -> Result<Response, ErrorResponse> {
        let ok = request
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v == format!("Bearer {AGENT_TOKEN}"));
        if ok {
            Ok(response)
        } else {
            let mut err = ErrorResponse::new(Some("unauthorized".to_string()));
            *err.status_mut() = tokio_tungstenite::tungstenite::http::StatusCode::UNAUTHORIZED;
            Err(err)
        }
    }
}

/// Spin up a TLS websocket fake gateway with a freshly generated self-signed
/// cert. Writes the cert PEM to `ca_pem_path` and returns the listen address
/// plus a channel of parsed agent messages.
async fn start_tls_fake_gateway(
    ca_pem_path: &Path,
) -> (SocketAddr, mpsc::Receiver<AgentToGateway>) {
    ody_utils_rustls_provider::ensure_rustls_crypto_provider();
    let rcgen::CertifiedKey { cert, signing_key } =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string(), "127.0.0.1".to_string()])
            .expect("generate cert");
    std::fs::write(ca_pem_path, cert.pem()).expect("write ca pem");
    let server_config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(cert.der().clone())],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(signing_key.serialize_der())),
        )
        .expect("server config");
    let acceptor = TlsAcceptor::from(Arc::new(server_config));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let (from_agent_tx, from_agent_rx) = mpsc::channel(32);
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.expect("accept");
            let Ok(tls) = acceptor.accept(stream).await else {
                continue; // handshake failure (e.g. untrusted client dropping)
            };
            let Ok(mut ws): Result<Ws, _> = accept_hdr_async(tls, AuthCallback).await else {
                continue;
            };
            while let Some(Ok(Message::Text(text))) = ws.next().await {
                if let Ok(parsed) = serde_json::from_str::<AgentToGateway>(&text) {
                    let _ = from_agent_tx.send(parsed).await;
                }
            }
        }
    });
    (addr, from_agent_rx)
}

fn tls_test_config(gateway_addr: SocketAddr, ca_pem: Option<&Path>) -> AgentConfig {
    let ca_line = ca_pem
        .map(|p| format!("gateway_ca_cert = \"{}\"", p.display()))
        .unwrap_or_default();
    AgentConfig::from_toml_str(&format!(
        r#"
        gateway_url = "wss://{gateway_addr}/ws/agent"
        token = "{AGENT_TOKEN}"
        machine = "mac-test"
        reconnect_delay_ms = 100
        {ca_line}

        [[instances]]
        name = "i1"
        url = "ws://127.0.0.1:9"
        "#
    ))
    .expect("config")
}

#[tokio::test]
async fn agent_connects_via_wss_with_trusted_self_signed_ca() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ca_pem = tmp.path().join("gateway-ca.pem");
    let (addr, mut from_agent_rx) = start_tls_fake_gateway(&ca_pem).await;

    let config = tls_test_config(addr, Some(&ca_pem));
    let agent_task = tokio::spawn(async move {
        let _ = ody_remote::agent::run(config).await;
    });

    let msg = tokio::time::timeout(Duration::from_secs(10), from_agent_rx.recv())
        .await
        .expect("timeout waiting for agent hello over wss")
        .expect("channel closed");
    match msg {
        AgentToGateway::Hello { machine, .. } => assert_eq!(machine, "mac-test"),
        other => panic!("expected Hello, got {other:?}"),
    }
    agent_task.abort();
}

#[tokio::test]
async fn agent_rejects_self_signed_gateway_without_ca_cert() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ca_pem = tmp.path().join("gateway-ca.pem");
    let (addr, mut from_agent_rx) = start_tls_fake_gateway(&ca_pem).await;

    // No gateway_ca_cert: the self-signed cert is not in the native roots.
    let config = tls_test_config(addr, None);
    let agent_task = tokio::spawn(async move {
        let _ = ody_remote::agent::run(config).await;
    });

    let received = tokio::time::timeout(Duration::from_secs(2), from_agent_rx.recv()).await;
    assert!(
        received.is_err(),
        "agent must not complete the TLS handshake with an untrusted self-signed cert"
    );
    agent_task.abort();
}

#[test]
fn connector_errors_on_missing_ca_file() {
    let config = tls_test_config(
        "127.0.0.1:9".parse().expect("addr"),
        Some(Path::new("/definitely/not/here/ca.pem")),
    );
    assert!(ody_remote::agent::gateway_connector(&config).is_err());
}

#[test]
fn connector_errors_on_non_pem_ca_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ca_pem = tmp.path().join("garbage.pem");
    std::fs::write(&ca_pem, b"not a pem file").expect("write");
    let config = tls_test_config("127.0.0.1:9".parse().expect("addr"), Some(&ca_pem));
    assert!(ody_remote::agent::gateway_connector(&config).is_err());
}

#[test]
fn connector_is_none_without_ca_cert() {
    let config = tls_test_config("127.0.0.1:9".parse().expect("addr"), None);
    assert!(
        ody_remote::agent::gateway_connector(&config)
            .expect("connector")
            .is_none()
    );
}
