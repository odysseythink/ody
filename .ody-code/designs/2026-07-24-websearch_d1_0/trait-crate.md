# Part 1 — `ody-web-search` crate 边界与核心类型

## 来源 [C:UPSTREAM]

- TS: `packages/agent-core/src/tools/builtin/web/web-search.ts:21-36` 定义 `WebSearchResult` / `WebSearchProvider`。
- TS: `packages/agent-core/src/tools/providers/web-search/types.ts:1-17` 导出并扩展 `normalizeResult`。
- Rust 现状：`ext/web-search/src/tool.rs:36-40` 的 `WebSearchTool` 结构体依赖 `SharedModelProvider` + `SearchSettings`；本次完全替换，不保留旧类型。

## Crate 边界 [C:USER]

新建 crate `crates/ody-web-search`（workspace 中注册为 `ody-web-search`）。

### 依赖方向

```
ody-web-search
  ├── ody-protocol          (error types, config primitives, if needed)
  ├── serde + serde_json
  ├── schemars              (JSON schema for config types)
  ├── reqwest               (HTTP client; provider 实现使用)
  ├── thiserror             (WebSearchError)
  ├── tracing               (structured logging)
  └── async-trait           (trait async method)
```

`ody-web-search` **不依赖** `ody-core`、`ody-app-server`、`ody-model-provider`、`ody-api`。任何需要与这些 crate 交互的逻辑（例如注入工具、HTTP transport 选择）都由调用方负责。这保证：
- provider 实现可独立测试；
- `core` 与 `app-server` 对 `ody-web-search` 的依赖是单向的。

### 公开 API 清单

| 符号 | 类型 | 可见性 | 用途 |
|---|---|---|---|
| `WebSearchResult` | struct | `pub` | 单条搜索结果 |
| `WebSearchOptions` | struct | `pub` | 搜索调用参数 |
| `WebSearchProvider` | trait | `pub` | provider 抽象 |
| `WebSearchProviderRegistry` | struct | `pub` | 工厂注册表 |
| `FallbackWebSearchProvider` | struct | `pub` | primary + secondary 包装 |
| `WebSearchError` | enum | `pub` | 错误分类 |
| `classify_search_error` | fn | `pub` | 工具层错误前缀 |
| `is_retryable_search_error` | fn | `pub` | fallback 决策 |
| `WebSearchConfig` | struct | `pub` | `services.webSearch` 配置 |
| `WebSearchProviderConfig` | struct | `pub` | 单个 provider 配置 |
| `WebSearchProviderName` | enum / const set | `pub` | 支持的 provider 名集合 |
| `create_default_web_search_registry` | fn | `pub` | 预置 12 个 provider 工厂 |
| `normalize_result` / `normalize_results` | fn | `pub` | 原始响应规范化 |

---

## Data Models

### `WebSearchResult` [C:UPSTREAM]

对齐 TS `WebSearchResult`。

```rust
use serde::Serialize;

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct WebSearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub date: Option<String>,
    pub content: Option<String>,

    /// 原始响应，用于调试或 provider 特殊输出。
    /// 在格式化输出中不序列化给模型。
    #[serde(skip)]
    pub raw: Option<serde_json::Value>,
}
```

约束：
- `title` 截断至 500 字符（与 TS `normalizeResult` 一致）。
- `url` 截断至 2048 字符。
- `snippet` 截断至 4000 字符。
- `normalize_results` 过滤掉 `title` 或 `url` 为空的条目。

### `WebSearchOptions` [C:UPSTREAM]

对齐 TS `provider.search(query, options)` 的第二个参数。

```rust
#[derive(Debug, Clone, Default)]
pub struct WebSearchOptions {
    pub limit: Option<u32>,
    pub include_content: Option<bool>,
    pub tool_call_id: Option<String>,
}
```

- `limit` 有效范围 1..=20；provider 实现负责 clamp 或返回 `WebSearchError::InvalidOptions`。
- `include_content` 为 true 时，支持 fetch content 的 provider 应返回 `content`。
- `tool_call_id` 仅用于日志关联，不参与请求。

### `WebSearchProvider` trait [C:USER]

```rust
use async_trait::async_trait;
use std::fmt::Debug;

#[async_trait]
pub trait WebSearchProvider: Send + Sync + Debug {
    fn name(&self) -> &str;

    async fn search(
        &self,
        query: &str,
        options: WebSearchOptions,
    ) -> Result<Vec<WebSearchResult>, WebSearchError>;
}
```

设计要点：
- `Send + Sync`：provider 在 `app-server` 与 `core` 的异步任务中共享。
- `Debug`：便于日志和测试。
- 返回类型显式为 `Result<Vec<WebSearchResult>, WebSearchError>`，而非 `anyhow::Result`，保证错误分类可预测。
- `query` 为 `&str`：provider 内部负责 URL 编码。

### `WebSearchError` [C:UPSTREAM]

对齐 TS 错误分类（`fallback.ts:48-74`）与 `web-search.ts:116-120` 的错误前缀输出。

```rust
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq)]
pub enum WebSearchError {
    #[error("Search cancelled: {0}")]
    Cancelled(String),

    #[error("Search timed out: {0}")]
    Timeout(String),

    #[error("Search failed (authentication): {0}")]
    Authentication(String),

    #[error("Search failed (rate limit): {0}")]
    RateLimit(String),

    #[error("Search failed (network): {0}")]
    Network(String),

    #[error("Search failed (server): {0}")]
    Server(String),

    #[error("Search failed: {0}")]
    Other(String),

    #[error("Invalid web search options: {0}")]
    InvalidOptions(String),

    #[error("Unknown web search provider: {0}")]
    UnknownProvider(String),
}
```

- `Display` 输出直接可作为工具错误文本返回给模型（与 TS 的 `classifySearchError` 一致）。
- `PartialEq` 便于测试断言。
- `Clone` 便于 fallback 二次抛出和日志记录。

### 错误分类函数 [C:UPSTREAM]

```rust
pub fn classify_search_error<E: std::fmt::Display>(error: E) -> WebSearchError {
    let message = error.to_string();
    let lower = message.to_lowercase();
    let name = std::any::type_name_of_val(&error); // 仅用于 rust 内部类型判断

    if lower.contains("abort") || name.ends_with("AbortError") {
        return WebSearchError::Cancelled(message);
    }
    if lower.contains("timed out") || lower.contains("timeout") || name.ends_with("TimeoutError") {
        return WebSearchError::Timeout(message);
    }
    if lower.contains("401") || lower.contains("403") || lower.contains("unauthorized") || lower.contains("authentication") || lower.contains("auth") {
        return WebSearchError::Authentication(message);
    }
    if lower.contains("429") {
        return WebSearchError::RateLimit(message);
    }
    if lower.contains("5") && lower.contains("http ") || lower.contains("server") {
        return WebSearchError::Server(message);
    }
    if lower.contains("network") || lower.contains("fetch") || lower.contains("dns") || lower.contains("connection") {
        return WebSearchError::Network(message);
    }
    WebSearchError::Other(message)
}

pub fn is_retryable_search_error(error: &WebSearchError) -> bool {
    match error {
        WebSearchError::Cancelled(_) => false,
        WebSearchError::Timeout(_) => true,
        WebSearchError::Authentication(_) => false,
        WebSearchError::RateLimit(_) => true,
        WebSearchError::Network(_) => true,
        WebSearchError::Server(_) => true,
        WebSearchError::Other(_) => false,
        WebSearchError::InvalidOptions(_) => false,
        WebSearchError::UnknownProvider(_) => false,
    }
}
```

注意：
- Rust 没有 TS 的 `AbortError` / `TimeoutError` 全局类；约定通过 `error.to_string()` 中的子串或 `tokio::time::error::Elapsed` 等类型名称判断。
- 在 `reqwest` 场景下，timeout 会被包装成 `reqwest::Error`，其 `is_timeout()` 方法应在 provider 实现层优先使用，再 fallback 到字符串分类。

---

## 复用分析 [C:INFERRED]

- `ody-api::SearchClient` 与 `ody_protocol::items::WebSearchItem` 本次不复用，目标是删除。
- `reqwest` 已是 workspace 依赖；`ody_client::default_client::build_reqwest_client` 可被 provider 实现参考用于统一 TLS/timeout 配置，但 `ody-web-search` 不直接依赖 `ody-client` 以避免循环依赖。
- `ody_protocol::config_types` 中的 `WebSearchMode` / `WebSearchContextSize` 本次删除，不复用。

---

## 接口契约

### 输入不变式
- `query` 非空；`WebSearchTool` 在调用前校验。
- `limit` 范围 1..=20；provider 实现可安全 clamp 到 20，但不允许为 0。

### 输出不变式
- `search` 返回空 `Vec` 表示无结果，不是错误。
- `title` 和 `url` 非空（经 `normalize_results` 过滤）。
- `url` 应为绝对 URL；provider 实现负责拼接 base URL。

### 并发
- trait 方法 `&self` 不可变引用，允许多线程并发调用同一 provider 实例。
- 若 provider 需要可变状态（如 token 桶），应内部使用 `Mutex` / `Atomic`。

---

## 风险与决策

| 决策 | 选择 | 理由 |
|---|---|---|
| 是否用 `anyhow` 替代 `WebSearchError` | 否 | 需要稳定错误分类用于 fallback 和工具输出。 |
| 是否把 `reqwest::Client` 放进 trait 方法 | 否 | 每个 provider 内部持有 client；trait 保持最小。 |
| 是否支持 `raw` 字段给模型 | 否 | `raw` 仅用于调试和 parity 测试，不输出给模型。 |
| `include_content` 语义 | 与 TS 一致 | provider 若支持，返回 `content`；否则 `content` 为 `None`。 |
