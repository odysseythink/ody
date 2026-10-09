//! The gateway's in-memory routing hub: which agent owns which instance,
//! which browser clients are connected, and pending-RPC bookkeeping.

use crate::proto::GatewayToAgent;
use crate::proto::GatewayToClient;
use crate::proto::InstanceDescriptor;
use crate::proto::InstanceInfo;
use crate::proto::RpcOutcome;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use tokio::sync::mpsc;

pub type AgentId = u64;
pub type ClientId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteError {
    UnknownInstance,
    AgentGone,
}

struct AgentEntry {
    tx: mpsc::Sender<GatewayToAgent>,
}

#[derive(Clone)]
struct InstanceEntry {
    agent: AgentId,
    machine: String,
    name: String,
    online: bool,
}

struct PendingRpc {
    client: ClientId,
    client_rpc_id: u64,
}

#[derive(Default)]
struct HubState {
    next_agent_id: AgentId,
    next_client_id: ClientId,
    next_rpc_id: u64,
    agents: HashMap<AgentId, AgentEntry>,
    instances: HashMap<String, InstanceEntry>,
    clients: HashMap<ClientId, mpsc::Sender<GatewayToClient>>,
    pending: HashMap<u64, PendingRpc>,
}

/// Shared routing hub, cheap to clone into handlers.
#[derive(Clone, Default)]
pub struct Hub {
    state: Arc<Mutex<HubState>>,
}

impl Hub {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn agent_connected(
        &self,
        machine: String,
        instances: Vec<InstanceDescriptor>,
        tx: mpsc::Sender<GatewayToAgent>,
    ) -> AgentId {
        let mut state = crate::lock(&self.state);
        let agent_id = state.next_agent_id;
        state.next_agent_id += 1;
        state.agents.insert(agent_id, AgentEntry { tx });
        for instance in instances {
            state.instances.insert(
                instance.id,
                InstanceEntry {
                    agent: agent_id,
                    machine: machine.clone(),
                    name: instance.name,
                    online: true,
                },
            );
        }
        Self::broadcast_instances_locked(&mut state);
        agent_id
    }

    pub async fn agent_disconnected(&self, agent_id: AgentId) {
        let mut state = crate::lock(&self.state);
        state.agents.remove(&agent_id);
        for instance in state.instances.values_mut() {
            if instance.agent == agent_id {
                instance.online = false;
            }
        }
        Self::broadcast_instances_locked(&mut state);
    }

    pub async fn set_instance_status(&self, instance: &str, online: bool) {
        let mut state = crate::lock(&self.state);
        if let Some(entry) = state.instances.get_mut(instance) {
            entry.online = online;
            Self::broadcast_instances_locked(&mut state);
        }
    }

    pub async fn list_instances(&self) -> Vec<InstanceInfo> {
        let state = crate::lock(&self.state);
        Self::instances_locked(&state)
    }

    pub async fn client_connected(&self, tx: mpsc::Sender<GatewayToClient>) -> ClientId {
        let mut state = crate::lock(&self.state);
        let client_id = state.next_client_id;
        state.next_client_id += 1;
        let snapshot = GatewayToClient::Instances {
            list: Self::instances_locked(&state),
        };
        // Best effort: if the client is already gone, just don't register.
        if tx.try_send(snapshot).is_ok() {
            state.clients.insert(client_id, tx);
        }
        client_id
    }

    pub async fn client_disconnected(&self, client_id: ClientId) {
        let mut state = crate::lock(&self.state);
        state.clients.remove(&client_id);
        state.pending.retain(|_, p| p.client != client_id);
    }

    /// Remap the client-facing rpc id and forward to the owning agent.
    pub async fn route_rpc(
        &self,
        client: ClientId,
        client_rpc_id: u64,
        instance: &str,
        method: String,
        params: serde_json::Value,
    ) -> Result<(), RouteError> {
        let mut state = crate::lock(&self.state);
        let agent_tx = {
            let entry = state
                .instances
                .get(instance)
                .ok_or(RouteError::UnknownInstance)?;
            state
                .agents
                .get(&entry.agent)
                .ok_or(RouteError::AgentGone)?
                .tx
                .clone()
        };
        let gw_id = state.next_rpc_id;
        state.next_rpc_id += 1;
        let msg = GatewayToAgent::Rpc {
            id: gw_id,
            instance: instance.to_string(),
            method,
            params,
        };
        // try_send first so a dead agent does not leave a pending entry.
        if agent_tx.try_send(msg).is_err() {
            return Err(RouteError::AgentGone);
        }
        state.pending.insert(
            gw_id,
            PendingRpc {
                client,
                client_rpc_id,
            },
        );
        Ok(())
    }

    /// Forward an agent's response to the originating browser client.
    pub async fn resolve_rpc(&self, gw_id: u64, outcome: RpcOutcome) {
        let mut state = crate::lock(&self.state);
        let Some(pending) = state.pending.remove(&gw_id) else {
            return;
        };
        if let Some(tx) = state.clients.get(&pending.client) {
            let msg = GatewayToClient::RpcResponse {
                id: pending.client_rpc_id,
                outcome,
            };
            if tx.try_send(msg).is_err() {
                state.clients.remove(&pending.client);
            }
        }
    }

    /// Broadcast an instance event to every connected browser client.
    // ody: 不做按实例的订阅过滤，所有客户端收全量事件；升级触发条件=多用户隔离或单实例事件洪泛压垮手机端时加订阅表。
    pub async fn fanout_event(&self, instance: &str, payload: serde_json::Value) {
        let mut state = crate::lock(&self.state);
        let msg = GatewayToClient::Event {
            instance: instance.to_string(),
            payload,
        };
        state
            .clients
            .retain(|_, tx| tx.try_send(msg.clone()).is_ok());
    }

    fn instances_locked(state: &HubState) -> Vec<InstanceInfo> {
        let mut list: Vec<InstanceInfo> = state
            .instances
            .iter()
            .map(|(id, e)| InstanceInfo {
                id: id.clone(),
                machine: e.machine.clone(),
                name: e.name.clone(),
                online: e.online,
            })
            .collect();
        list.sort_by(|a, b| a.id.cmp(&b.id));
        list
    }

    fn broadcast_instances_locked(state: &mut HubState) {
        let msg = GatewayToClient::Instances {
            list: Self::instances_locked(state),
        };
        state
            .clients
            .retain(|_, tx| tx.try_send(msg.clone()).is_ok());
    }
}
