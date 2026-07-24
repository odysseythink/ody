# Part 4 — `services.webSearch` 配置解析与 app-server 注入

## 来源 [C:UPSTREAM]

- TS: `packages/agent-core-shared/src/config.ts:341-376` — `OdyConfigSchema` 包含 `services: ServicesConfigSchema.optional()`。
- TS: `packages/agent-core-shared/src/config.ts:141-240` — `MoonshotServiceConfigSchema`、`WebSearchProviderNameSchema`、`WebSearchProviderConfigSchema`、`WebSearchConfigSchema`、`ServicesConfigSchema`。
- TS: `packages/agent-core/src/tools/providers/web-search/runtime.ts:12-30` — `resolveWebSearchRuntime` 从 `OdyConfig` 解析配置并构造 fallback provider。
- Rust 现状：`config/src/config_toml.rs:247-680` — `ConfigToml` 无 `services` 字段；`core/src/config/mod.rs:685` — `Config` 运行时结构体；`app-server/src/extensions.rs:75` — 现有 `ody_web_search_extension::install(&mut builder)` 将被替换为新扩展或新工具 contributor。

## 配置解析 [C:USER]

### 新增 `ServicesConfigToml` 类型

在 `config/src/config_toml.rs` 中新增：

```rust
use ody_web_search::WebSearchConfig;

/// `services` 配置段，兼容 ody-code 格式。
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ServicesConfigToml {
    #[serde(default, rename = "webSearch")]
    pub web_search: Option<WebSearchConfig>,

    #[serde(default, rename = "moonshotSearch")]
    pub moonshot_search: Option<serde_json::Value>,

    #[serde(default, rename = "moonshotFetch")]
    pub moonshot_fetch: Option<serde_json::Value>,
}
```

- `moonshot_search` / `moonshot_fetch` 本次不解析为强类型，仅保留为 `serde_json::Value` 以兼容 TS 配置；实际使用由 `moonshot` web search provider 在 `options` 或 `deps.moonshot_service_config` 中读取。
- 由于 `ConfigToml` 已使用 `#[schemars(deny_unknown_fields)]`，新增 `services` 字段后必须显式定义所有子字段；未知 `services.*` 字段将报错。

### 新增 `ConfigToml::services` 字段

在 `ConfigToml` 中合适位置（例如 `tools` 字段附近）新增：

```rust
#[serde(default)]
pub services: Option<ServicesConfigToml>,
```

### 旧 `web_search` / `web_search_mode` 配置处理 [C:USER]

- `ConfigToml::web_search: Option<WebSearchMode>` 删除。
- `web_search_config` 相关字段（如 `WebSearchToolConfig`）删除。
- 在配置合并阶段检测旧 key：
  - 若 `ConfigToml` 中仍出现 `web_search`（旧 key），在 `startup_warnings` 中添加：
    ```
    The `web_search` config key is deprecated. Use `[services.webSearch]` instead.
    ```
  - 不自动迁移；因为语义完全不同（旧的是 mode，新的是 provider + API key）。

### 配置校验时序

1. TOML 反序列化：由 `serde` + `#[schemars(deny_unknown_fields)]` 保证 schema。
2. `ConfigToml` → `Config` 转换：在 `core/src/config/mod.rs` 中，将 `config_toml.services.web_search` 原样放入 `Config::services_web_search: Option<ody_web_search::WebSearchConfig>`。
3. 配置校验：在 `config/src/config_requirements.rs` 中删除 `WebSearchModeRequirement` 相关校验；新增 `WebSearchConfig` 结构校验由 `ody-web-search` 在 provider 创建时执行（timeout 范围、provider 存在性）。

---

## `Config` 运行时结构 [C:USER]

在 `core/src/config/mod.rs` 的 `Config` 结构体中新增字段：

```rust
pub struct Config {
    // ... 现有字段 ...

    /// `services.webSearch` 配置；未配置时 `None`。
    pub services_web_search: Option<ody_web_search::WebSearchConfig>,
}
```

- 不在这里缓存 provider 实例；`core` 不直接创建 web search provider。
- `Config` 保持可 `Clone`；`WebSearchConfig` 已实现 `Clone`。

---

## app-server 注入 [C:USER]

### 目标

对齐 TS 的 host 注入模式：`app-server` 在 thread start 时根据 `Config.services_web_search` 创建 `Arc<dyn WebSearchProvider>`，并通过 extension 工具注册机制将其注入到 `core` 的工具集合中。

### 方案 A：新建 `WebSearchExtension`（推荐）

新建 crate `ext/web-search` 或保留原目录但完全重写。本次推荐**保留 `ext/web-search` 目录但重写内容**，因为：
- 现有 `app-server/src/extensions.rs:75` 已调用 `ody_web_search_extension::install(&mut builder)`；
- 删除并重建 crate 与保留目录重写工作量相同，保留目录可减少 workspace member 和 Cargo.toml 引用改动。

#### 新 `ext/web-search/src/extension.rs`

```rust
use std::sync::Arc;

use ody_core::config::Config;
use ody_extension_api::ConfigContributor;
use ody_extension_api::ExtensionData;
use ody_extension_api::ExtensionFuture;
use ody_extension_api::ExtensionRegistryBuilder;
use ody_extension_api::ThreadLifecycleContributor;
use ody_extension_api::ThreadStartInput;
use ody_extension_api::ToolContributor;
use ody_web_search::ProviderFactoryDeps;
use ody_web_search::WebSearchProvider;
use ody_web_search::resolve_web_search_provider;

use crate::tool::WebSearchTool;

pub(crate) struct WebSearchExtensionConfig {
    provider: Arc<dyn WebSearchProvider>,
}

impl From<&Config> for Option<WebSearchExtensionConfig> {
    fn from(config: &Config) -> Self {
        let web_search_config = config.services_web_search.as_ref()?;
        let provider = resolve_web_search_provider(web_search_config, &ProviderFactoryDeps::default())
            .map_err(|err| {
                tracing::warn!(error = %err, "failed to create web search provider; WebSearch tool will not be registered");
            })
            .ok()?;
        Some(WebSearchExtensionConfig { provider })
    }
}

impl ThreadLifecycleContributor<Config> for WebSearchExtension {
    fn on_thread_start<'a>(
        &'a self,
        input: ThreadStartInput<'a, Config>,
    ) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            if let Some(config) = Option::<WebSearchExtensionConfig>::from(input.config) {
                input.thread_store.insert(config);
            }
        })
    }
}

impl ConfigContributor<Config> for WebSearchExtension {
    fn on_config_changed(
        &self,
        _session_store: &ExtensionData,
        thread_store: &ExtensionData,
        _previous_config: &Config,
        new_config: &Config,
    ) {
        if let Some(config) = Option::<WebSearchExtensionConfig>::from(new_config) {
            thread_store.insert(config);
        }
    }
}

impl ToolContributor for WebSearchExtension {
    fn tools(
        &self,
        _session_store: &ExtensionData,
        thread_store: &ExtensionData,
    ) -> Vec<Arc<dyn ody_extension_api::ToolExecutor<ody_extension_api::ToolCall>>> {
        let Some(config) = thread_store.get::<WebSearchExtensionConfig>() else {
            return Vec::new();
        };

        vec![Arc::new(WebSearchTool::new(config.provider.clone()))]
    }
}

pub fn install(registry: &mut ExtensionRegistryBuilder<Config>) {
    let extension = Arc::new(WebSearchExtension {});
    registry.thread_lifecycle_contributor(extension.clone());
    registry.config_contributor(extension.clone());
    registry.tool_contributor(extension);
}
```

### 关键不变式

- **无 provider 时不注册工具**：`WebSearchExtensionConfig` 仅在 `services.webSearch` 配置有效且 provider 创建成功时插入 thread store；`ToolContributor::tools` 在找不到配置时返回空 Vec。
- **配置变更动态生效**：`on_config_changed` 重新解析；如果新配置无效则移除旧配置，工具在下一次工具规划时不再出现。
- **错误不阻塞 thread start**：provider 创建失败时记录 warning，thread 继续启动，只是没有 `WebSearch` 工具。

### `ProviderFactoryDeps` 默认值

`app-server` 注入时使用默认 `ProviderFactoryDeps`：

```rust
ProviderFactoryDeps {
    http_client: None,                 // 使用 provider 内部默认 reqwest client
    moonshot_service_config: config.services_moonshot_search.clone(), // `serde_json::Value`
}
```

未来若需要统一 TLS/timeout client，可在此处传入 `ody_client::default_client::build_reqwest_client()`。

---

## 与 `ody-core` 的边界 [C:USER]

- `ody-core` 不直接依赖 `ody-web-search` 的配置类型；它只通过 `WebSearchTool` 的构造函数接收 `Arc<dyn WebSearchProvider>`。
- `WebSearchTool` 定义在 `ext/web-search/src/tool.rs` 或 `core/src/tools/builtin/web_search.rs` 中（见 `tool-tui.md`）。
- 工具 schema、输入解析、输出格式化全部由 `ody-core` 或 `ext/web-search` 完成；`ody-web-search` 只负责搜索调用和结果返回。

---

## 复用分析 [C:INFERRED]

- 复用 `ody_extension_api::ToolContributor` / `ThreadLifecycleContributor` / `ConfigContributor` 接口；现有 `ext/web-search/src/extension.rs` 已展示用法。
- 不复用 `ody_model_provider::create_model_provider` 或 `ody_api::SearchClient`；这些将被删除。
- 不复用 `config/src/config_requirements.rs` 中的 `WebSearchModeRequirement`；同步删除。

---

## 风险与决策

| 决策 | 选择 | 理由 |
|---|---|---|
| 是否保留 `ext/web-search` 目录 | 是，重写内容 | 减少 workspace member 和 Cargo.toml 改动；已存在 install 调用。 |
| provider 创建失败是否阻塞 thread start | 否 | 对齐 TS 的 host 注入语义；错误配置不应导致整个线程崩溃。 |
| 配置变更后是否动态刷新 provider | 是 | 通过 `ConfigContributor` 实现；现有 extension 机制支持。 |
| `moonshot_search` 是否强类型解析 | 否 | 本次仅作为 `serde_json::Value` 透传；`moonshot` provider 自行读取。 |
| 是否允许 `services.webSearch` 中 `secondary` 与 `primary` 同 provider | 是 | 允许但无意义；fallback 逻辑不限制。 |

---

## 测试要点 [C:INFERRED]

- `cargo test -p ody-config`：新增 `services.webSearch` 解析测试；旧 `web_search` 警告测试；`timeout_ms` 越界测试；未知 provider 名测试。
- `cargo test -p ody-app-server`：
  - 未配置 `services.webSearch` 时 `WebSearchTool` 不在工具列表中；
  - 配置有效时 `WebSearchTool` 出现；
  - 配置无效（如 unknown provider）时工具不出现且生成 warning；
  - 配置变更后工具动态出现/消失。
- 需要 mock `ThreadStore` 和 `ExtensionRegistry` 进行单元测试。

---

## 文件改动清单 [C:INFERRED]

- `config/src/config_toml.rs`：新增 `ServicesConfigToml`，新增 `ConfigToml::services` 字段，删除 `web_search` 字段。
- `core/src/config/mod.rs`：新增 `Config::services_web_search`，删除 `web_search_mode` / `web_search_config` 相关字段。
- `ext/web-search/Cargo.toml`：依赖改为 `ody-web-search`、`ody-core`、`ody-extension-api`，移除 `ody-api`、`ody-model-provider`、`ody-protocol` 中旧类型。
- `ext/web-search/src/extension.rs`：完全重写为上述 host 注入逻辑。
- `ext/web-search/src/tool.rs`：重写为 `WebSearchTool`（见 `tool-tui.md`）。
- `app-server/src/extensions.rs`：保持 `ody_web_search_extension::install(&mut builder)` 调用不变。
