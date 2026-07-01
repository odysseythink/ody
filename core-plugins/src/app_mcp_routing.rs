use ody_app_server_protocol::AuthMode;
use ody_plugin::AppDeclaration;
use std::collections::HashMap;
use std::collections::HashSet;

pub fn apps_route_available(_auth_mode: Option<AuthMode>) -> bool {
    false
}

pub(crate) fn apply_app_mcp_routing_policy<M>(
    apps: &mut Vec<AppDeclaration>,
    mcp_servers: &mut HashMap<String, M>,
    auth_mode: Option<AuthMode>,
    plugin_active: bool,
) {
    if !apps_route_available(auth_mode) {
        apps.clear();
        return;
    }

    if plugin_active && !apps.is_empty() {
        let app_declaration_names = apps
            .iter()
            .map(|app| app.name.as_str())
            .collect::<HashSet<_>>();
        mcp_servers.retain(|name, _| !app_declaration_names.contains(name.as_str()));
    }
}

#[cfg(test)]
#[path = "app_mcp_routing_tests.rs"]
mod tests;
