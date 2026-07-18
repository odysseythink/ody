use super::*;
use ody_plugin::AppConnectorId;
use pretty_assertions::assert_eq;
use std::collections::HashMap;

fn app(name: &str) -> AppDeclaration {
    AppDeclaration {
        name: name.to_string(),
        connector_id: AppConnectorId(format!("connector_{name}")),
        category: None,
    }
}

fn mcp_servers(mcp_servers: impl IntoIterator<Item = (&'static str, i32)>) -> HashMap<String, i32> {
    mcp_servers
        .into_iter()
        .map(|(name, value)| (name.to_string(), value))
        .collect::<HashMap<_, _>>()
}

fn sorted_mcp_server_names(mcp_servers: &HashMap<String, i32>) -> Vec<String> {
    let mut names = mcp_servers.keys().cloned().collect::<Vec<_>>();
    names.sort();
    names
}

#[test]
fn apps_route_available_is_always_false() {
    assert!(!apps_route_available(Some(AuthMode::ApiKey)));
    assert!(!apps_route_available(Some(AuthMode::Unauthenticated)));
    assert!(!apps_route_available(/*auth_mode*/ None));
}

#[test]
fn app_mcp_routing_clears_apps_when_apps_route_is_unavailable() {
    let mut apps = vec![app("linear")];
    let mut mcp_servers = mcp_servers([("linear", 1), ("docs", 2)]);

    apply_app_mcp_routing_policy(
        &mut apps,
        &mut mcp_servers,
        Some(AuthMode::ApiKey),
        /*plugin_active*/ true,
    );

    assert!(apps.is_empty());
    assert_eq!(
        sorted_mcp_server_names(&mcp_servers),
        vec!["docs".to_string(), "linear".to_string()]
    );
}

#[test]
fn app_mcp_routing_clears_apps_regardless_of_plugin_state() {
    let mut apps = vec![app("linear"), app("notion")];
    let mut mcp_servers = mcp_servers([("linear", 1), ("docs", 2), ("notion", 3)]);

    apply_app_mcp_routing_policy(
        &mut apps,
        &mut mcp_servers,
        Some(AuthMode::ApiKey),
        /*plugin_active*/ true,
    );

    assert!(apps.is_empty());
    assert_eq!(
        sorted_mcp_server_names(&mcp_servers),
        vec![
            "docs".to_string(),
            "linear".to_string(),
            "notion".to_string()
        ]
    );
}
