//! Rust implementations of the odyBox builtin MCP servers.
//!
//! Each module exposes one MCP server (stdio transport) that mirrors the
//! tool surface of the corresponding chatbox builtin server:
//!
//! - [`fetch`]: web page fetching with HTML -> markdown conversion
//! - [`sequentialthinking`]: structured, reflective problem solving
//! - [`arxiv`]: arXiv paper search / download / list / read
//! - [`context7`]: up-to-date library documentation lookup
//!
//! All servers are served by the `ody-builtin-mcp` binary, one subcommand
//! per server, so ody can spawn them as stdio child processes. When ody is
//! built with its `bundled-builtin-mcp` feature, the same servers are instead
//! served in-process over a tokio duplex stream via [`serve_builtin`], and
//! the helper binary is not needed at runtime.

pub mod arxiv;
pub mod context7;
pub mod fetch;
pub mod sequentialthinking;

use rmcp::RoleServer;
use rmcp::transport::IntoTransport;

/// Serve the named builtin server over an arbitrary rmcp transport.
///
/// `name` must be one of `fetch`, `sequentialthinking`, `arxiv`, or
/// `context7`. This is the entry point used by ody's `bundled-builtin-mcp`
/// feature to run builtin servers in-process (typically over a
/// `tokio::io::DuplexStream` pair) instead of spawning the
/// `ody-builtin-mcp` binary as a stdio child process.
pub async fn serve_builtin<T, E, A>(name: &str, transport: T) -> anyhow::Result<()>
where
    T: IntoTransport<RoleServer, E, A>,
    E: std::error::Error + Send + Sync + 'static,
{
    match name {
        "fetch" => fetch::serve(transport).await,
        "sequentialthinking" => sequentialthinking::serve(transport).await,
        "arxiv" => arxiv::serve(transport).await,
        "context7" => context7::serve(transport).await,
        other => anyhow::bail!("unknown builtin MCP server `{other}`"),
    }
}

#[cfg(test)]
mod tests {
    use super::serve_builtin;
    use rmcp::service::serve_client;
    use rmcp::transport::IntoTransport;
    use rmcp::transport::async_rw::TransportAdapterAsyncCombinedRW;

    /// Each builtin server must complete the rmcp handshake and list its
    /// tools when served in-process over a tokio duplex stream (the transport
    /// used by ody's `bundled-builtin-mcp` feature).
    #[tokio::test(flavor = "multi_thread")]
    async fn serve_builtin_handshakes_over_duplex_stream() {
        let cases = [
            ("fetch", vec!["fetch"]),
            ("sequentialthinking", vec!["sequentialthinking"]),
            (
                "arxiv",
                vec!["search_papers", "download_paper", "list_papers", "read_paper"],
            ),
            ("context7", vec!["resolve-library-id", "query-docs"]),
        ];
        for (name, expected_tools) in cases {
            let (client_stream, server_stream) = tokio::io::duplex(64 * 1024);
            tokio::spawn(async move {
                serve_builtin(name, server_stream)
                    .await
                    .unwrap_or_else(|e| panic!("serving {name} failed: {e:#}"));
            });
            let client = serve_client((), client_stream).await.unwrap();
            let tools = client.list_all_tools().await.unwrap();
            let mut tool_names = tools
                .into_iter()
                .map(|tool| tool.name.to_string())
                .collect::<Vec<_>>();
            tool_names.sort();
            let mut expected_tools = expected_tools.clone();
            expected_tools.sort();
            assert_eq!(tool_names, expected_tools, "tools for {name}");
            client.cancel().await.unwrap();
        }
    }

    #[test]
    fn serve_builtin_rejects_unknown_names() {
        // The dispatcher must fail loudly rather than silently serving
        // nothing for a typo'd server name.
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let err = runtime
            .block_on(async {
                let (_client, server) = tokio::io::duplex(64);
                serve_builtin("nope", server).await
            })
            .unwrap_err();
        assert!(format!("{err:#}").contains("unknown builtin MCP server"));
    }

    #[test]
    fn duplex_stream_implements_rmcp_transport() {
        // Compile-time guarantee that the factory's transport type satisfies
        // rmcp's blanket IntoTransport impl for combined AsyncRead + AsyncWrite.
        fn assert_transport<T>(_: std::marker::PhantomData<T>)
        where
            T: IntoTransport<rmcp::RoleServer, std::io::Error, TransportAdapterAsyncCombinedRW>,
        {
        }
        assert_transport::<tokio::io::DuplexStream>(std::marker::PhantomData);
    }
}
