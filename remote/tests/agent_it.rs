//! End-to-end agent test: a fake ody app-server (JSON-RPC over ws) and a fake
//! gateway, exercising bridging, rpc proxying, event forwarding and reconnect.

use futures::SinkExt;
use futures::StreamExt;
use ody_remote::agent::config::AgentConfig;
use ody_remote::proto::*;
use serde_json::json;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::Callback;
use tokio_tungstenite::tungstenite::handshake::server::ErrorResponse;
use tokio_tungstenite::tungstenite::handshake::server::Request;
use tokio_tungstenite::tungstenite::handshake::server::Response;

type Ws = WebSocketStream<TcpStream>;

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

async fn ws_send<T: serde::Serialize>(ws: &mut Ws, msg: &T) {
    ws.send(Message::Text(
        serde_json::to_string(msg).expect("json").into(),
    ))
    .await
    .expect("ws send");
}

async fn ws_recv_value(ws: &mut Ws) -> serde_json::Value {
    let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("ws recv timeout")
        .expect("ws closed")
        .expect("ws error");
    let Message::Text(text) = msg else {
        panic!("expected text frame, got {msg:?}");
    };
    serde_json::from_str(&text).expect("json parse")
}

/// Fake ody app-server: answers initialize, echoes a canned thread/list
/// result, and can be told to emit one notification.
async fn fake_ody_server(listener: TcpListener, notify_tx: mpsc::Receiver<()>) {
    let (stream, _) = listener.accept().await.expect("accept");
    let mut ws = tokio_tungstenite::accept_async(stream)
        .await
        .expect("ws handshake");
    let mut notify_rx = notify_tx;
    loop {
        tokio::select! {
            msg = ws.next() => {
                let Some(Ok(Message::Text(text))) = msg else { break };
                let value: serde_json::Value = serde_json::from_str(&text).expect("json");
                if value.get("id").is_none() {
                    continue; // notification (e.g. "initialized")
                }
                let id = value["id"].clone();
                let result = match value["method"].as_str() {
                    Some("initialize") => json!({
                        "userAgent": "fake-ody/0.0.0",
                        "odyHome": "/tmp/fake-ody-home",
                        "platformFamily": "unix",
                        "platformOs": "macos"
                    }),
                    Some("thread/list") => json!({"threads": [{"id": "t-1", "preview": "hello"}], "nextCursor": null}),
                    Some(_) => json!({}),
                    None => break,
                };
                ws_send(&mut ws, &json!({"jsonrpc": "2.0", "id": id, "result": result})).await;
            }
            cmd = notify_rx.recv() => {
                if cmd.is_none() { break; }
                ws_send(&mut ws, &json!({
                    "jsonrpc": "2.0",
                    "method": "item/agentMessage/delta",
                    "params": {"threadId": "t-1", "turnId": "turn-1", "itemId": "item-1", "delta": "你好"}
                })).await;
            }
        }
    }
}

/// Fake gateway: collects agent messages, lets the test push GatewayToAgent
/// messages and close the connection on demand.
struct FakeGateway {
    addr: SocketAddr,
    from_agent_rx: mpsc::Receiver<AgentToGateway>,
    to_agent_tx: mpsc::Sender<GatewayToAgent>,
    close_tx: mpsc::Sender<()>,
}

async fn start_fake_gateway() -> FakeGateway {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let (from_agent_tx, from_agent_rx) = mpsc::channel(32);
    let (to_agent_tx, mut to_agent_rx) = mpsc::channel::<GatewayToAgent>(32);
    let (close_tx, mut close_rx) = mpsc::channel::<()>(8);
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.expect("accept");
            let Ok(mut ws) = accept_hdr_async(stream, AuthCallback).await else {
                continue;
            };
            loop {
                tokio::select! {
                    msg = ws.next() => {
                        match msg {
                            Some(Ok(Message::Text(text))) => {
                                if let Ok(parsed) = serde_json::from_str::<AgentToGateway>(&text) {
                                    let _ = from_agent_tx.send(parsed).await;
                                }
                            }
                            _ => break, // closed or error: wait for reconnect
                        }
                    }
                    out = to_agent_rx.recv() => {
                        let Some(out) = out else { break };
                        let text = serde_json::to_string(&out).expect("json");
                        if ws.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    _ = close_rx.recv() => {
                        let _ = ws.send(Message::Close(None)).await;
                        break;
                    }
                }
            }
        }
    });
    FakeGateway {
        addr,
        from_agent_rx,
        to_agent_tx,
        close_tx,
    }
}

impl FakeGateway {
    async fn next_from_agent(&mut self) -> AgentToGateway {
        tokio::time::timeout(Duration::from_secs(10), self.from_agent_rx.recv())
            .await
            .expect("timeout waiting for agent message")
            .expect("agent channel closed")
    }
}

fn test_config(gateway_addr: SocketAddr, ody_addr: SocketAddr) -> AgentConfig {
    AgentConfig::from_toml_str(&format!(
        r#"
        gateway_url = "ws://{gateway_addr}/ws/agent"
        token = "{AGENT_TOKEN}"
        machine = "mac-test"
        reconnect_delay_ms = 100

        [[instances]]
        name = "i1"
        url = "ws://{ody_addr}"
        "#
    ))
    .expect("config")
}

#[tokio::test]
async fn agent_bridges_rpc_events_and_reconnects() {
    let ody_listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let ody_addr = ody_listener.local_addr().expect("addr");
    let (notify_tx, notify_rx) = mpsc::channel::<()>(4);
    tokio::spawn(fake_ody_server(ody_listener, notify_rx));

    let mut gateway = start_fake_gateway().await;
    let config = test_config(gateway.addr, ody_addr);
    let agent_task = tokio::spawn(async move {
        let _ = ody_remote::agent::run(config).await;
    });

    // 1. Hello with the announced instance.
    match gateway.next_from_agent().await {
        AgentToGateway::Hello { machine, instances } => {
            assert_eq!(machine, "mac-test");
            assert_eq!(instances.len(), 1);
            assert_eq!(instances[0].id, "mac-test/i1");
        }
        other => panic!("expected Hello, got {other:?}"),
    }

    // 2. The instance reports online once connected to the fake ody server.
    let mut saw_online = false;
    for _ in 0..3 {
        if let AgentToGateway::Status { online: true, .. } = gateway.next_from_agent().await {
            saw_online = true;
            break;
        }
    }
    assert!(saw_online, "agent should report instance online");

    // 3. RPC proxying: gateway -> agent -> fake ody -> back.
    gateway
        .to_agent_tx
        .send(GatewayToAgent::Rpc {
            id: 7,
            instance: "mac-test/i1".to_string(),
            method: "thread/list".to_string(),
            params: json!({}),
        })
        .await
        .expect("send rpc");
    loop {
        match gateway.next_from_agent().await {
            AgentToGateway::RpcResponse { id, outcome } => {
                assert_eq!(id, 7);
                match outcome {
                    RpcOutcome::Ok { value } => {
                        assert_eq!(value["threads"][0]["id"], json!("t-1"));
                        break;
                    }
                    RpcOutcome::Err { message } => panic!("rpc failed: {message}"),
                }
            }
            _ => continue, // ignore interleaved status messages
        }
    }

    // 4. Events from the ody server flow upstream.
    notify_tx.send(()).await.expect("notify");
    loop {
        match gateway.next_from_agent().await {
            AgentToGateway::Event { instance, payload } => {
                assert_eq!(instance, "mac-test/i1");
                assert_eq!(payload["method"], json!("item/agentMessage/delta"));
                assert_eq!(payload["params"]["delta"], json!("你好"));
                break;
            }
            _ => continue,
        }
    }

    // 5. Gateway drop triggers a reconnect with a fresh Hello.
    gateway.close_tx.send(()).await.expect("close");
    let mut saw_second_hello = false;
    for _ in 0..10 {
        if let AgentToGateway::Hello { .. } = gateway.next_from_agent().await {
            saw_second_hello = true;
            break;
        }
    }
    assert!(saw_second_hello, "agent should reconnect and re-announce");

    agent_task.abort();
}
