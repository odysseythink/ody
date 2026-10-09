//! Per-machine agent: reverse-connects to the gateway and bridges each
//! configured local ody app-server (JSON-RPC over websocket).

pub mod config;

use crate::proto::AgentToGateway;
use crate::proto::GatewayToAgent;
use crate::proto::InstanceDescriptor;
use crate::proto::RpcOutcome;
use anyhow::Context as _;
use config::AgentConfig;
use config::InstanceConf;
use futures::SinkExt;
use futures::StreamExt;
use ody_app_server_client::AppServerEvent;
use ody_app_server_client::AppServerRequestHandle;
use ody_app_server_client::RemoteAppServerClient;
use ody_app_server_client::RemoteAppServerConnectArgs;
use ody_app_server_client::RemoteAppServerEndpoint;
use ody_app_server_protocol::ClientRequest;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use tokio::sync::mpsc;
use tokio_tungstenite::Connector;
use tokio_tungstenite::connect_async_tls_with_config;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tracing::info;
use tracing::warn;

/// Run forever: connect to the gateway, drive one session, reconnect on drop.
// ody: 固定间隔重连，无指数退避；升级触发条件=gateway 长时间宕机导致重连风暴时改为指数退避。
pub async fn run(config: AgentConfig) -> anyhow::Result<()> {
    loop {
        match drive(&config).await {
            Ok(()) => info!("gateway connection closed cleanly; reconnecting"),
            Err(err) => warn!("gateway session ended: {err:#}; reconnecting"),
        }
        tokio::time::sleep(config.reconnect_delay).await;
    }
}

/// Build the TLS connector for the gateway connection: system roots plus an
/// optional extra CA/cert PEM from `gateway_ca_cert` (needed when the gateway
/// is fronted by a self-signed/internal CA). Returns `None` for plain
/// system-roots TLS so non-TLS `ws://` URLs keep working unchanged.
pub fn gateway_connector(config: &AgentConfig) -> anyhow::Result<Option<Connector>> {
    let Some(ca_path) = &config.gateway_ca_cert else {
        return Ok(None);
    };
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_native_certs::load_native_certs().certs {
        // Skip individual unparseable system certs rather than failing hard.
        let _ = roots.add(cert);
    }
    let pem = std::fs::read(ca_path.as_path())
        .with_context(|| format!("read gateway_ca_cert {}", ca_path.display()))?;
    let mut added = 0usize;
    for cert in rustls::pki_types::pem::PemObject::pem_slice_iter(&pem as &[u8]) {
        let cert: rustls::pki_types::CertificateDer<'static> =
            cert.context("parse gateway_ca_cert PEM")?;
        roots.add(cert).context("add gateway_ca_cert to roots")?;
        added += 1;
    }
    anyhow::ensure!(
        added > 0,
        "gateway_ca_cert {} contains no certificates",
        ca_path.display()
    );
    ody_utils_rustls_provider::ensure_rustls_crypto_provider();
    let tls = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Some(Connector::Rustls(Arc::new(tls))))
}

/// One gateway session: hello, spawn instance supervisors, proxy rpcs.
async fn drive(config: &AgentConfig) -> anyhow::Result<()> {
    let mut request = config
        .gateway_url
        .clone()
        .into_client_request()
        .context("invalid gateway_url")?;
    request.headers_mut().insert(
        "authorization",
        format!("Bearer {}", config.token)
            .parse()
            .context("agent token is not a valid header value")?,
    );
    let connector = gateway_connector(config)?;
    let (ws, _resp) = connect_async_tls_with_config(request, None, false, connector)
        .await
        .context("connect gateway")?;
    let (mut sink, mut stream) = ws.split();

    // Single writer for all upstream traffic.
    let (up_tx, mut up_rx) = mpsc::channel::<AgentToGateway>(256);
    let writer = tokio::spawn(async move {
        while let Some(msg) = up_rx.recv().await {
            let Some(text) = crate::proto::to_json_text(&msg) else {
                continue;
            };
            if sink.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    let instances: Vec<InstanceDescriptor> = config
        .instances
        .iter()
        .map(|i| InstanceDescriptor {
            id: i.id(),
            name: i.name.clone(),
        })
        .collect();
    let hello = AgentToGateway::Hello {
        machine: config.machine.clone(),
        instances,
    };
    up_tx.send(hello).await.context("send hello")?;

    // Instance supervisors: each keeps a reconnecting connection to its local
    // app-server and streams events upstream.
    let handles: Arc<Mutex<HashMap<String, AppServerRequestHandle>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let mut supervisors = Vec::new();
    for instance in &config.instances {
        supervisors.push(tokio::spawn(supervise_instance(
            instance.clone(),
            up_tx.clone(),
            Arc::clone(&handles),
            config.reconnect_delay,
        )));
    }

    // Proxy rpcs from the gateway to instance handles.
    let mut session_result = Ok(());
    while let Some(msg) = stream.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                let Ok(parsed) = serde_json::from_str::<GatewayToAgent>(&text) else {
                    warn!("ignoring unparsable gateway frame");
                    continue;
                };
                match parsed {
                    GatewayToAgent::Rpc {
                        id,
                        instance,
                        method,
                        params,
                    } => {
                        let handle = crate::lock(&handles).get(&instance).cloned();
                        let up = up_tx.clone();
                        tokio::spawn(async move {
                            let outcome = match handle {
                                Some(handle) => {
                                    let parsed = serde_json::from_value::<ClientRequest>(
                                        serde_json::json!({
                                            "id": id,
                                            "method": method,
                                            "params": params,
                                        }),
                                    );
                                    match parsed {
                                        Err(err) => RpcOutcome::Err {
                                            message: format!("invalid request params: {err}"),
                                        },
                                        Ok(request) => match handle.request(request).await {
                                            Ok(Ok(value)) => RpcOutcome::Ok { value },
                                            Ok(Err(err)) => RpcOutcome::Err {
                                                message: format!(
                                                    "{} (code {})",
                                                    err.message, err.code
                                                ),
                                            },
                                            Err(err) => RpcOutcome::Err {
                                                message: err.to_string(),
                                            },
                                        },
                                    }
                                }
                                None => RpcOutcome::Err {
                                    message: format!("instance `{instance}` offline"),
                                },
                            };
                            let _ = up.send(AgentToGateway::RpcResponse { id, outcome }).await;
                        });
                    }
                }
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {} // ping/pong/binary
            Err(err) => {
                session_result = Err(err).context("gateway websocket error");
                break;
            }
        }
    }

    for task in &supervisors {
        task.abort();
    }
    drop(up_tx);
    let _ = writer.await;
    session_result
}

/// Keep one local app-server connected; stream its events upstream.
async fn supervise_instance(
    instance: InstanceConf,
    up: mpsc::Sender<AgentToGateway>,
    handles: Arc<Mutex<HashMap<String, AppServerRequestHandle>>>,
    reconnect_delay: std::time::Duration,
) {
    let id = instance.id();
    loop {
        match connect_instance(&instance).await {
            Ok(mut client) => {
                info!(instance = %id, "connected to local app-server");
                crate::lock(&handles).insert(
                    id.clone(),
                    AppServerRequestHandle::Remote(client.request_handle()),
                );
                let _ = up
                    .send(AgentToGateway::Status {
                        instance: id.clone(),
                        online: true,
                    })
                    .await;
                while let Some(event) = client.next_event().await {
                    let disconnected = matches!(event, AppServerEvent::Disconnected { .. });
                    let payload = match &event {
                        AppServerEvent::ServerNotification(n) => serde_json::to_value(n).ok(),
                        AppServerEvent::ServerRequest(r) => serde_json::to_value(r).ok(),
                        AppServerEvent::Lagged { skipped } => Some(serde_json::json!({
                            "method": "ody-remote/lagged",
                            "params": {"skipped": skipped},
                        })),
                        AppServerEvent::Disconnected { .. } => None,
                    };
                    if let Some(payload) = payload {
                        let _ = up
                            .send(AgentToGateway::Event {
                                instance: id.clone(),
                                payload,
                            })
                            .await;
                    }
                    if disconnected {
                        break;
                    }
                }
                crate::lock(&handles).remove(&id);
                let _ = up
                    .send(AgentToGateway::Status {
                        instance: id.clone(),
                        online: false,
                    })
                    .await;
                warn!(instance = %id, "local app-server connection ended; retrying");
            }
            Err(err) => {
                warn!(instance = %id, "connect to local app-server failed: {err:#}");
            }
        }
        tokio::time::sleep(reconnect_delay).await;
    }
}

async fn connect_instance(instance: &InstanceConf) -> anyhow::Result<RemoteAppServerClient> {
    RemoteAppServerClient::connect(RemoteAppServerConnectArgs {
        endpoint: RemoteAppServerEndpoint::WebSocket {
            websocket_url: instance.url.clone(),
            auth_token: instance.token.clone(),
        },
        client_name: "ody-remote-agent".to_string(),
        client_version: env!("CARGO_PKG_VERSION").to_string(),
        experimental_api: false,
        mcp_server_form_elicitation: false,
        opt_out_notification_methods: Vec::new(),
        channel_capacity: 256,
    })
    .await
    .map_err(|err| anyhow::anyhow!(err))
    .context("app-server connect")
}
