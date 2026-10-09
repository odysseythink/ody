use ody_remote::agent::config::AgentConfig;

#[test]
fn parses_minimal_config() {
    let toml = r#"
        gateway_url = "wss://gateway.example.com/ws/agent"
        token = "secret"
        machine = "mac-a"

        [[instances]]
        name = "inst-1"
        url = "ws://127.0.0.1:8781"

        [[instances]]
        name = "inst-2"
        url = "ws://127.0.0.1:8782"
        token = "cap-token"
    "#;
    let config = AgentConfig::from_toml_str(toml).expect("parse");
    assert_eq!(config.gateway_url, "wss://gateway.example.com/ws/agent");
    assert_eq!(config.machine, "mac-a");
    assert_eq!(config.instances.len(), 2);
    assert_eq!(config.instances[0].id(), "mac-a/inst-1");
    assert_eq!(config.instances[1].token.as_deref(), Some("cap-token"));
    assert_eq!(config.instances[0].token, None);
}

#[test]
fn rejects_missing_instances() {
    let toml = r#"
        gateway_url = "wss://g.example.com/ws/agent"
        token = "secret"
        machine = "mac-a"
    "#;
    assert!(AgentConfig::from_toml_str(toml).is_err());
}

#[test]
fn rejects_duplicate_instance_names() {
    let toml = r#"
        gateway_url = "wss://g.example.com/ws/agent"
        token = "secret"
        machine = "mac-a"
        [[instances]]
        name = "i1"
        url = "ws://127.0.0.1:8781"
        [[instances]]
        name = "i1"
        url = "ws://127.0.0.1:8782"
    "#;
    assert!(AgentConfig::from_toml_str(toml).is_err());
}

#[test]
fn rejects_non_websocket_urls() {
    let toml = r#"
        gateway_url = "https://g.example.com/ws/agent"
        token = "secret"
        machine = "mac-a"
        [[instances]]
        name = "i1"
        url = "http://127.0.0.1:8781"
    "#;
    assert!(AgentConfig::from_toml_str(toml).is_err());
}

#[test]
fn rejects_invalid_toml() {
    assert!(AgentConfig::from_toml_str("not [toml").is_err());
}

#[test]
fn parses_gateway_ca_cert_absolute_path() {
    let ca = if cfg!(windows) {
        "C:\\certs\\ca.pem"
    } else {
        "/tmp/ca.pem"
    };
    let toml = format!(
        r#"
        gateway_url = "wss://gateway.example.com/ws/agent"
        token = "secret"
        machine = "mac-a"
        gateway_ca_cert = "{ca}"

        [[instances]]
        name = "i1"
        url = "ws://127.0.0.1:8781"
        "#
    );
    let config = AgentConfig::from_toml_str(&toml).expect("parse");
    let parsed = config.gateway_ca_cert.expect("ca cert path");
    assert_eq!(parsed.as_path(), std::path::Path::new(ca));
}

#[test]
fn defaults_gateway_ca_cert_to_none() {
    let toml = r#"
        gateway_url = "wss://gateway.example.com/ws/agent"
        token = "secret"
        machine = "mac-a"

        [[instances]]
        name = "i1"
        url = "ws://127.0.0.1:8781"
    "#;
    let config = AgentConfig::from_toml_str(toml).expect("parse");
    assert_eq!(config.gateway_ca_cert, None);
}

#[test]
fn rejects_relative_gateway_ca_cert() {
    let toml = r#"
        gateway_url = "wss://gateway.example.com/ws/agent"
        token = "secret"
        machine = "mac-a"
        gateway_ca_cert = "certs/ca.pem"

        [[instances]]
        name = "i1"
        url = "ws://127.0.0.1:8781"
    "#;
    assert!(AgentConfig::from_toml_str(toml).is_err());
}
