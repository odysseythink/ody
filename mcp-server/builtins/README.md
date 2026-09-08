# ody-builtin-mcp

Rust implementations of the odyBox builtin MCP servers, based on
[rmcp](https://github.com/modelcontextprotocol/rust-sdk) (the official Rust MCP
SDK). One binary, four stdio servers:

```console
ody-builtin-mcp fetch
ody-builtin-mcp sequentialthinking
ody-builtin-mcp arxiv
ody-builtin-mcp context7
```

| Server | Source alignment |
| --- | --- |
| `fetch` | Adapted from [`mcp-server-fetch` v0.1.0](https://crates.io/crates/mcp-server-fetch) (MIT), rmcp upgraded to the workspace version; prompts dropped |
| `sequentialthinking` | Port of the official [`@modelcontextprotocol/server-sequential-thinking`](https://github.com/modelcontextprotocol/servers/tree/main/src/sequentialthinking) |
| `arxiv` | Tool schema aligned with the chatbox-hosted arXiv server (per `blazickjp/arxiv-mcp-server`); download goes through ar5iv HTML |
| `context7` | Port of [`@upstash/context7-mcp` v3.x](https://github.com/upstash/context7) tools (`resolve-library-id` / `query-docs`) against the Context7 v2 HTTP API |

## In-process bundling (single-binary ody)

Each server's `run()` is a thin wrapper around a generic `serve(transport)`,
and the crate root exposes `serve_builtin(name, transport)` which dispatches
by server name. ody's `bundled-builtin-mcp` Cargo feature
(`ody-cli/bundled-builtin-mcp`, forwarding to `ody-mcp/bundled-builtin-mcp`)
uses this to serve the builtin servers inside the ody process over tokio
duplex streams, so a release build can distribute a single `ody` binary
without the `ody-builtin-mcp` helper executable:

```console
cargo build -p ody-cli --release --features bundled-builtin-mcp
```

## Environment variables

- `ODY_FETCH_USER_AGENT`: custom User-Agent for the fetch server
- `ODY_FETCH_IGNORE_ROBOTS_TXT=1`: skip robots.txt compliance checks
- `ODY_FETCH_PROXY_URL`: proxy for fetch requests
- `ODY_BUILTIN_ARXIV_STORAGE`: paper cache directory (default: platform cache dir)
- `CONTEXT7_API_KEY`: optional Context7 API key (sent as `X-Context7-API-Key`)
