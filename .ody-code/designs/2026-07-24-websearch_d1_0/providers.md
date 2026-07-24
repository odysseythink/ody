# Part 3 — 12 个 Provider 实现要点与共性

## 来源 [C:UPSTREAM]

- TS: `packages/agent-core/src/tools/providers/web-search/duckduckgo.ts` — HTML 解析型 provider。
- TS: `packages/agent-core/src/tools/providers/web-search/bing.ts` — JSON GET API 型 provider。
- TS: `packages/agent-core/src/tools/providers/web-search/tavily.ts` — JSON POST API 型 provider。
- TS: `packages/agent-core/src/tools/providers/web-search/exa.ts` — 支持 `includeContent` 的 provider 示例。
- TS: `packages/agent-core/src/tools/providers/web-search/http.ts` — 通用 HTTP 工具函数：buildUrl、getJson、postJson、authHeaderForProvider、fetchWithTimeout、httpError。
- TS: `packages/agent-core/src/tools/providers/web-search/moonshot.ts` — 依赖 `services.moonshotSearch` 配置的 provider 入口。
- TS: `packages/agent-core/src/tools/providers/web-search/registry.ts` — 12 个 provider 名称与默认注册。

## 共性抽象 [C:UPSTREAM]

所有 provider 实现统一遵循以下模式：

```rust
pub struct DuckDuckGoProvider {
    options: DuckDuckGoOptions,
    client: reqwest::Client,
}

impl DuckDuckGoProvider {
    pub fn from_config(
        config: &WebSearchProviderConfig,
        deps: &ProviderFactoryDeps,
    ) -> Result<Self, WebSearchError> {
        let options: DuckDuckGoOptions = serde_json::from_value(
            config.options.clone().unwrap_or_default().into(),
        ).map_err(|e| WebSearchError::InvalidOptions(format!("duckduckgo options: {e}")))?;

        let timeout_ms = config.timeout_ms.unwrap_or(DEFAULT_WEB_SEARCH_TIMEOUT_MS);

        let client = deps.http_client.clone().unwrap_or_else(|| {
            reqwest::Client::builder()
                .timeout(Duration::from_millis(timeout_ms))
                .build()
                .expect("reqwest client builds")
        });

        Ok(Self { options, client })
    }
}

#[async_trait]
impl WebSearchProvider for DuckDuckGoProvider {
    fn name(&self) -> &str { "duckduckgo" }

    async fn search(
        &self,
        query: &str,
        options: WebSearchOptions,
    ) -> Result<Vec<WebSearchResult>, WebSearchError> {
        // 1. 构建请求
        // 2. 发送并检查 HTTP 状态
        // 3. 解析响应
        // 4. 调用 normalize_results
        // 5. 应用 limit
        Ok(vec![])
    }
}
```

### 公共辅助函数

在 `ody-web-search/src/providers/http.rs` 中提供：

```rust
pub fn build_url(base: &str, params: &[(&str, String)]) -> String;

pub fn auth_header_for_provider(name: &str, api_key: &str) -> HeaderMap;

pub fn http_error(response: &reqwest::Response, provider: &str) -> WebSearchError;
```

- `build_url`：与 TS `buildUrl` 对齐，仅包含非空参数。
- `auth_header_for_provider`：与 TS `authHeaderForProvider` 对齐，按 provider 名返回对应 header 集合。
- `http_error`：从响应中提取前 500 字符 body 文本，构造 `WebSearchError::Server` 或 `WebSearchError::Other`。

### 超时处理

- 每个 provider 使用 `reqwest::Client::timeout(Duration::from_millis(timeout_ms))`。
- 如果 `deps.http_client` 已提供，则直接使用该 client（测试注入或统一配置）。
- 超时被 reqwest 抛出后，由 `classify_search_error` 映射为 `WebSearchError::Timeout`。

### 响应规范化

所有 provider 在解析后调用 `normalize_results`（定义见 `trait-crate.md`）：

- 字段映射：`title`/`name` → `title`；`url`/`link`/`uri` → `url`；`snippet`/`description`/`content`/`text` → `snippet`。
- 截断：title 500，url 2048，snippet 4000。
- 过滤：只保留 `title` 和 `url` 非空的结果。
- 应用 `options.limit`：在规范化之后 slice。

---

## 12 个 Provider 实现要点

| # | Provider | 协议/方法 | 认证方式 | 特殊参数 | 响应处理 |
|---|---|---|---|---|---|
| 1 | DuckDuckGo | GET `https://html.duckduckgo.com/html?q=` | 无 | `proxyUrl`（通过代理请求） | 解析 HTML，提取 `result__a` / `result__snippet`；处理 `uddg` 重定向 |
| 2 | Bing | GET `https://api.bing.microsoft.com/v7.0/search` | `Ocp-Apim-Subscription-Key` | `market` | 取 `webPages.value` 数组 |
| 3 | SerpApi | GET/POST `https://serpapi.com/search` | `api_key` query param | 多种 engine | 取 `organic_results` 数组 |
| 4 | SearchApi | GET `https://www.searchapi.io/api/v1/search` | `Authorization: Bearer` | engine | 取 `organic_results` 数组 |
| 5 | Serper | GET `https://google.serper.dev/search` | `X-API-KEY` | engine | 取 `organic` 数组 |
| 6 | Baidu | POST `https://appbuilder.baidu.com/s/...` | `Authorization: Bearer` + `X-Appbuilder-Authorization` | 无 | 取 JSON 中的搜索结果数组 |
| 7 | Serply | GET `https://api.serply.io/v1/search/` | `X-API-KEY` | 无 | 取 `results` 数组 |
| 8 | SearXNG | GET `{baseUrl}/search` | 可选 | `baseUrl`（必需） | 解析 HTML 或 JSON 输出 |
| 9 | Tavily | POST `https://api.tavily.com/search` | body `api_key` | `searchDepth`（basic/advanced） | 取 `results` 数组 |
| 10 | Exa | POST `https://api.exa.ai/search` | `x-api-key` | `type`, `livecrawl` | 取 `results`；`includeContent` 映射为 `contents.text` |
| 11 | Perplexity | POST `https://api.perplexity.ai/search` | `Authorization: Bearer` | 无 | 取 `results` 或类似字段 |
| 12 | Moonshot | POST `{baseUrl}/web-search` | `Authorization: Bearer`（或 OAuth） | `baseUrl`（必需，可来自 `services.moonshotSearch`） | 取响应结果数组；OAuth 后续补齐 |

### 特殊说明

- **DuckDuckGo**：无 API key，但 HTML 解析易失效；需要稳定的 HTML 解析单元测试和回归快照。
- **SearXNG**：必须提供 `options.baseUrl`；无默认值。
- **Moonshot**：需要 `services.moonshotSearch` 或 `options.baseUrl`；OAuth 本次不实现，但接口预留 `token_provider` 字段为 `Option`。
- **Baidu / Serper / Serply / SearchApi / SerpApi**：均需要 API key；在 `from_config` 中检查 `api_key` 非空，否则返回 `WebSearchError::Authentication`。
- **Exa / Perplexity / Tavily**：支持更多 options；解析失败时返回 `WebSearchError::InvalidOptions`。

---

## 实现结构建议 [C:INFERRED]

```
crates/ody-web-search/src/
  lib.rs              # 公开 re-export
  provider.rs         # WebSearchProvider trait
  result.rs           # WebSearchResult / WebSearchOptions
  error.rs            # WebSearchError / classify / retryable
  registry.rs         # WebSearchProviderRegistry / create_default_registry
  fallback.rs         # FallbackWebSearchProvider
  config.rs           # WebSearchConfig / WebSearchProviderConfig / WebSearchProviderName
  providers/
    mod.rs            # 统一 re-export
    http.rs           # build_url / auth_header_for_provider / http_error
    duckduckgo.rs
    bing.rs
    serpapi.rs
    searchapi.rs
    serper.rs
    baidu.rs
    serply.rs
    searxng.rs
    tavily.rs
    exa.rs
    perplexity.rs
    moonshot.rs
```

每个 provider 文件包含：
1. `*Options` struct（强类型或 `serde_json::Map` 解析）。
2. `*Provider` struct + `from_config`。
3. `#[async_trait] impl WebSearchProvider for *Provider`。
4. `#[cfg(test)] mod tests`：mock HTTP client + 响应解析测试 + 错误分类测试。

---

## 批次建议 [C:USER]

| 批次 | Provider | 理由 |
|---|---|---|
| 第一批 | DuckDuckGo、Bing、Serper、Tavily | 实现最简单，覆盖无 key、MS key、JSON POST 三种模式 |
| 第二批 | SerpApi、SearchApi、Baidu、Serply、SearXNG | 中等复杂度，需要更多 options 解析 |
| 第三批 | Exa、Perplexity、Moonshot | 复杂 options / include_content / OAuth 预留 |

每批次独立可测试、可 review；不阻塞其他批次。

---

## 测试策略 [C:INFERRED]

- 每个 provider 使用 mock `reqwest::Client`（通过 `ProviderFactoryDeps.http_client` 注入）测试成功响应。
- 错误路径测试：401/403/429/5xx/timeout/HTML 解析失败。
- `normalize_results` 在 provider 测试之外独立测试，但 provider 测试需覆盖字段映射。
- 真实网络测试：仅作为 `#[cfg(feature = "web-search-live")]` 的集成测试，默认不运行。

---

## 风险与决策

| 决策 | 选择 | 理由 |
|---|---|---|
| 是否所有 provider 一期实现 | 否，分三批 | 降低 review 负担，优先验证架构。 |
| DuckDuckGo HTML 解析失效时如何处理 | 返回 `WebSearchError::Other` 并记录原始 HTML 前 1KB | 便于调试。 |
| 是否统一使用 `reqwest` 而非 `ody_client` | 是 | 避免 `ody-web-search` 依赖 `ody-client` 形成循环依赖。 |
| 是否支持自定义 HTTP headers | 是，通过 `options.customHeaders` | 与 TS `Moonshot` 对齐；其他 provider 可忽略。 |
| 是否支持 `toolCallId` header | 是 | 与 TS `http.ts` 的 `X-Msh-Tool-Call-Id` 对齐；默认发送。 |
