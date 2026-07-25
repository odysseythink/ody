# D1.0 Part 2 — Provider Registry, Fallback, and Provider Contracts

## 1. Provider Registry

### 1.1 `WebSearchProviderFactory`

```rust
pub trait WebSearchProviderFactory: Send + Sync {
    fn name(&self) -> &str;

    fn create(
        &self,
        config: WebSearchProviderConfig,
        http_client: reqwest::Client,
    ) -> Result<SharedWebSearchProvider, WebSearchError>;
}
```

`[C:INFERRED]` 每个 provider 模块实现一个 `WebSearchProviderFactory`，由 registry 统一按名称查找。工厂接收解析后的配置和共享 `reqwest::Client`，返回 `Arc<dyn WebSearchProvider>`。

### 1.2 `WebSearchProviderRegistry`

```rust
pub struct WebSearchProviderRegistry {
    factories: HashMap<String, Box<dyn WebSearchProviderFactory>>,
}

impl WebSearchProviderRegistry {
    pub fn new() -> Self {
        Self { factories: HashMap::new() }
    }

    pub fn register(&mut self, factory: Box<dyn WebSearchProviderFactory>) {
        self.factories.insert(factory.name().to_string(), factory);
    }

    pub fn create(
        &self,
        config: &WebSearchProviderConfig,
        http_client: reqwest::Client,
    ) -> Result<SharedWebSearchProvider, WebSearchError> {
        let name = config.provider.to_string();
        let factory = self.factories
            .get(&name)
            .ok_or_else(|| WebSearchError::Unexpected {
                message: format!("unknown web search provider: {}", name),
            })?;
        factory.create(config.clone(), http_client)
    }

    pub fn create_default_registry() -> Self {
        let mut registry = Self::new();
        registry.register(Box::new(duckduckgo::DuckDuckGoFactory));
        registry.register(Box::new(bing::BingFactory));
        registry.register(Box::new(serpapi::SerpApiFactory));
        registry.register(Box::new(searchapi::SearchApiFactory));
        registry.register(Box::new(serper::SerperFactory));
        registry.register(Box::new(baidu::BaiduFactory));
        registry.register(Box::new(serply::SerplyFactory));
        registry.register(Box::new(searxng::SearXngFactory));
        registry.register(Box::new(tavily::TavilyFactory));
        registry.register(Box::new(exa::ExaFactory));
        registry.register(Box::new(perplexity::PerplexityFactory));
        registry.register(Box::new(moonshot::MoonshotFactory));
        registry
    }
}
```

`[C:UPSTREAM]` 注册表包含 TS 中全部 12 个 provider。默认注册表在 `ody-web-search` crate 启动时构建，app-server 无需关心具体 provider。

## 2. Fallback Provider

### 2.1 `FallbackWebSearchProvider`

```rust
#[derive(Debug)]
pub struct FallbackWebSearchProvider {
    primary: SharedWebSearchProvider,
    secondary: Option<SharedWebSearchProvider>,
}

impl FallbackWebSearchProvider {
    pub fn new(
        primary: SharedWebSearchProvider,
        secondary: Option<SharedWebSearchProvider>,
    ) -> Self {
        Self { primary, secondary }
    }
}

#[async_trait::async_trait]
impl WebSearchProvider for FallbackWebSearchProvider {
    fn name(&self) -> &str {
        "fallback"
    }

    async fn search(
        &self,
        query: &str,
        options: &WebSearchOptions,
    ) -> Result<Vec<WebSearchResult>, WebSearchError> {
        match self.primary.search(query, options).await {
            Ok(results) => Ok(results),
            Err(primary_err) => {
                if let Some(secondary) = &self.secondary {
                    secondary.search(query, options).await
                } else {
                    Err(primary_err)
                }
            }
        }
    }
}
```

`[C:USER]` 严格按用户决策：primary 失败时切换 secondary，不重试、不合并结果、不降级为空结果。

### 2.2 Fallback 边界行为

| 场景 | 行为 |
| --- | --- |
| primary 成功 | 返回 primary 结果，忽略 secondary |
| primary 失败，secondary 成功 | 返回 secondary 结果 |
| primary 失败，secondary 失败 | 返回 secondary 的错误（若存在 secondary）；否则返回 primary 的错误 |
| secondary 未配置 | primary 失败直接向上返回错误 |

## 3. Provider 实现契约

### 3.1 文件布局

```
crates/ody-web-search/src/
├── lib.rs
├── provider.rs        # WebSearchProvider trait, WebSearchResult, WebSearchOptions
├── error.rs           # WebSearchError + classify
├── registry.rs        # WebSearchProviderRegistry
├── fallback.rs        # FallbackWebSearchProvider
├── providers/
│   ├── mod.rs
│   ├── duckduckgo.rs
│   ├── bing.rs
│   ├── serpapi.rs
│   ├── searchapi.rs
│   ├── serper.rs
│   ├── baidu.rs
│   ├── serply.rs
│   ├── searxng.rs
│   ├── tavily.rs
│   ├── exa.rs
│   ├── perplexity.rs
│   └── moonshot.rs
└── providers_tests/   # 每个 provider 的 mock HTTP 测试
```

### 3.2 第一批实现（D1.0 落地时）

`[C:INFERRED]` 第一批优先选择有稳定 JSON API 的 provider，避免 HTML 解析/反爬不确定性影响架构验证。

| Provider | 主要参考 | 认证 | 特殊依赖 |
| --- | --- | --- | --- |
| Bing | `packages/agent-core/src/tools/providers/web-search/bing.ts` | `api_key` / `BING_API_KEY` env | MS Cognitive Services API |
| SerpApi | `packages/agent-core/src/tools/providers/web-search/serpapi.ts` | `api_key` / `SERPAPI_API_KEY` env | serpapi.com |
| SearchApi | `packages/agent-core/src/tools/providers/web-search/searchapi.ts` | `api_key` / `SEARCHAPI_API_KEY` env | searchapi.io |
| Moonshot | `packages/agent-core/src/tools/providers/web-search/moonshot.ts` + `moonshot-web-search.ts` | `api_key` / `MOONSHOT_API_KEY` env | Kimi 搜索 API |

### 3.3 第二批实现

| Provider | 主要参考 | 认证 | 特殊依赖 |
| --- | --- | --- | --- |
| DuckDuckGo | `packages/agent-core/src/tools/providers/web-search/duckduckgo.ts` | 无 | **HTML 解析，标记为 unstable provider** |
| Serper | `packages/agent-core/src/tools/providers/web-search/serper.ts` | `api_key` / `SERPER_API_KEY` env | serper.dev |
| Baidu | `packages/agent-core/src/tools/providers/web-search/baidu.ts` | `api_key` / `BAIDU_API_KEY` env | baidu.com |
| Serply | `packages/agent-core/src/tools/providers/web-search/serply.ts` | `api_key` / `SERPLY_API_KEY` env | serply.io |
| SearXNG | `packages/agent-core/src/tools/providers/web-search/searxng.ts` | 无 / 可选 | self-host，需 `base_url` |
| Tavily | `packages/agent-core/src/tools/providers/web-search/tavily.ts` | `api_key` / `TAVILY_API_KEY` env | tavily.com |
| Exa | `packages/agent-core/src/tools/providers/web-search/exa.ts` | `api_key` / `EXA_API_KEY` env | exa.ai |
| Perplexity | `packages/agent-core/src/tools/providers/web-search/perplexity.ts` | `api_key` / `PERPLEXITY_API_KEY` env | perplexity.ai |

### 3.4 每个 Provider 的实现模式

```rust
// src/providers/duckduckgo.rs
use async_trait::async_trait;

pub struct DuckDuckGoProvider {
    client: reqwest::Client,
    timeout: Duration,
}

impl DuckDuckGoProvider {
    pub fn new(client: reqwest::Client, timeout: Duration) -> Self {
        Self { client, timeout }
    }
}

#[async_trait]
impl WebSearchProvider for DuckDuckGoProvider {
    fn name(&self) -> &str { "duckduckgo" }

    async fn search(
        &self,
        query: &str,
        options: &WebSearchOptions,
    ) -> Result<Vec<WebSearchResult>, WebSearchError> {
        // 1. build request URL + query params
        // 2. execute HTTP request with timeout
        // 3. parse response (HTML or JSON depending on provider)
        // 4. normalize into Vec<WebSearchResult>
        // 5. apply limit
    }
}

pub struct DuckDuckGoFactory;

impl WebSearchProviderFactory for DuckDuckGoFactory {
    fn name(&self) -> &str { "duckduckgo" }

    fn create(
        &self,
        config: WebSearchProviderConfig,
        http_client: reqwest::Client,
    ) -> Result<SharedWebSearchProvider, WebSearchError> {
        let timeout = config.timeout_ms.map(Duration::from_millis)
            .unwrap_or(Duration::from_secs(10));
        Ok(Arc::new(DuckDuckGoProvider::new(http_client, timeout)))
    }
}
```

`[C:INFERRED]` 所有 provider 遵循相同结构：工厂负责从配置创建实例，provider 负责 HTTP 请求与结果规范化。认证信息（api_key）由工厂从配置取出并传给 provider。

### 3.5 `options` 透传规则与校验

`WebSearchProviderConfig.options` 中的键值对透传给对应 provider 的 HTTP 请求或构造参数。例如：

- `searxng`: `options["base_url"]` 必填，用于构造请求 URL。
- `tavily`: `options["search_depth"]` 可选，取值为 `"basic"` / `"advanced"`。
- `exa`: `options["use_autoprompt"]` 可选，取值为 boolean。

`[C:INFERRED]` 每个 provider 工厂在 `create()` 时检查 `options` 白名单：

```rust
fn validate_options(
    config: &WebSearchProviderConfig,
    allowed: &[&str],
) -> Result<(), WebSearchError> {
    for key in config.options.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(WebSearchError::Unexpected {
                message: format!("unknown option '{}' for provider '{}'", key, config.provider),
            });
        }
    }
    Ok(())
}
```

必填项缺失（如 `searxng` 缺少 `base_url`）也在 `create()` 时返回错误，避免运行时才发现配置错误。

### 3.6 API key 读取优先级

`[C:INFERRED]` 为了降低明文 key 泄漏面，API key 按以下优先级读取：

1. `config.api_key`（明文 TOML 配置）。
2. 环境变量：例如 `BING_API_KEY`、`SERPAPI_API_KEY`、`TAVILY_API_KEY` 等（provider 名称大写 + `_API_KEY`）。
3. 若两者都缺失且 provider 需要 key，则 `create()` 返回错误，提示用户通过配置或 env var 提供。

配置打印、日志、诊断输出必须 mask key 值（显示为 `***` 或前 4 位 + `...`）。

## 4. 错误分类

### 4.1 `WebSearchError` 到模型消息的映射

```rust
impl WebSearchError {
    /// Generic user-facing / model-facing message; does not leak internal
    /// classification or provider details.
    pub fn user_message(&self) -> String {
        match self {
            Self::Auth => "Search failed: please check your web search API key.".to_string(),
            _ => "Search temporarily unavailable. Please retry later.".to_string(),
        }
    }

    /// Internal classification prefix for tracing/structured logs only.
    pub fn log_message(&self) -> String {
        match self {
            Self::Network { message } => format!("Search failed (network): {}", message),
            Self::Timeout => "Search timed out.".to_string(),
            Self::Auth => "Search failed (authentication): check your API key.".to_string(),
            Self::RateLimited => "Search failed (rate limited): please retry later.".to_string(),
            Self::Provider { code, message } => format!("Search failed ({}): {}", code, message),
            Self::Unexpected { message } => format!("Search failed: {}", message),
        }
    }
}
```

`[C:INFERRED]` `user_message()` 返回通用文本，避免向模型暴露 provider 内部状态（如限流、认证失败细节）；`log_message()` 保留分类前缀供 tracing/日志使用。

### 4.2 HTTP 错误到 `WebSearchError` 的分类

```rust
impl WebSearchError {
    pub fn from_http_status(status: reqwest::StatusCode, body: &str) -> Self {
        match status.as_u16() {
            401 | 403 => Self::Auth,
            429 => Self::RateLimited,
            408 | 504 => Self::Timeout,
            500..=599 => Self::Provider {
                code: status.to_string(),
                message: body.to_string(),
            },
            _ => Self::Network {
                message: format!("HTTP {}: {}", status, body),
            },
        }
    }
}
```

`[C:INFERRED]` 分类规则是启发式的；各 provider 可在自己的实现中提供更精确的错误映射。

### 4.3 Provider 内部错误映射

```rust
// inside a provider implementation
let response = self.client.get(&url).send().await
    .map_err(|e| WebSearchError::from_reqwest(e))?;

if !response.status().is_success() {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    return Err(WebSearchError::from_http_status(status, &body));
}
```

`[C:INFERRED]` `from_reqwest` 处理连接超时、DNS 失败、TLS 错误等网络层异常，统一映射为 `WebSearchError::Network` 或 `WebSearchError::Timeout`（当 `is_timeout()` 为 true 时）。

## 5. Error Handling / Degradation（Provider 侧）

| 场景 | 行为 | 模型可见 |
| --- | --- | --- |
| 网络超时 | `WebSearchError::Timeout` | `Search timed out.` |
| 401/403 | `WebSearchError::Auth` | `Search failed (authentication): ...` |
| 429 | `WebSearchError::RateLimited` | `Search failed (rate limited): ...` |
| 5xx | `WebSearchError::Provider` | `Search failed (HTTP 503): ...` |
| primary 失败且 secondary 成功 | fallback 返回 secondary 结果 | 正常结果 |
| primary 失败且 secondary 失败 | 返回 secondary 错误 | 错误文本 |
| 空结果 | 返回空 `Vec`（不视为错误） | `No search results found.`（由 WebSearchTool 格式化） |

`[C:USER]` 不降级为空结果：所有网络/认证/限流错误都向上抛给 `WebSearchTool`，由其包装为 `FunctionCallError::Fatal(user_message)`。
