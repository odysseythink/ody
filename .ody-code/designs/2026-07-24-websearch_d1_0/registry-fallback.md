# Part 2 — 注册表、Fallback 与配置 Schema

## 来源 [C:UPSTREAM]

- TS: `packages/agent-core/src/tools/providers/web-search/registry.ts` — 工厂注册表、12 个 provider 初始化。
- TS: `packages/agent-core/src/tools/providers/web-search/runtime.ts` — `resolveWebSearchRuntime` 选择 primary/secondary 并包装 `FallbackWebSearchProvider`。
- TS: `packages/agent-core/src/tools/providers/web-search/fallback.ts` — fallback 逻辑与 `isRetryableError`。
- TS: `packages/agent-core-shared/src/config.ts:150-238` — `WebSearchProviderNameSchema`、`WebSearchProviderConfigSchema`、`WebSearchConfigSchema`、`ServicesConfigSchema`。
- Rust 现状：`config/src/config_toml.rs` 与 `config/src/mod.rs` 中的 `web_search_mode` 解析将删除；`config/src/services.rs` 新文件承担 `services` 解析。

## 配置 Schema [C:UPSTREAM]

### 支持的 provider 名称集合

对齐 TS `WebSearchProviderNameSchema`（12 个值）。

```rust
use serde::Deserialize;
use std::str::FromStr;

/// 支持的 web search provider 名称。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WebSearchProviderName {
    Duckduckgo,
    Serpapi,
    Searchapi,
    Serper,
    Bing,
    Baidu,
    Serply,
    Searxng,
    Tavily,
    Exa,
    Perplexity,
    Moonshot,
}

impl std::fmt::Display for WebSearchProviderName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Duckduckgo => "duckduckgo",
            Self::Serpapi => "serpapi",
            Self::Searchapi => "searchapi",
            Self::Serper => "serper",
            Self::Bing => "bing",
            Self::Baidu => "baidu",
            Self::Serply => "serply",
            Self::Searxng => "searxng",
            Self::Tavily => "tavily",
            Self::Exa => "exa",
            Self::Perplexity => "perplexity",
            Self::Moonshot => "moonshot",
        };
        f.write_str(s)
    }
}

impl FromStr for WebSearchProviderName {
    type Err = WebSearchError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "duckduckgo" => Ok(Self::Duckduckgo),
            "serpapi" => Ok(Self::Serpapi),
            "searchapi" => Ok(Self::Searchapi),
            "serper" => Ok(Self::Serper),
            "bing" => Ok(Self::Bing),
            "baidu" => Ok(Self::Baidu),
            "serply" => Ok(Self::Serply),
            "searxng" => Ok(Self::Searxng),
            "tavily" => Ok(Self::Tavily),
            "exa" => Ok(Self::Exa),
            "perplexity" => Ok(Self::Perplexity),
            "moonshot" => Ok(Self::Moonshot),
            _ => Err(WebSearchError::UnknownProvider(s.to_string())),
        }
    }
}
```

### 单个 provider 配置

```rust
use serde_json::Map;

/// 单个 web search provider 配置。
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WebSearchProviderConfig {
    pub provider: WebSearchProviderName,

    #[serde(default)]
    pub api_key: Option<String>,

    #[serde(default, rename = "timeoutMs")]
    pub timeout_ms: Option<u64>,

    #[serde(default)]
    pub options: Option<Map<String, serde_json::Value>>,
}
```

- `timeout_ms` 范围 1000..=120000，与 TS 一致；解析时校验。
- `options` 为 provider 特定参数，每个 provider 工厂自行解析。
- `deny_unknown_fields` 防止拼写错误；但 `options` 内部保留未知字段由各 provider schema 决定。

### 顶层 `services.webSearch` 配置

```rust
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WebSearchConfig {
    pub primary: WebSearchProviderConfig,

    #[serde(default)]
    pub secondary: Option<WebSearchProviderConfig>,
}
```

### `services` 配置容器

```rust
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ServicesConfig {
    #[serde(default, rename = "webSearch")]
    pub web_search: Option<WebSearchConfig>,

    #[serde(default, rename = "moonshotSearch")]
    pub moonshot_search: Option<serde_json::Value>,

    #[serde(default, rename = "moonshotFetch")]
    pub moonshot_fetch: Option<serde_json::Value>,
}
```

- `moonshotSearch` / `moonshotFetch` 本次不实现，但保留 schema 字段以免 TS 配置迁移时报错；解析为 `serde_json::Value` 并忽略。
- `services` 整体在 `ody-config` 顶层以 `Option<ServicesConfig>` 出现。

### TOML 示例

```toml
[services.webSearch]
primary = { provider = "duckduckgo" }
secondary = { provider = "bing", apiKey = "...", timeoutMs = 25000 }
```

---

## Provider 注册表 [C:UPSTREAM]

对齐 TS `WebSearchProviderRegistry`。

```rust
use std::collections::HashMap;
use std::sync::Arc;

pub struct WebSearchProviderFactory {
    create: Arc<
        dyn Fn(&WebSearchProviderConfig, &ProviderFactoryDeps) -> Result<Arc<dyn WebSearchProvider>, WebSearchError>
            + Send
            + Sync,
    >,
}

impl WebSearchProviderFactory {
    pub fn create(
        &self,
        config: &WebSearchProviderConfig,
        deps: &ProviderFactoryDeps,
    ) -> Result<Arc<dyn WebSearchProvider>, WebSearchError> {
        (self.create)(config, deps)
    }
}

pub struct WebSearchProviderRegistry {
    factories: HashMap<WebSearchProviderName, WebSearchProviderFactory>,
}

impl WebSearchProviderRegistry {
    pub fn new() -> Self {
        Self {
            factories: HashMap::new(),
        }
    }

    pub fn register<F>(&mut self, name: WebSearchProviderName, factory: F)
    where
        F: Fn(&WebSearchProviderConfig, &ProviderFactoryDeps) -> Result<Arc<dyn WebSearchProvider>, WebSearchError>
            + Send
            + Sync
            + 'static,
    {
        self.factories.insert(
            name,
            WebSearchProviderFactory {
                create: Arc::new(factory),
            },
        );
    }

    pub fn create(
        &self,
        config: &WebSearchProviderConfig,
        deps: &ProviderFactoryDeps,
    ) -> Result<Arc<dyn WebSearchProvider>, WebSearchError> {
        let factory = self
            .factories
            .get(&config.provider)
            .ok_or_else(|| WebSearchError::UnknownProvider(config.provider.to_string()))?;
        factory.create(config, deps)
    }

    pub fn has(&self, name: WebSearchProviderName) -> bool {
        self.factories.contains_key(&name)
    }
}
```

### 工厂依赖

```rust
#[derive(Debug, Default, Clone)]
pub struct ProviderFactoryDeps {
    /// 可选的 HTTP client；未提供时使用 provider 内部默认 client。
    pub http_client: Option<reqwest::Client>,

    /// Moonshot 搜索服务专属配置（来自 `services.moonshotSearch`）。
    pub moonshot_service_config: Option<serde_json::Value>,
}
```

- `http_client` 允许测试注入 mock client；也允许调用方统一 TLS 配置。
- `moonshot_service_config` 用于 `moonshot` provider 需要访问 `services.moonshotSearch` 的 baseUrl/apiKey/oauth 的场景。

### 默认注册表

```rust
pub fn create_default_web_search_registry() -> WebSearchProviderRegistry {
    use crate::providers::*;

    let mut registry = WebSearchProviderRegistry::new();

    registry.register(WebSearchProviderName::Duckduckgo, |config, deps| {
        Ok(Arc::new(DuckDuckGoProvider::from_config(config, deps)?))
    });
    registry.register(WebSearchProviderName::Serpapi, |config, deps| {
        Ok(Arc::new(SerpApiProvider::from_config(config, deps)?))
    });
    registry.register(WebSearchProviderName::Searchapi, |config, deps| {
        Ok(Arc::new(SearchApiProvider::from_config(config, deps)?))
    });
    registry.register(WebSearchProviderName::Serper, |config, deps| {
        Ok(Arc::new(SerperProvider::from_config(config, deps)?))
    });
    registry.register(WebSearchProviderName::Bing, |config, deps| {
        Ok(Arc::new(BingProvider::from_config(config, deps)?))
    });
    registry.register(WebSearchProviderName::Baidu, |config, deps| {
        Ok(Arc::new(BaiduProvider::from_config(config, deps)?))
    });
    registry.register(WebSearchProviderName::Serply, |config, deps| {
        Ok(Arc::new(SerplyProvider::from_config(config, deps)?))
    });
    registry.register(WebSearchProviderName::Searxng, |config, deps| {
        Ok(Arc::new(SearXNGProvider::from_config(config, deps)?))
    });
    registry.register(WebSearchProviderName::Tavily, |config, deps| {
        Ok(Arc::new(TavilyProvider::from_config(config, deps)?))
    });
    registry.register(WebSearchProviderName::Exa, |config, deps| {
        Ok(Arc::new(ExaProvider::from_config(config, deps)?))
    });
    registry.register(WebSearchProviderName::Perplexity, |config, deps| {
        Ok(Arc::new(PerplexityProvider::from_config(config, deps)?))
    });
    registry.register(WebSearchProviderName::Moonshot, |config, deps| {
        Ok(Arc::new(MoonshotProvider::from_config(config, deps)?))
    });

    registry
}
```

- 每个 provider 实现 `from_config(&WebSearchProviderConfig, &ProviderFactoryDeps) -> Result<Self, WebSearchError>`。
- 工厂返回 `Arc<dyn WebSearchProvider>` 以方便共享和 trait object。

---

## Fallback Provider [C:UPSTREAM]

对齐 TS `FallbackWebSearchProvider`。

```rust
use std::sync::Arc;
use tracing::debug;

pub struct FallbackWebSearchProvider {
    name: &'static str,
    primary: Arc<dyn WebSearchProvider>,
    secondary: Option<Arc<dyn WebSearchProvider>>,
}

impl FallbackWebSearchProvider {
    pub fn new(
        primary: Arc<dyn WebSearchProvider>,
        secondary: Option<Arc<dyn WebSearchProvider>>,
    ) -> Self {
        Self {
            name: "fallback",
            primary,
            secondary,
        }
    }
}

#[async_trait]
impl WebSearchProvider for FallbackWebSearchProvider {
    fn name(&self) -> &str {
        self.name
    }

    async fn search(
        &self,
        query: &str,
        options: WebSearchOptions,
    ) -> Result<Vec<WebSearchResult>, WebSearchError> {
        debug!(provider = %self.primary.name(), "web_search.attempt");

        match self.primary.search(query, options.clone()).await {
            Ok(results) => {
                debug!(provider = %self.primary.name(), result_count = results.len(), "web_search.success");
                Ok(results)
            }
            Err(primary_error) => {
                let category = classify_search_error(&primary_error);
                debug!(provider = %self.primary.name(), error_category = %category, "web_search.failure");

                let Some(secondary) = &self.secondary else {
                    return Err(primary_error);
                };

                if !is_retryable_search_error(&primary_error) {
                    return Err(primary_error);
                }

                debug!(provider = %secondary.name(), "web_search.attempt");
                match secondary.search(query, options).await {
                    Ok(results) => {
                        debug!(provider = %secondary.name(), result_count = results.len(), "web_search.success");
                        Ok(results)
                    }
                    Err(secondary_error) => {
                        let category = classify_search_error(&secondary_error);
                        debug!(provider = %secondary.name(), error_category = %category, "web_search.failure");
                        Err(secondary_error)
                    }
                }
            }
        }
    }
}
```

### 运行时解析函数

```rust
pub fn resolve_web_search_provider(
    config: &WebSearchConfig,
    deps: &ProviderFactoryDeps,
) -> Result<Arc<dyn WebSearchProvider>, WebSearchError> {
    let registry = create_default_web_search_registry();
    let primary = registry.create(&config.primary, deps)?;
    let secondary = config
        .secondary
        .as_ref()
        .map(|cfg| registry.create(cfg, deps))
        .transpose()?;
    Ok(Arc::new(FallbackWebSearchProvider::new(primary, secondary)))
}
```

- `app-server` 在 `on_thread_start` 或配置变更时调用此函数。
- 若 `services.webSearch` 不存在，则返回 `None`，不注册工具。

---

## 错误分类与 retryable 判定 [C:UPSTREAM]

已在 `trait-crate.md` 中定义 `WebSearchError`；此处定义从任意错误转换和 retryable 判定的规则。

### 转换规则

```rust
pub fn classify_search_error<E: std::fmt::Display>(error: E) -> WebSearchError {
    let message = error.to_string();
    let lower = message.to_lowercase();

    if lower.contains("abort") {
        return WebSearchError::Cancelled(message);
    }
    if lower.contains("timed out") || lower.contains("timeout") {
        return WebSearchError::Timeout(message);
    }
    if lower.contains("401") || lower.contains("403") || lower.contains("unauthorized") || lower.contains("authentication") {
        return WebSearchError::Authentication(message);
    }
    if lower.contains("429") {
        return WebSearchError::RateLimit(message);
    }
    if lower.contains("500") || lower.contains("502") || lower.contains("503") || lower.contains("504") || lower.contains("http 5") {
        return WebSearchError::Server(message);
    }
    if lower.contains("network") || lower.contains("dns") || lower.contains("connection") || lower.contains("fetch") {
        return WebSearchError::Network(message);
    }
    WebSearchError::Other(message)
}

pub fn is_retryable_search_error(error: &WebSearchError) -> bool {
    matches!(
        error,
        WebSearchError::Timeout(_)
            | WebSearchError::RateLimit(_)
            | WebSearchError::Network(_)
            | WebSearchError::Server(_)
    )
}
```

### Provider 实现层建议

- `reqwest::Error` 优先使用其内置方法：`is_timeout()` → `WebSearchError::Timeout`；`status()` 4xx/5xx 直接映射；`is_connect()` → `WebSearchError::Network`。
-  Provider 内部解析失败（如 JSON 缺失字段）映射为 `WebSearchError::Other`，不 retryable。

---

## 配置解析时校验 [C:INFERRED]

`ody-config` 在解析 `services.webSearch` 时应执行：
1. `primary.provider` 必须有效（通过 `WebSearchProviderName` 反序列化自然保证）。
2. `primary.timeout_ms` 若存在，必须在 1000..=120000 范围内；否则返回配置错误。
3. `secondary` 若存在，与 primary 规则相同。
4. 若 `provider` 需要 `api_key`（如 `bing`、`serpapi` 等），在创建 provider 时检查，而非 TOML 解析时；因为某些 provider 可能从环境或 OAuth 获取 key。

---

## 风险与决策

| 决策 | 选择 | 理由 |
|---|---|---|
| 注册表是否允许运行时动态注册新 provider | 否 | 仅支持内置 12 个；与 TS 一致。 |
| `options` 用 `serde_json::Map` 还是强类型 struct | `Map` | 与 TS 一致；各 provider 工厂自行解析。 |
| `timeout_ms` 默认值 | 25000 ms | 与 TS `DEFAULT_WEB_SEARCH_TIMEOUT_MS` 一致。 |
| `services.moonshotSearch` 是否本次实现 | 否 | 但保留字段避免解析错误。 |
| `api_key` 为空字符串时是否允许 | 部分 provider 允许 | DuckDuckGo 不需要 key；Bing 等需要 key 的 provider 在创建时报错。 |

---

## 测试要点 [C:INFERRED]

- 注册表 `unknown provider` 报错。
- 注册表 `create` 返回正确具体类型（通过 `name()` 断言）。
- Fallback primary 成功时不调用 secondary。
- Fallback primary 非 retryable 错误时不调用 secondary。
- Fallback primary retryable 错误时调用 secondary 并返回 secondary 结果。
- Fallback primary 和 secondary 都失败时返回 secondary 错误。
- 配置解析：`timeout_ms` 越界报错；`provider` 大小写不敏感；未知 provider 名报错；`deny_unknown_fields` 拒绝拼写错误。
