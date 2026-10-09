use ody_remote::gateway::auth::hash_password;
use ody_remote::gateway::server::GatewayConfig;

#[test]
fn parses_valid_gateway_config() {
    let hash = hash_password("pw");
    let toml = format!(
        r#"
        username = "ranwei"
        password_hash = "{hash}"
        agent_token = "0123456789abcdef"
        "#
    );
    let config = GatewayConfig::from_toml_str(&toml).expect("parse");
    assert_eq!(config.username, "ranwei");
    assert_eq!(config.session_ttl.as_secs(), 7 * 24 * 3600);
}

#[test]
fn rejects_short_agent_token() {
    let hash = hash_password("pw");
    let toml = format!(
        r#"
        username = "ranwei"
        password_hash = "{hash}"
        agent_token = "short"
        "#
    );
    assert!(GatewayConfig::from_toml_str(&toml).is_err());
}

#[test]
fn rejects_malformed_password_hash() {
    let toml = r#"
        username = "ranwei"
        password_hash = "not-argon2"
        agent_token = "0123456789abcdef"
    "#;
    assert!(GatewayConfig::from_toml_str(toml).is_err());
}
