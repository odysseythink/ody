use ody_remote::gateway::auth::hash_password;
use ody_remote::gateway::server::GatewayConfig;
use ody_remote::gateway::server::router;
use ody_remote::proto::*;
use serde_json::json;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

const USERNAME: &str = "ranwei";
const PASSWORD: &str = "test-password-123";
const AGENT_TOKEN: &str = "agent-secret-token";

struct TestServer {
    addr: SocketAddr,
    _task: JoinHandle<()>,
}

async fn start_server() -> TestServer {
    let config = GatewayConfig {
        username: USERNAME.to_string(),
        password_hash: hash_password(PASSWORD),
        agent_token: AGENT_TOKEN.to_string(),
        session_ttl: Duration::from_secs(3600),
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let app = router(config);
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    TestServer { addr, _task: task }
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("client")
}

async fn login(client: &reqwest::Client, addr: SocketAddr) -> String {
    let resp = client
        .post(format!("http://{addr}/api/login"))
        .json(&json!({"username": USERNAME, "password": PASSWORD}))
        .send()
        .await
        .expect("login request");
    assert_eq!(resp.status(), 200);
    let set_cookie = resp
        .headers()
        .get("set-cookie")
        .expect("set-cookie header")
        .to_str()
        .expect("utf8")
        .to_string();
    ody_remote::gateway::auth::extract_session_cookie(&set_cookie).expect("session cookie")
}

async fn connect_agent(
    addr: SocketAddr,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let mut request = format!("ws://{addr}/ws/agent")
        .into_client_request()
        .expect("request");
    request.headers_mut().insert(
        "authorization",
        format!("Bearer {AGENT_TOKEN}").parse().expect("header"),
    );
    let (ws, _resp) = connect_async(request).await.expect("agent ws connect");
    ws
}

use futures::SinkExt;
use futures::StreamExt;

async fn send<T: serde::Serialize>(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    msg: &T,
) {
    ws.send(Message::Text(
        serde_json::to_string(msg).expect("json").into(),
    ))
    .await
    .expect("ws send");
}

async fn recv<T: serde::de::DeserializeOwned>(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> T {
    let msg = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("ws recv timeout")
        .expect("ws closed")
        .expect("ws error");
    let Message::Text(text) = msg else {
        panic!("expected text frame, got {msg:?}");
    };
    serde_json::from_str(&text).expect("parse json")
}

#[tokio::test]
async fn login_rejects_wrong_password_and_accepts_correct() {
    let server = start_server().await;
    let client = http_client();

    let resp = client
        .post(format!("http://{}/api/login", server.addr))
        .json(&json!({"username": USERNAME, "password": "nope"}))
        .send()
        .await
        .expect("request");
    assert_eq!(resp.status(), 401);

    let _cookie = login(&client, server.addr).await;
}

#[tokio::test]
async fn instances_api_requires_session() {
    let server = start_server().await;
    let client = http_client();

    let resp = client
        .get(format!("http://{}/api/instances", server.addr))
        .send()
        .await
        .expect("request");
    assert_eq!(resp.status(), 401);

    let cookie = login(&client, server.addr).await;
    let resp = client
        .get(format!("http://{}/api/instances", server.addr))
        .header("cookie", format!("ody_remote_session={cookie}"))
        .send()
        .await
        .expect("request");
    assert_eq!(resp.status(), 200);
    let list: Vec<InstanceInfo> = resp.json().await.expect("json");
    assert!(list.is_empty());
}

#[tokio::test]
async fn client_ws_rejected_without_session() {
    let server = start_server().await;
    let result = connect_async(format!("ws://{}/ws", server.addr)).await;
    assert!(result.is_err(), "unauthenticated ws upgrade must fail");
}

#[tokio::test]
async fn agent_ws_rejected_with_wrong_token() {
    let server = start_server().await;
    let mut request = format!("ws://{}/ws/agent", server.addr)
        .into_client_request()
        .expect("request");
    request.headers_mut().insert(
        "authorization",
        "Bearer wrong-token".parse().expect("header"),
    );
    assert!(connect_async(request).await.is_err());
}

#[tokio::test]
async fn full_path_agent_rpc_and_events_reach_client() {
    let server = start_server().await;
    let client = http_client();
    let cookie = login(&client, server.addr).await;

    // Agent connects and announces one instance.
    let mut agent = connect_agent(server.addr).await;
    send(
        &mut agent,
        &AgentToGateway::Hello {
            machine: "mac-a".to_string(),
            instances: vec![InstanceDescriptor {
                id: "mac-a/inst-1".to_string(),
                name: "inst-1".to_string(),
            }],
        },
    )
    .await;

    // The instance shows up in the REST listing.
    let list: Vec<InstanceInfo> = client
        .get(format!("http://{}/api/instances", server.addr))
        .header("cookie", format!("ody_remote_session={cookie}"))
        .send()
        .await
        .expect("instances")
        .json()
        .await
        .expect("json");
    assert_eq!(list.len(), 1);
    assert!(list[0].online);

    // Browser client connects over ws and gets a snapshot.
    let mut request = format!("ws://{}/ws", server.addr)
        .into_client_request()
        .expect("request");
    request.headers_mut().insert(
        "cookie",
        format!("ody_remote_session={cookie}")
            .parse()
            .expect("header"),
    );
    let (mut browser, _resp) = connect_async(request).await.expect("client ws");
    match recv::<GatewayToClient>(&mut browser).await {
        GatewayToClient::Instances { list } => assert_eq!(list.len(), 1),
        other => panic!("expected Instances, got {other:?}"),
    }

    // Client RPC is proxied to the agent and the response comes back.
    send(
        &mut browser,
        &ClientToGateway::Rpc {
            id: 1,
            instance: "mac-a/inst-1".to_string(),
            method: "thread/list".to_string(),
            params: json!({}),
        },
    )
    .await;
    let GatewayToAgent::Rpc { id: gw_id, .. } = recv::<GatewayToAgent>(&mut agent).await else {
        panic!("agent should receive the proxied rpc");
    };
    send(
        &mut agent,
        &AgentToGateway::RpcResponse {
            id: gw_id,
            outcome: RpcOutcome::Ok {
                value: json!({"threads": [{"id": "t-1"}]}),
            },
        },
    )
    .await;
    assert_eq!(
        recv::<GatewayToClient>(&mut browser).await,
        GatewayToClient::RpcResponse {
            id: 1,
            outcome: RpcOutcome::Ok {
                value: json!({"threads": [{"id": "t-1"}]})
            },
        }
    );

    // An event from the agent fans out to the browser.
    send(
        &mut agent,
        &AgentToGateway::Event {
            instance: "mac-a/inst-1".to_string(),
            payload: json!({"method": "turn/started", "params": {}}),
        },
    )
    .await;
    assert_eq!(
        recv::<GatewayToClient>(&mut browser).await,
        GatewayToClient::Event {
            instance: "mac-a/inst-1".to_string(),
            payload: json!({"method": "turn/started", "params": {}}),
        }
    );

    // Agent disconnect marks the instance offline and pushes the update.
    drop(agent);
    match recv::<GatewayToClient>(&mut browser).await {
        GatewayToClient::Instances { list } => {
            assert_eq!(list.len(), 1);
            assert!(!list[0].online, "instance should be offline");
        }
        other => panic!("expected Instances, got {other:?}"),
    }
}

#[tokio::test]
async fn non_whitelisted_method_is_rejected_before_reaching_agent() {
    let server = start_server().await;
    let client = http_client();
    let cookie = login(&client, server.addr).await;

    let mut agent = connect_agent(server.addr).await;
    send(
        &mut agent,
        &AgentToGateway::Hello {
            machine: "mac-a".to_string(),
            instances: vec![InstanceDescriptor {
                id: "mac-a/inst-1".to_string(),
                name: "inst-1".to_string(),
            }],
        },
    )
    .await;

    let mut request = format!("ws://{}/ws", server.addr)
        .into_client_request()
        .expect("request");
    request.headers_mut().insert(
        "cookie",
        format!("ody_remote_session={cookie}")
            .parse()
            .expect("header"),
    );
    let (mut browser, _resp) = connect_async(request).await.expect("client ws");
    let _snapshot: GatewayToClient = recv(&mut browser).await;

    send(
        &mut browser,
        &ClientToGateway::Rpc {
            id: 5,
            instance: "mac-a/inst-1".to_string(),
            method: "turn/start".to_string(),
            params: json!({}),
        },
    )
    .await;

    match recv::<GatewayToClient>(&mut browser).await {
        GatewayToClient::RpcResponse {
            id,
            outcome: RpcOutcome::Err { message },
        } => {
            assert_eq!(id, 5);
            assert!(message.contains("read-only"), "message: {message}");
        }
        other => panic!("expected rpc error, got {other:?}"),
    }
    // Nothing should reach the agent; give it a brief window to prove a negative.
    let unexpected = tokio::time::timeout(Duration::from_millis(300), agent.next()).await;
    assert!(
        unexpected.is_err(),
        "agent must not receive non-whitelisted rpc"
    );
}

#[tokio::test]
async fn index_page_is_served() {
    let server = start_server().await;
    let client = http_client();
    let resp = client
        .get(format!("http://{}/", server.addr))
        .send()
        .await
        .expect("request");
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.expect("body");
    assert!(body.contains("<html"), "index html: {body}");
}
