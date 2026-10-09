use ody_remote::proto::*;
use pretty_assertions::assert_eq;
use serde_json::json;

fn roundtrip<T>(msg: &T) -> T
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let text = serde_json::to_string(msg).expect("serialize");
    serde_json::from_str(&text).expect("deserialize")
}

#[test]
fn agent_hello_roundtrips() {
    let msg = AgentToGateway::Hello {
        machine: "mac-a".to_string(),
        instances: vec![
            InstanceDescriptor {
                id: "mac-a/inst-1".to_string(),
                name: "inst-1".to_string(),
            },
            InstanceDescriptor {
                id: "mac-a/inst-2".to_string(),
                name: "inst-2".to_string(),
            },
        ],
    };
    assert_eq!(roundtrip(&msg), msg);
    let text = serde_json::to_string(&msg).expect("serialize");
    assert!(text.contains("\"type\":\"hello\""), "tagged json: {text}");
}

#[test]
fn agent_event_roundtrips_with_raw_payload() {
    let payload = json!({"method": "item/agentMessage/delta", "params": {"delta": "hi"}});
    let msg = AgentToGateway::Event {
        instance: "mac-a/inst-1".to_string(),
        payload: payload.clone(),
    };
    let back = roundtrip(&msg);
    assert_eq!(back, msg);
    match back {
        AgentToGateway::Event { payload: p, .. } => assert_eq!(p, payload),
        other => panic!("unexpected variant: {other:?}"),
    }
}

#[test]
fn rpc_outcome_ok_and_err_roundtrip() {
    let ok = AgentToGateway::RpcResponse {
        id: 7,
        outcome: RpcOutcome::Ok {
            value: json!({"threads": []}),
        },
    };
    assert_eq!(roundtrip(&ok), ok);

    let err = AgentToGateway::RpcResponse {
        id: 8,
        outcome: RpcOutcome::Err {
            message: "instance offline".to_string(),
        },
    };
    assert_eq!(roundtrip(&err), err);
}

#[test]
fn gateway_rpc_to_agent_roundtrips() {
    let msg = GatewayToAgent::Rpc {
        id: 42,
        instance: "mac-a/inst-1".to_string(),
        method: "thread/list".to_string(),
        params: json!({"limit": 20}),
    };
    assert_eq!(roundtrip(&msg), msg);
}

#[test]
fn client_rpc_roundtrips() {
    let msg = ClientToGateway::Rpc {
        id: 1,
        instance: "mac-b/inst-1".to_string(),
        method: "thread/read".to_string(),
        params: json!({"threadId": "t-1", "includeTurns": true}),
    };
    assert_eq!(roundtrip(&msg), msg);
}

#[test]
fn gateway_instances_push_roundtrips() {
    let msg = GatewayToClient::Instances {
        list: vec![InstanceInfo {
            id: "mac-a/inst-1".to_string(),
            machine: "mac-a".to_string(),
            name: "inst-1".to_string(),
            online: true,
        }],
    };
    assert_eq!(roundtrip(&msg), msg);
}

#[test]
fn agent_status_roundtrips() {
    let msg = AgentToGateway::Status {
        instance: "mac-a/inst-1".to_string(),
        online: false,
    };
    assert_eq!(roundtrip(&msg), msg);
}
