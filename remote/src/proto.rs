//! JSON message envelopes exchanged over websocket between agents, the
//! gateway, and browser clients. All frames are JSON text.

use serde::Deserialize;
use serde::Serialize;

/// An ody instance as announced by its agent. `id` is globally unique and
/// stable across reconnects: `"<machine>/<name>"`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InstanceDescriptor {
    pub id: String,
    pub name: String,
}

/// An instance as presented to browser clients.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InstanceInfo {
    pub id: String,
    pub machine: String,
    pub name: String,
    pub online: bool,
}

/// Outcome of a proxied JSON-RPC call.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum RpcOutcome {
    Ok { value: serde_json::Value },
    Err { message: String },
}

/// Agent -> Gateway.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AgentToGateway {
    /// Announces the machine and its instances right after connecting.
    Hello {
        machine: String,
        instances: Vec<InstanceDescriptor>,
    },
    /// A raw JSON-RPC notification received from a local ody app-server.
    Event {
        instance: String,
        payload: serde_json::Value,
    },
    /// Response to a [`GatewayToAgent::Rpc`].
    RpcResponse { id: u64, outcome: RpcOutcome },
    /// Local app-server connection state changed.
    Status { instance: String, online: bool },
}

/// Gateway -> Agent.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum GatewayToAgent {
    /// Proxy a (read-only) JSON-RPC request to a local ody app-server.
    Rpc {
        id: u64,
        instance: String,
        method: String,
        params: serde_json::Value,
    },
}

/// Browser client -> Gateway.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ClientToGateway {
    Rpc {
        id: u64,
        instance: String,
        method: String,
        params: serde_json::Value,
    },
}

/// Gateway -> Browser client.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum GatewayToClient {
    RpcResponse {
        id: u64,
        outcome: RpcOutcome,
    },
    Event {
        instance: String,
        payload: serde_json::Value,
    },
    /// Full snapshot of the instance registry; pushed on change.
    Instances {
        list: Vec<InstanceInfo>,
    },
}

/// JSON-RPC methods a browser client may invoke through the gateway.
/// Read-only slice: everything else is rejected before reaching an agent.
pub const READ_ONLY_METHODS: [&str; 4] = [
    "thread/list",
    "thread/read",
    "thread/resume",
    "thread/unsubscribe",
];

pub fn is_read_only_method(method: &str) -> bool {
    READ_ONLY_METHODS.contains(&method)
}

/// Serialize a proto message to a JSON text frame. Returns `None` only if
/// serialization itself fails (effectively impossible for these types).
pub fn to_json_text<T: Serialize>(msg: &T) -> Option<String> {
    match serde_json::to_string(msg) {
        Ok(text) => Some(text),
        Err(err) => {
            tracing::warn!(%err, "failed to serialize proto message");
            None
        }
    }
}
