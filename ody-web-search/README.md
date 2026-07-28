# `ody-web-search`

Web search provider registry, fallback chain, and `WebSearch` tool implementation for Ody.

Web search is no longer gated by provider/model capabilities or feature flags. Instead, you opt in by adding a `[services.webSearch]` table to your `~/.ody-code/config.toml`. Ody creates the configured provider(s) at thread start and registers the `WebSearch` tool only when a provider is available.

## Supported providers

The following provider names can be used in `services.webSearch.primary.provider` and `services.webSearch.secondary.provider`:

| Provider | Requires `api_key` | Common `options` |
|---|---|---|
| `duckduckgo` | No | `proxy_url` |
| `searxng` | No | `base_url` (required) |
| `bing` | Yes | `base_url` |
| `moonshot` | Yes | `base_url` |
| `serpapi` | Yes | `base_url` |
| `searchapi` | Yes | `base_url` |
| `serper` | Yes | `base_url` |
| `baidu` | Yes | `base_url`, `top_k` |
| `serply` | Yes | `base_url`, `language`, `hl`, `gl`, `device` |
| `tavily` | Yes | `base_url`, `search_depth` |
| `exa` | Yes | `base_url`, `type`, `livecrawl` |
| `perplexity` | Yes | `base_url`, `max_results`, `max_tokens_per_page` |

For providers that require an API key, you can either set `api_key = "..."` in the config or set the environment variable `<PROVIDER>_API_KEY` (for example, `BING_API_KEY`, `TAVILY_API_KEY`).

## Configuration schema

```toml
[services.webSearch]
primary = { provider = "...", api_key = "...", timeout_ms = 30000, options = { ... } }
# optional fallback provider
secondary = { provider = "...", api_key = "...", timeout_ms = 30000, options = { ... } }
```

Fields:

- `provider` — one of the supported provider names above.
- `api_key` — API key for the provider. Optional for providers that do not require one, or when the equivalent `<PROVIDER>_API_KEY` environment variable is set.
- `timeout_ms` — request timeout in milliseconds. Defaults to provider-specific values when omitted.
- `options` — provider-specific key/value map. Every provider supports `base_url` to override the default endpoint.

## Examples

### Local SearXNG instance

SearXNG does not require an API key.

```toml
[services]
[services.webSearch]
primary = { provider = "searxng", timeout_ms = 30000, options = { base_url = "http://localhost:9999/search" } }
```

### DuckDuckGo

No API key required.

```toml
[services.webSearch]
primary = { provider = "duckduckgo" }
```

### Bing

```toml
[services.webSearch]
primary = { provider = "bing", api_key = "YOUR_BING_API_KEY", options = { base_url = "https://api.bing.microsoft.com/v7.0" } }
```

Or with the environment variable:

```bash
export BING_API_KEY="YOUR_BING_API_KEY"
```

```toml
[services.webSearch]
primary = { provider = "bing", options = { base_url = "https://api.bing.microsoft.com/v7.0" } }
```

### Moonshot

```toml
[services.webSearch]
primary = { provider = "moonshot", api_key = "YOUR_MOONSHOT_API_KEY" }
```

### Perplexity with custom limits

```toml
[services.webSearch]
primary = {
    provider = "perplexity",
    api_key = "YOUR_PERPLEXITY_API_KEY",
    options = {
        base_url = "https://api.perplexity.ai",
        max_results = 10,
        max_tokens_per_page = 4000
    }
}
```

## Fallback provider

When both `primary` and `secondary` are configured, Ody tries `primary` first and automatically falls back to `secondary` if the primary search fails (network error, timeout, rate limit, etc.). If both fail, the error is surfaced to the model with a categorized message.

```toml
[services.webSearch]
primary = { provider = "bing", api_key = "...", options = { base_url = "..." } }
secondary = { provider = "duckduckgo" }
```

## How it works

1. On thread start, `app-server/src/web_search_extension.rs` reads `config.services.webSearch`.
2. It creates the configured providers via `WebSearchProviderRegistry` and wraps them in `FallbackWebSearchProvider`.
3. The provider handle is stored in thread-level extension data.
4. The `WebSearch` tool is exposed to the model only when a provider exists for the current thread.
5. When the model calls `WebSearch`, the tool invokes `provider.search(query, {limit, includeContent})` and returns formatted text (`Title`, `URL`, `Snippet`).

## Running tests

Unit and mock-based integration tests run by default:

```bash
cargo nextest run -p ody-web-search
```

End-to-end tests against a local SearXNG instance are marked `#[ignore]` and require a server on `http://localhost:9999/search`:

```bash
cargo nextest run -p ody-web-search --test e2e_searxng_local --run-ignored all
```

## Migration from `web_search_mode`

The old `web_search_mode` / `web_search` feature flag / `web/run` extension tool paths have been removed. Web search is now purely opt-in via `[services.webSearch]`. If your config still contains `[tools.web_search]`, Ody will reject it with a migration message pointing to `[services.webSearch]`.
