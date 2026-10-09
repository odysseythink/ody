use ody_remote::gateway::hub::Hub;
use ody_remote::proto::*;
use pretty_assertions::assert_eq;
use serde_json::json;
use tokio::sync::mpsc;

fn desc(id: &str) -> InstanceDescriptor {
    InstanceDescriptor {
        id: id.to_string(),
        name: id.rsplit('/').next().unwrap().to_string(),
    }
}

async fn recv_client(rx: &mut mpsc::Receiver<GatewayToClient>) -> GatewayToClient {
    tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
        .await
        .expect("timed out waiting for client message")
        .expect("client channel closed")
}

async fn recv_agent(rx: &mut mpsc::Receiver<GatewayToAgent>) -> GatewayToAgent {
    tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
        .await
        .expect("timed out waiting for agent message")
        .expect("agent channel closed")
}

#[tokio::test]
async fn agent_connect_registers_instances_and_broadcasts_snapshot() {
    let hub = Hub::new();
    let (client_tx, mut client_rx) = mpsc::channel(8);
    let _client = hub.client_connected(client_tx).await;
    // The initial snapshot is empty.
    assert_eq!(
        recv_client(&mut client_rx).await,
        GatewayToClient::Instances { list: vec![] }
    );

    let (agent_tx, _agent_rx) = mpsc::channel(8);
    hub.agent_connected(
        "mac-a".to_string(),
        vec![desc("mac-a/inst-1"), desc("mac-a/inst-2")],
        agent_tx,
    )
    .await;

    match recv_client(&mut client_rx).await {
        GatewayToClient::Instances { list } => {
            assert_eq!(list.len(), 2);
            assert!(list.iter().all(|i| i.online));
            assert_eq!(list[0].machine, "mac-a");
        }
        other => panic!("expected Instances push, got {other:?}"),
    }
    assert_eq!(hub.list_instances().await.len(), 2);
}

#[tokio::test]
async fn second_agent_adds_its_instances() {
    let hub = Hub::new();
    let (tx_a, _rx_a) = mpsc::channel(8);
    let (tx_b, _rx_b) = mpsc::channel(8);
    hub.agent_connected("mac-a".to_string(), vec![desc("mac-a/i1")], tx_a)
        .await;
    hub.agent_connected("mac-b".to_string(), vec![desc("mac-b/i1")], tx_b)
        .await;
    let ids: Vec<String> = hub
        .list_instances()
        .await
        .into_iter()
        .map(|i| i.id)
        .collect();
    assert_eq!(ids, vec!["mac-a/i1".to_string(), "mac-b/i1".to_string()]);
}

#[tokio::test]
async fn agent_disconnect_marks_instances_offline() {
    let hub = Hub::new();
    let (agent_tx, _agent_rx) = mpsc::channel(8);
    let agent = hub
        .agent_connected("mac-a".to_string(), vec![desc("mac-a/i1")], agent_tx)
        .await;
    hub.agent_disconnected(agent).await;
    let list = hub.list_instances().await;
    assert_eq!(list.len(), 1);
    assert!(!list[0].online);
}

#[tokio::test]
async fn route_rpc_forwards_to_owning_agent_with_remapped_id() {
    let hub = Hub::new();
    let (agent_tx, mut agent_rx) = mpsc::channel(8);
    hub.agent_connected("mac-a".to_string(), vec![desc("mac-a/i1")], agent_tx)
        .await;
    let (client_tx, _client_rx) = mpsc::channel(8);
    let client = hub.client_connected(client_tx).await;

    hub.route_rpc(
        client,
        99,
        "mac-a/i1",
        "thread/list".to_string(),
        json!({"limit": 5}),
    )
    .await
    .expect("route should succeed");

    match recv_agent(&mut agent_rx).await {
        GatewayToAgent::Rpc {
            id,
            instance,
            method,
            params,
        } => {
            assert_ne!(id, 99, "gateway must remap the client-facing id");
            assert_eq!(instance, "mac-a/i1");
            assert_eq!(method, "thread/list");
            assert_eq!(params, json!({"limit": 5}));
        }
        other => panic!("expected Rpc, got {other:?}"),
    }
}

#[tokio::test]
async fn route_rpc_to_unknown_instance_fails() {
    let hub = Hub::new();
    let (client_tx, _client_rx) = mpsc::channel(8);
    let client = hub.client_connected(client_tx).await;
    let err = hub
        .route_rpc(client, 1, "nope/i1", "thread/list".to_string(), json!({}))
        .await;
    assert!(err.is_err());
}

#[tokio::test]
async fn resolve_rpc_forwards_outcome_to_originating_client() {
    let hub = Hub::new();
    let (agent_tx, mut agent_rx) = mpsc::channel(8);
    hub.agent_connected("mac-a".to_string(), vec![desc("mac-a/i1")], agent_tx)
        .await;
    let (client_tx, mut client_rx) = mpsc::channel(8);
    let client = hub.client_connected(client_tx).await;
    let _snapshot = recv_client(&mut client_rx).await;

    hub.route_rpc(client, 7, "mac-a/i1", "thread/read".to_string(), json!({}))
        .await
        .expect("route");
    let gw_id = match recv_agent(&mut agent_rx).await {
        GatewayToAgent::Rpc { id, .. } => id,
        other => panic!("expected Rpc, got {other:?}"),
    };

    hub.resolve_rpc(
        gw_id,
        RpcOutcome::Ok {
            value: json!({"thread": {}}),
        },
    )
    .await;

    assert_eq!(
        recv_client(&mut client_rx).await,
        GatewayToClient::RpcResponse {
            id: 7,
            outcome: RpcOutcome::Ok {
                value: json!({"thread": {}})
            },
        }
    );
}

#[tokio::test]
async fn resolve_rpc_with_unknown_id_is_ignored() {
    let hub = Hub::new();
    hub.resolve_rpc(
        12345,
        RpcOutcome::Err {
            message: "late".to_string(),
        },
    )
    .await;
}

#[tokio::test]
async fn events_fan_out_to_all_connected_clients() {
    let hub = Hub::new();
    let (tx1, mut rx1) = mpsc::channel(8);
    let (tx2, mut rx2) = mpsc::channel(8);
    hub.client_connected(tx1).await;
    hub.client_connected(tx2).await;
    let _snap1 = recv_client(&mut rx1).await;
    let _snap2 = recv_client(&mut rx2).await;

    hub.fanout_event("mac-a/i1", json!({"method": "turn/started"}))
        .await;

    for rx in [&mut rx1, &mut rx2] {
        assert_eq!(
            recv_client(rx).await,
            GatewayToClient::Event {
                instance: "mac-a/i1".to_string(),
                payload: json!({"method": "turn/started"}),
            }
        );
    }
}

#[tokio::test]
async fn disconnected_client_is_dropped_from_fanout() {
    let hub = Hub::new();
    let (tx1, rx1) = mpsc::channel(8);
    let client = hub.client_connected(tx1).await;
    drop(rx1);
    hub.fanout_event("mac-a/i1", json!({"method": "x"})).await;
    hub.fanout_event("mac-a/i1", json!({"method": "y"})).await;
    // No panic and no bookkeeping leak: a fresh list call still works.
    assert!(hub.list_instances().await.is_empty());
}

#[tokio::test]
async fn status_update_flips_online_flag_and_broadcasts() {
    let hub = Hub::new();
    let (client_tx, mut client_rx) = mpsc::channel(8);
    hub.client_connected(client_tx).await;
    let _snap = recv_client(&mut client_rx).await;
    let (agent_tx, _agent_rx) = mpsc::channel(8);
    hub.agent_connected("mac-a".to_string(), vec![desc("mac-a/i1")], agent_tx)
        .await;
    let _push = recv_client(&mut client_rx).await;

    hub.set_instance_status("mac-a/i1", false).await;
    assert!(!hub.list_instances().await[0].online);
    match recv_client(&mut client_rx).await {
        GatewayToClient::Instances { list } => assert!(!list[0].online),
        other => panic!("expected Instances push, got {other:?}"),
    }
}

#[tokio::test]
async fn reconnected_agent_restores_instances_online() {
    let hub = Hub::new();
    let (tx1, _rx1) = mpsc::channel(8);
    let agent = hub
        .agent_connected("mac-a".to_string(), vec![desc("mac-a/i1")], tx1)
        .await;
    hub.agent_disconnected(agent).await;
    assert!(!hub.list_instances().await[0].online);

    let (tx2, _rx2) = mpsc::channel(8);
    hub.agent_connected("mac-a".to_string(), vec![desc("mac-a/i1")], tx2)
        .await;
    assert!(hub.list_instances().await[0].online);
}
