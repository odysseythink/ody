//! In-process serving for the builtin MCP servers (feature
//! `bundled-builtin-mcp`).
//!
//! When this feature is enabled, stdio launches of the `ody-builtin-mcp`
//! helper binary for the four builtin servers (`fetch`, `sequentialthinking`,
//! `arxiv`, `context7`) are redirected to [`ody_rmcp_client::RmcpClient`]'s
//! in-process transport: the server handler is served inside the ody process
//! over a `tokio::io::DuplexStream` pair, so the distributed binary does not
//! need a separate `ody-builtin-mcp` executable on `PATH`.
//!
//! Only the exact builtin launcher shape is intercepted — local environment,
//! the default command with exactly the server-name argument, and no custom
//! env / env_vars / cwd. Anything else (including user overrides that keep
//! the helper command but add environment) falls through to the normal stdio
//! child-process path.

use std::collections::HashMap;
use std::io;
use std::sync::Arc;

use futures::future::BoxFuture;
use futures::future::FutureExt;
use ody_config::types::McpServerEnvVar;
use ody_rmcp_client::InProcessTransportFactory;
use tokio::io::DuplexStream;
use tracing::info;

use crate::builtin::BUILTIN_MCP_SERVER_COMMAND;
use crate::builtin::BUILTIN_MCP_SERVERS;

/// Buffer size for the in-process client/server byte streams.
///
/// MCP messages (especially tool results) can be large; the duplex buffer
/// only bounds in-flight buffering, not message size.
const DUPLEX_BUFFER_SIZE: usize = 256 * 1024;

/// Factory that serves one builtin MCP server in-process per connection.
///
/// A fresh server instance is served for every `open()` call so reconnects
/// behave exactly like respawning the `ody-builtin-mcp` child process.
#[derive(Debug)]
struct BuiltinInProcessTransportFactory {
    server_name: &'static str,
}

impl InProcessTransportFactory for BuiltinInProcessTransportFactory {
    fn open(&self) -> BoxFuture<'static, io::Result<DuplexStream>> {
        let server_name = self.server_name;
        async move {
            let (client_stream, server_stream) = tokio::io::duplex(DUPLEX_BUFFER_SIZE);
            tokio::spawn(async move {
                if let Err(error) = ody_builtin_mcp::serve_builtin(server_name, server_stream).await
                {
                    tracing::warn!(
                        server = server_name,
                        "in-process builtin MCP server exited with error: {error:#}"
                    );
                }
            });
            info!(
                server = server_name,
                "serving builtin MCP server in-process (bundled-builtin-mcp)"
            );
            Ok(client_stream)
        }
        .boxed()
    }
}

/// Returns an in-process transport factory when the stdio launch describes
/// exactly one of the builtin MCP servers in their default shape.
///
/// Returns `None` (caller falls through to the normal stdio child-process
/// launch) when any of the following holds:
///
/// - the launch targets a non-local environment (the helper binary must be
///   spawned there);
/// - the command/args do not match `ody-builtin-mcp <builtin-name>`;
/// - the config adds custom `env`, `env_vars`, or `cwd` that the in-process
///   server would silently ignore.
pub(crate) fn in_process_factory_for_stdio_command(
    command: &str,
    args: &[String],
    env: Option<&HashMap<String, String>>,
    env_vars: &[McpServerEnvVar],
    cwd: Option<&str>,
    is_local_environment: bool,
) -> Option<Arc<dyn InProcessTransportFactory>> {
    if !is_local_environment {
        return None;
    }
    if command != BUILTIN_MCP_SERVER_COMMAND {
        return None;
    }
    if env.is_some() || !env_vars.is_empty() || cwd.is_some() {
        return None;
    }
    let [server_name] = args else {
        return None;
    };
    let builtin = BUILTIN_MCP_SERVERS
        .iter()
        .find(|builtin| builtin.name == server_name)?;
    Some(Arc::new(BuiltinInProcessTransportFactory {
        server_name: builtin.name,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env() -> Option<HashMap<String, String>> {
        None
    }

    #[test]
    fn intercepts_default_builtin_launch_shape() {
        for builtin in BUILTIN_MCP_SERVERS {
            let factory = in_process_factory_for_stdio_command(
                BUILTIN_MCP_SERVER_COMMAND,
                &[builtin.name.to_string()],
                no_env().as_ref(),
                &[],
                None,
                true,
            );
            assert!(
                factory.is_some(),
                "{} should be served in-process",
                builtin.name
            );
        }
    }

    #[test]
    fn ignores_non_builtin_commands() {
        assert!(
            in_process_factory_for_stdio_command(
                "npx",
                &["-y".to_string(), "some-server".to_string()],
                no_env().as_ref(),
                &[],
                None,
                true,
            )
            .is_none()
        );
    }

    #[test]
    fn ignores_unknown_builtin_name_and_extra_args() {
        assert!(
            in_process_factory_for_stdio_command(
                BUILTIN_MCP_SERVER_COMMAND,
                &["not-a-server".to_string()],
                no_env().as_ref(),
                &[],
                None,
                true,
            )
            .is_none()
        );
        assert!(
            in_process_factory_for_stdio_command(
                BUILTIN_MCP_SERVER_COMMAND,
                &["fetch".to_string(), "extra".to_string()],
                no_env().as_ref(),
                &[],
                None,
                true,
            )
            .is_none()
        );
    }

    #[test]
    fn ignores_customized_builtin_configs() {
        // A user override that keeps the helper command but adds env or cwd
        // must keep going through the real child process so the customization
        // is not silently dropped.
        let env = HashMap::from([("CONTEXT7_API_KEY".to_string(), "k".to_string())]);
        assert!(
            in_process_factory_for_stdio_command(
                BUILTIN_MCP_SERVER_COMMAND,
                &["context7".to_string()],
                Some(&env),
                &[],
                None,
                true,
            )
            .is_none()
        );
        assert!(
            in_process_factory_for_stdio_command(
                BUILTIN_MCP_SERVER_COMMAND,
                &["context7".to_string()],
                no_env().as_ref(),
                &[],
                Some("/tmp"),
                true,
            )
            .is_none()
        );
    }

    #[test]
    fn ignores_non_local_environments() {
        assert!(
            in_process_factory_for_stdio_command(
                BUILTIN_MCP_SERVER_COMMAND,
                &["fetch".to_string()],
                no_env().as_ref(),
                &[],
                None,
                false,
            )
            .is_none()
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn factory_open_starts_serving_builtin() {
        use rmcp::service::serve_client;

        let factory = in_process_factory_for_stdio_command(
            BUILTIN_MCP_SERVER_COMMAND,
            &["sequentialthinking".to_string()],
            no_env().as_ref(),
            &[],
            None,
            true,
        )
        .expect("sequentialthinking should be intercepted");
        let stream = factory.open().await.expect("open");
        let client = serve_client((), stream).await.expect("handshake");
        let tools = client.list_all_tools().await.expect("list tools");
        assert_eq!(
            tools
                .into_iter()
                .map(|t| t.name.to_string())
                .collect::<Vec<_>>(),
            vec!["sequentialthinking"]
        );
        client.cancel().await.unwrap();
    }
}
