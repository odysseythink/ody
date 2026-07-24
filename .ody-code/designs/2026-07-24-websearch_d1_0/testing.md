# Part 7 — 测试策略与 Parity 验收

## 来源 [C:UPSTREAM]

- TS: `packages/agent-core/src/tools/builtin/web/web-search.ts` — 工具输入 schema、输出格式、错误处理（参考 parity 预期）。
- TS: `packages/agent-core/src/tools/providers/web-search/` — 各 provider 响应解析逻辑与字段映射（参考 parity 预期）。
- TS: `apps/ody-code/src/tui/components/messages/tool-renderers/chip.ts:103-111` — chip 计数规则（参考 parity 预期）。
- Rust 现状：`ext/web-search/src/tool.rs` 旧测试、`core/tests/suite/web_search.rs` 旧集成测试将删除；`ody-config` 已有 `config_toml` 解析测试可作为模板。

## 测试目标 [C:USER]

1. **正确性**：每个 provider 的 HTTP 请求构造、响应解析、字段映射、`normalize_results` 截断与过滤均正确。
2. **错误处理**：`classify_search_error` / `is_retryable_search_error` 与 `FallbackWebSearchProvider` 的 retryable 判定正确。
3. **配置解析**：`services.webSearch` 反序列化、校验、旧 `web_search` 迁移 warning 正确。
4. **工具执行**：`WebSearchTool` 的 schema、输入校验、输出格式化、错误转换正确。
5. **TUI 渲染**：`WebSearch` chip 按输出内容正确计数。
6. **TS-Rust parity**：关键运行时输出与 TS `ody-code` 实现一致，可接受差异必须在 `known-gaps.md` 中记录。
7. **回归防护**：删除旧代码后，原 `web_search_mode` / `hosted web search` / `SearchClient` 路径不再被误触发。

---

## 测试层次 [C:USER]

```
┌─────────────────────────────────────────────────────────────┐
│  L3 — Parity / E2E 测试（可选，非默认）                    │
│  - 与 TS 运行时输出逐项对比                                 │
│  - 真实网络集成测试（feature-gated）                        │
└─────────────────────────────────────────────────────────────┘
┌─────────────────────────────────────────────────────────────┐
│  L2 — 集成测试                                              │
│  - app-server 工具注册与配置变更                            │
│  - 端到端工具调用（mock provider）                          │
└─────────────────────────────────────────────────────────────┘
┌─────────────────────────────────────────────────────────────┐
│  L1 — 单元测试（默认运行）                                  │
│  - ody-web-search: provider / registry / fallback / error  │
│  - ody-config: services.webSearch 解析                        │
│  - ext/web-search: WebSearchTool execute                     │
│  - ody-tui: chip 渲染                                        │
└─────────────────────────────────────────────────────────────┘
```

- **L1 必须全绿**才能合并任何 D1.0 PR。
- **L2 在删除旧代码后必须全绿**，否则不能进入后续阶段。
- **L3 作为校验参考**，不要求每次 CI 运行；在 provider 分批实现时作为批次验收条件。

---

## L1 单元测试 — `ody-web-search` [C:USER]

### 1. Provider 单元测试

每个 provider 文件包含 `#[cfg(test)] mod tests`，使用 mock HTTP client 注入。

#### Mock HTTP client 设计 [C:INFERRED]

`ProviderFactoryDeps` 中的 `http_client: Option<reqwest::Client>` 允许测试注入一个指向 mock server 的 `reqwest::Client`。

推荐方案：使用 `wiremock`（或 `mockito`）在测试中启动本地 HTTP server，并用 `reqwest::Client::builder().base_url(...)` 或普通 absolute URL 指向它。由于 `reqwest` 不支持 `base_url`，测试中由 provider 构造 absolute URL；mock server 监听 `127.0.0.1:0`，测试将 URL 通过 `options.baseUrl` 或 `provider` 的默认 endpoint 指向该端口。

```rust
#[tokio::test]
async fn bing_parses_web_pages() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/v7.0/search"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({
            "webPages": {
                "value": [
                    { "name": "Rust", "url": "https://www.rust-lang.org", "snippet": "A language" }
                ]
            }
        })))
        .mount(&server)
        .await;

    let client = reqwest::Client::new();
    let deps = ProviderFactoryDeps {
        http_client: Some(client),
        moonshot_service_config: None,
    };
    let config = WebSearchProviderConfig {
        provider: WebSearchProviderName::Bing,
        api_key: Some("test-key".to_string()),
        timeout_ms: Some(5000),
        options: None,
    };

    let provider = create_default_web_search_registry()
        .create(&config, &deps)
        .unwrap();

    // 将 bing 的 endpoint 替换为 mock server：通过 options 覆盖 baseUrl
    // 注意：实际实现时 Bing provider 应支持 options.baseUrl 以方便测试。
    let options = WebSearchOptions { limit: Some(1), ..Default::default() };
    let results = provider.search("rust", options).await.unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].title, "Rust");
    assert_eq!(results[0].url, "https://www.rust-lang.org");
}
```

为适应 mock 测试，所有 provider 实现应支持 `options.baseUrl` 覆盖默认 endpoint，或测试时使用仅解析响应的辅助函数（不发送真实请求）。

#### 每个 provider 的最小测试集

| 测试 | 覆盖 |
|---|---|
| `*_parses_success_response` | 成功响应 → 正确字段映射与结果数量 |
| `*_empty_response_yields_zero_results` | 空响应 → 返回空 `Vec`，不是错误 |
| `*_authentication_error` | 401/403 → `WebSearchError::Authentication` |
| `*_rate_limit_error` | 429 → `WebSearchError::RateLimit` |
| `*_server_error` | 5xx → `WebSearchError::Server` |
| `*_network_timeout` | 模拟 timeout → `WebSearchError::Timeout` |
| `*_invalid_json` | 返回非 JSON 或缺字段 → `WebSearchError::Other` |
| `*_respects_limit` | `limit = 1` 时仅返回 1 条，多余结果被截断 |
| `*_include_content_when_supported` | `include_content = true` 时存在 `content`（仅 Exa 等） |
| `*_options_validation` | 无效 `options` → `WebSearchError::InvalidOptions` |
| `*_from_config_requires_api_key` | 需要 key 的 provider 在 `api_key` 缺失时返回 `Authentication` |

- DuckDuckGo 额外测试：HTML 解析成功、HTML 结构变化降级、uddg 重定向 URL 提取。
- SearXNG 额外测试：`baseUrl` 必需、HTML 与 JSON 输出模式。
- Moonshot 额外测试：`baseUrl` 来源（`services.moonshotSearch` vs `options.baseUrl`），OAuth 缺失时返回 `Authentication`（本次 OAuth 不实现，留占位测试）。

### 2. `normalize_results` 测试 [C:UPSTREAM]

```rust
#[test]
fn normalize_results_filters_and_truncates() {
    let raw = vec![
        WebSearchResult { title: "a".repeat(600), url: "https://x.com", snippet: "b".repeat(5000), ..Default::default() },
        WebSearchResult { title: "", url: "https://empty-title.com", snippet: "x", ..Default::default() },
    ];
    let normalized = normalize_results(raw, 1);

    assert_eq!(normalized.len(), 1);
    assert_eq!(normalized[0].title.len(), 500);
    assert_eq!(normalized[0].url.len(), 2048);
    assert_eq!(normalized[0].snippet.len(), 4000);
}
```

覆盖：字段别名映射、截断、空字段过滤、limit slice。

### 3. 注册表测试 [C:UPSTREAM]

- `create_default_web_search_registry` 包含全部 12 个 provider。
- `registry.create` 返回的 provider `name()` 与配置一致。
- 未知 provider 返回 `WebSearchError::UnknownProvider`。
- `registry.list()` / `names()` 返回稳定顺序集合（便于测试与 CLI 展示）。

### 4. Fallback 测试 [C:UPSTREAM]

| 测试 | 预期行为 |
|---|---|
| `fallback_primary_ok_no_secondary_call` | primary 成功，不调用 secondary |
| `fallback_primary_non_retryable_secondary_not_called` | primary `Authentication` 错误，不调用 secondary，返回 primary 错误 |
| `fallback_primary_retryable_secondary_ok` | primary `Timeout` 错误，调用 secondary 成功，返回 secondary 结果 |
| `fallback_primary_retryable_secondary_fail` | primary `Timeout` 错误，调用 secondary `Other` 错误，返回 secondary 错误 |
| `fallback_no_secondary` | 未配置 secondary 且 primary 失败时，直接返回 primary 错误 |
| `fallback_options_passed_unchanged` | 传递给 fallback 的 `options` 原样传给 primary 和 secondary |

使用 `tokio::test` + `Arc<dyn WebSearchProvider>` 的 mock 实现（实现 `WebSearchProvider` trait 的测试 stub）完成这些测试。

### 5. 错误分类测试 [C:UPSTREAM]

```rust
#[test]
fn classify_search_error_maps_status_codes() {
    assert!(matches!(classify_search_error("HTTP 401"), WebSearchError::Authentication(_)));
    assert!(matches!(classify_search_error("HTTP 429"), WebSearchError::RateLimit(_)));
    assert!(matches!(classify_search_error("HTTP 503"), WebSearchError::Server(_)));
    assert!(matches!(classify_search_error("dns error"), WebSearchError::Network(_)));
    assert!(matches!(classify_search_error("timeout"), WebSearchError::Timeout(_)));
    assert!(matches!(classify_search_error("abort"), WebSearchError::Cancelled(_)));
    assert!(matches!(classify_search_error("something else"), WebSearchError::Other(_)));
}

#[test]
fn is_retryable_search_error_matches_expected() {
    assert!(!is_retryable_search_error(&WebSearchError::Authentication("x".into())));
    assert!(is_retryable_search_error(&WebSearchError::Timeout("x".into())));
    assert!(is_retryable_search_error(&WebSearchError::RateLimit("x".into())));
    assert!(is_retryable_search_error(&WebSearchError::Network("x".into())));
    assert!(is_retryable_search_error(&WebSearchError::Server("x".into())));
}
```

---

## L1 单元测试 — `ody-config` [C:USER]

在 `config/src/config_toml_tests.rs` 或新增 `config/src/services_tests.rs` 中：

| 测试 | 输入 | 预期 |
|---|---|---|
| `services_web_search_parses_primary` | `[services.webSearch]\nprimary = { provider = "duckduckgo" }` | `services.web_search` 为 `Some`，`primary.provider = Duckduckgo` |
| `services_web_search_parses_secondary` | `[services.webSearch]\nsecondary = { provider = "bing", apiKey = "x" }` | `secondary` 解析正确 |
| `services_web_search_rejects_unknown_provider` | `provider = "google"` | 反序列化错误 |
| `services_web_search_rejects_timeout_out_of_range` | `timeoutMs = 500` | 解析错误（范围 1000..=120000） |
| `services_web_search_rejects_unknown_field` | `primary = { provider = "bing", unknown = 1 }` | 反序列化错误（`deny_unknown_fields`） |
| `deprecated_web_search_emits_warning` | `web_search = "hosted"` | `startup_warnings` 包含指定 warning |
| `services_web_search_case_insensitive` | `provider = "Bing"` | 解析为 `Bing` |
| `services_moonshot_search_passthrough` | `[services.moonshotSearch]\nbaseUrl = "..."` | 解析为 `serde_json::Value` 且不报错 |

---

## L1 单元测试 — `ext/web-search` [C:USER]

在 `ext/web-search/src/tool.rs` 的 `#[cfg(test)]` 中或 `ext/web-search/tests/` 中：

| 测试 | 覆盖 |
|---|---|
| `web_search_tool_spec_has_query_limit_include_content` | schema 包含 3 个参数，`query` required |
| `web_search_tool_rejects_empty_query` | `query = ""` → `FunctionCallError::RespondToModel` |
| `web_search_tool_rejects_missing_query` | 缺少 `query` → `FunctionCallError::RespondToModel` |
| `web_search_tool_formats_single_result` | 单结果 → 包含 `Title`/`Date`/`URL`/`Snippet` |
| `web_search_tool_formats_multiple_results` | 多结果 → 用 `---\n` 分隔 |
| `web_search_tool_formats_content` | `content` 存在 → 追加 `Content:` 段落 |
| `web_search_tool_no_results` | 空结果 → `No search results found.` |
| `web_search_tool_error_timeout` | provider 返回 `Timeout` → `FunctionCallError::Fatal` 且文本前缀为 `Search timed out:` |
| `web_search_tool_error_auth` | provider 返回 `Authentication` → `Search failed (authentication):` |
| `web_search_tool_options_passed` | `limit` 与 `include_content` 正确映射为 `WebSearchOptions` |

### Mock provider in tool tests

```rust
#[derive(Debug)]
struct StubProvider {
    results: Vec<WebSearchResult>,
    error: Option<WebSearchError>,
}

#[async_trait]
impl WebSearchProvider for StubProvider {
    fn name(&self) -> &str { "stub" }

    async fn search(&self, _query: &str, _options: WebSearchOptions) -> Result<Vec<WebSearchResult>, WebSearchError> {
        match &self.error {
            Some(e) => Err(e.clone()),
            None => Ok(self.results.clone()),
        }
    }
}
```

---

## L1 单元测试 — `ody-tui` chip 渲染 [C:USER]

在 `tui/src/thread_transcript.rs` 的 `#[cfg(test)]` 中或 `tui/src/thread_transcript_tests.rs` 中：

| 测试 | 输出文本 | 期望 chip |
|---|---|---|
| `web_search_chip_no_results_empty` | `"No search results found."` | `no results` |
| `web_search_chip_no_results_blank` | `""` | `no results` |
| `web_search_chip_single_result` | `"1. Title\nDate\nURL\nSnippet"` | `1 result` |
| `web_search_chip_multiple_results` | `"1. ...\n---\n2. ..."` | `2 results` |
| `web_search_chip_non_list` | `"Some plain text"` | `web result` |
| `web_search_chip_bullet_list` | `"- Item A\n- Item B"` | `2 results` |

---

## L2 集成测试 [C:USER]

### 1. app-server 工具注册

在 `app-server/tests/` 或 `app-server/src/extensions_tests.rs` 中，使用 `ExtensionRegistryBuilder` + `Config` 构造线程 store，并断言 `ToolContributor::tools` 返回的集合是否包含 `WebSearch`。

| 测试 | 配置 | 期望 |
|---|---|---|
| `web_search_tool_absent_without_config` | `services.web_search = None` | 工具列表无 `WebSearch` |
| `web_search_tool_present_with_config` | `primary = duckduckgo` | 工具列表包含 `WebSearch` |
| `web_search_tool_absent_on_invalid_config` | `primary = { provider = "unknown" }` | 工具列表无 `WebSearch`，记录 warning |
| `web_search_tool_changes_on_config_change` | 先有效再无效 | `on_config_changed` 后工具消失 |

### 2. 端到端工具调用（mock provider）

通过 `ExtensionRegistry` 注册 `WebSearchTool` 与一个 mock provider，调用 `ToolExecutor::handle`，断言输出文本。

---

## L3 Parity 测试 — TS-Rust 对齐验收 [C:USER]

### 目标

验证 Rust 实现的**运行时输出**在结构和关键文本上与 TS `ody-code` 一致。不需要字节级一致，但以下差异必须可解释并记录在 `known-gaps.md`：

1. 输出格式顺序：`Title\nDate\nURL\nSnippet\nContent`（与 TS 一致）。
2. 分隔符：`---\n`（与 TS 一致）。
3. 无结果文本：`No search results found.`（与 TS 一致）。
4. 错误前缀：`Search timed out:` / `Search failed (authentication):` 等（与 TS 一致）。
5. 工具 schema：参数名、类型、描述、required 列表（与 TS 一致）。
6. Chip 计数规则：数字 / `-` / `*` 开头行计数（与 TS 一致）。
7. Provider 结果字段映射：title/url/snippet/date/content 等价（与 TS 一致）。
8. Provider 请求参数：如 Bing 的 `Ocp-Apim-Subscription-Key`、Tavily 的 `api_key` 位置等（与 TS 一致）。

### 方法 [C:INFERRED]

不直接运行 TS 测试套件（因为 TS 端无对应测试文件），而是采用**双轨样本对比**：

1. 在 TS 仓库中运行一个最小脚本，对每个 provider 使用 mock HTTP server 或录制响应，输出：
   - provider 构造的 HTTP 请求（URL、header、body 关键字段）。
   - 返回的 `WebSearchResult` JSON 数组。
   - 工具输出文本。
2. 在 Rust 仓库中运行相同 mock 数据，输出相同三项。
3. 使用一个 parity 脚本做结构对比，允许白名单差异（如 header 顺序、JSON 字段顺序）。

由于无法保证 TS 环境可随时运行，推荐方案：
- 在 `.ody-code/spikes/web_search_parity/` 中编写一次性对比脚本，作为 D1.0 验收 spike；
- 如果无 TS 环境，则基于源码人工核对关键输出路径，并在 `known-gaps.md` 记录为 `[C:INFERRED]` 假设；
- 批次验收时，第一批 DuckDuckGo/Bing/Serper/Tavily 必须完成 parity 对比；后续批次每个 provider 至少使用 fixture 做一次 parity 对比，并记录到 `known-gaps.md`。

### 记录位置

所有 parity 差异和结论记录在 `packages/integration-tests/src/parity/known-gaps.md`（或其继任位置）新增 WebSearch 章节，包含：

```markdown
## WebSearch D1.0 Parity

- [x] Output format: Title/Date/URL/Snippet/Content order matches TS.
- [x] Separator `---\n` matches TS.
- [x] Empty result text matches TS.
- [ ] DuckDuckGo HTML parser may differ on edge cases; tracked in #<issue>.
```

---

## 测试数据与快照 [C:INFERRED]

### 响应 fixture

在 `ody-web-search/src/providers/fixtures/` 下为每个 provider 保存：

- `*_success.json` / `*_success.html`：成功响应。
- `*_empty.json` / `*_empty.html`：无结果响应。
- `*_error_401.json` / `*_error_429.json` / `*_error_5xx.json`：错误响应。
- `README.md`：每个 fixture 的来源（mock 或录制）、录制时间、是否脱敏。

HTML 解析型 provider（DuckDuckGo、SearXNG）保存真实 HTML 片段或录制样本，并用 **insta** 或纯 assert 比对解析结果。

### 输出快照

`WebSearchTool` 输出格式化、`normalize_results` 结果使用 `insta` 快照测试，便于后续 TS-Rust parity 对比。快照文件存于 `ext/web-search/tests/snapshots/` 和 `ody-web-search/src/providers/snapshots/`。

### 旧配置 fixture

在 `ody-config` 测试中保留 `web_search = "hosted"` 的 fixture，验证 deprecation warning。

### 录制响应与 parity

- 为每个 provider 录制至少一次真实响应（使用 `web-search-live` feature），保存为 fixture；录制时确保 API key 脱敏。
- parity 测试使用 fixture 而非每次连接真实 API；fixture 作为 TS 与 Rust 输出对比的基准输入。
- 每月或在 provider 代码改动时重新录制并审阅 fixture 差异，以捕获 API 漂移。

---

## 测试文件组织 [C:INFERRED]

```
ody-web-search/
  src/
    providers/
      mod.rs
      duckduckgo.rs              # 含 #[cfg(test)] mod tests
      bing.rs                    # 含 #[cfg(test)] mod tests
      ...（每个 provider 同结构）
      http.rs                    # 含 build_url / auth_header_for_provider 测试
    normalize.rs                 # 或 result.rs 中 normalize 函数 + 测试
    registry.rs                  # 含注册表测试
    fallback.rs                  # 含 fallback 测试
    error.rs                     # 含 classify / retryable 测试
    config.rs                    # 含 config 解析测试
  tests/                         # 可选：集成测试（真实网络 feature-gated）
    live_integration.rs          # #[cfg(feature = "web-search-live")]

ext/web-search/
  src/tool.rs                    # 含 #[cfg(test)] mod tests
  tests/
    e2e_tool.rs                  # 端到端工具调用集成测试

config/
  src/services_tests.rs          # 或 config_toml_tests.rs 新增 module

app-server/
  src/extensions_tests.rs        # 或 tests/extensions.rs

tui/
  src/thread_transcript.rs       # 含 #[cfg(test)] mod tests
```

---

## 测试命令与 CI [C:USER]

### 本地迭代命令

```bash
# 仅跑受影响的 crate
cargo nextest run -p ody-web-search
cargo nextest run -p ody-config
cargo nextest run -p ody-web-search-extension
cargo nextest run -p ody-tui

# 跑集成测试
cargo nextest run -p ody-app-server --test web_search_extension

# 检查编译
cargo check -p ody-web-search -p ody-config -p ody-web-search-extension -p ody-tui -p ody-app-server
```

### 旧代码删除后的验证命令

```bash
cargo check -p ody-app-server -p ody-core -p ody-api -p ody-config -p ody-features -p ody-cli -p ody-model-provider-info -p ody-models-manager -p ody-protocol
cargo nextest run -p ody-core -p ody-app-server -p ody-config -p ody-web-search -p ody-tui
```

### CI 要求

- 所有 D1.0 PR 必须通过 `cargo nextest run -p ody-web-search -p ody-config -p ody-web-search-extension -p ody-tui`。
- 删除旧代码的 PR 必须额外通过全 workspace `cargo check` 和上述 `ody-core` / `ody-app-server` / `ody-config` 的 nextest。
- `web-search-live` feature 默认不运行，只在 nightly/weekly CI 中可选运行，并需外部 API key secret；PR 中禁止运行。

---

## 测试批次与 D1.0 验收条件 [C:USER]

| 批次 | Provider | 验收条件 |
|---|---|---|
| 第一批 | DuckDuckGo、Bing、Serper、Tavily | 全部 L1 通过；每个 provider 完成与 TS 的 L3 parity 对比（至少 1 个 fixture） |
| 第二批 | SerpApi、SearchApi、Baidu、Serply、SearXNG | 全部 L1 通过；每个 provider 完成 L3 parity 对比（至少 1 个 fixture） |
| 第三批 | Exa、Perplexity、Moonshot | 全部 L1 通过；每个 provider 完成 L3 parity 对比；Moonshot 的 OAuth 占位测试通过 |

### D1.0 整体关闭条件

1. R1.1–R1.5 删除完成且全 workspace `cargo check` 通过。
2. I2.x 新建 `ody-web-search` / `ext/web-search` / `ody-config` 改动完成并通过 L1 + L2。
3. `known-gaps.md` WebSearch 章节已记录所有可接受差异和未关闭项。
4. 审计 gate 中的 `Assumption` 第 2 条（旧 thread history 兼容）已验证，或兼容降级层已合入。

---

## 复用分析 [C:INFERRED]

- 复用 `cargo nextest` 配置（`.config/nextest.toml`）和 insta 快照框架（若仓库已使用）。
- `ody-config` 中 `config_toml_tests.rs` 的测试 fixture 结构可作为 `services.webSearch` 解析测试模板。
- 不复用旧的 `ext/web-search` 测试、`core/tests/suite/web_search.rs` 和旧 `web_search_mode` 相关测试；它们将随旧代码一起删除。
- 在 app-server 集成测试中复用现有的 `ExtensionRegistryBuilder` 测试 helper（若存在）。

---

## 风险与决策 [C:INFERRED]

| 决策 | 选择 | 理由 |
|---|---|---|
| 是否要求真实网络测试 | 否，mock 为主；真实网络仅 feature-gated | 避免 CI 依赖外部 API key 和网络稳定性。 |
| 是否使用 `wiremock` / `mockito` | `wiremock`（推荐） | 若仓库未引入 `mockito`，`wiremock` 更成熟且支持 async；若已有 `mockito` 则优先复用。 |
| Provider 是否支持 `options.baseUrl` 测试覆盖 | 是 | 否则 mock HTTP 测试需要修改全局 URL 或做复杂解析。 |
| 是否每个 provider 测试都必须与 TS 做 L3 parity | 否，抽样 | 时间和环境受限；但第一批至少 1 个、后续每批至少 1 个。 |
| `insta` 快照是否必须 | 否，但推荐 | 便于格式化输出回归和 parity 对比。 |
| 是否删除旧测试后重写为新测试 | 是 | 旧测试与旧实现语义绑定，无法迁移。 |

---

## 测试要点索引（跨 parts 汇总） [C:INFERRED]

| Part | 测试文件 | 关键用例 |
|---|---|---|
| trait-crate | `ody-web-search/src/error.rs`、`result.rs` | `classify_search_error`、`normalize_results` |
| registry-fallback | `ody-web-search/src/registry.rs`、`fallback.rs` | 注册表、fallback、配置解析 |
| providers | `ody-web-search/src/providers/*.rs` | 12 个 provider 响应解析与错误路径 |
| config-injection | `config/src/services_tests.rs`、`app-server/src/extensions_tests.rs` | 配置解析、工具注入 |
| tool-tui | `ext/web-search/src/tool.rs`、`tui/src/thread_transcript.rs` | 工具执行、输出格式化、chip 渲染 |
| deletion-migration | `config/src/services_tests.rs` | 旧 `web_search` warning、旧代码无残留 grep |
