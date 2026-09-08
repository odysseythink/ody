//! Built-in MCP servers shipped with Ody.
//!
//! These mirror the "builtin MCP servers" that the odyBox desktop UI exposes,
//! reimplemented in Rust in `mcp-server/builtins` (the `ody-builtin-mcp`
//! binary, one subcommand per server) and served over stdio. The helper
//! binary is resolved like any other stdio command: on Windows the loader
//! also searches the directory of the spawning executable, so installing it
//! next to the ody entrypoint works; on other platforms it must be on `PATH`.
//!
//! Built-ins are registered into the MCP catalog with the lowest precedence,
//! so a config `mcp_servers` entry with the same name (or a plugin
//! contribution) always overrides them. Users can turn individual built-in
//! servers off via the `disabled_builtin_mcp_servers` config key.

use ody_config::McpServerConfig;
use ody_config::McpServerTransportConfig;
use ody_config::DEFAULT_MCP_SERVER_ENVIRONMENT_ID;

/// Command used to launch built-in MCP servers. The binary ships with the ody
/// package (on `PATH`, or next to the entrypoint executable on Windows).
pub const BUILTIN_MCP_SERVER_COMMAND: &str = "ody-builtin-mcp";

/// A built-in MCP server declaration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinMcpServer {
    /// Stable catalog name; also the key used in `disabled_builtin_mcp_servers`
    /// and the `ody-builtin-mcp` subcommand that serves this server.
    pub name: &'static str,
}

/// Built-in MCP servers enabled by default.
pub const BUILTIN_MCP_SERVERS: &[BuiltinMcpServer] = &[
    BuiltinMcpServer { name: "fetch" },
    BuiltinMcpServer {
        name: "sequentialthinking",
    },
    BuiltinMcpServer { name: "arxiv" },
    BuiltinMcpServer { name: "context7" },
];

impl BuiltinMcpServer {
    /// Materializes this built-in server as a regular MCP server config.
    pub fn to_mcp_server_config(&self) -> McpServerConfig {
        McpServerConfig {
            transport: McpServerTransportConfig::Stdio {
                command: BUILTIN_MCP_SERVER_COMMAND.to_string(),
                args: vec![self.name.to_string()],
                env: None,
                env_vars: Vec::new(),
                cwd: None,
            },
            environment_id: DEFAULT_MCP_SERVER_ENVIRONMENT_ID.to_string(),
            enabled: true,
            required: false,
            supports_parallel_tool_calls: false,
            disabled_reason: None,
            startup_timeout_sec: None,
            tool_timeout_sec: None,
            default_tools_approval_mode: None,
            enabled_tools: None,
            disabled_tools: None,
            scopes: None,
            oauth: None,
            oauth_resource: None,
            tools: Default::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_servers_have_unique_names_and_stdio_configs() {
        let mut names = std::collections::BTreeSet::new();
        for server in BUILTIN_MCP_SERVERS {
            assert!(names.insert(server.name), "duplicate name {}", server.name);
            let config = server.to_mcp_server_config();
            let McpServerTransportConfig::Stdio { command, args, .. } = config.transport else {
                panic!("builtin server {} must use stdio", server.name);
            };
            assert_eq!(command, BUILTIN_MCP_SERVER_COMMAND);
            assert_eq!(args, vec![server.name.to_string()]);
            assert!(config.enabled);
        }
    }

    #[test]
    fn edgeone_pages_is_removed() {
        assert!(!BUILTIN_MCP_SERVERS
            .iter()
            .any(|server| server.name == "edgeone-pages"));
    }
}
