# Part 4 — 扩展注册与配置集成

## 目标

把 `ody-browser-control` 接入 `app-server` 的扩展体系，复用 `ody-web-search` 已验证的「工具 crate + extension 注册 + `ServicesConfig`」模式。审批入口在扩展层：`ody-browser-control` 返回 `BrowserControlApprovalTicket`，扩展层转换为 `GuardianApprovalRequest::BrowserAction` 并调用 `review_approval_request` `[C:INFERRED]`。

## 配置 Schema 扩展

### 新增 `BrowserControlConfig` 到 `ServicesConfig`

在 `ody-web-search/src/config.rs` 的 `ServicesConfig` 中新增字段：`[C:UPSTREAM]`

```rust
// ody-web-search/src/config.rs
#[derive(Clone, Deserialize, Serialize, PartialEq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ServicesConfig {
    #[serde(rename = "webSearch")]
    pub web_search: Option<WebSearchConfig>,

    #[serde(rename = "browser", default)]
    pub browser: Option<BrowserControlConfig>,
}
```

`BrowserControlConfig` 定义在 `ody-browser-control/src/config.rs`（Part 2 已描述），通过 `ody-browser-control` crate 导出。`ody-web-search` 将依赖 `ody-browser-control` 仅用于该配置类型；为了避免循环依赖，`ody-browser-control` 不能依赖 `ody-web-search`。

> 注：这是 roadmap 中提到的 `ServicesConfig` 归属问题。最小修改方案是在 `ody-web-search` 中新增字段；若未来服务继续增多，再拆分 `ody-services-config` crate。

### 配置示例

```toml
[services.browser]
# chrome_executable = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
headless = true
viewport = { width = 1280, height = 720 }
sandbox = false
disable_extensions = true
extra_args = ["--disable-gpu"]
launch_timeout_ms = 30000
command_timeout_ms = 30000
```

## 扩展模块

新增 `app-server/src/browser_control_extension.rs`，模式与 `app-server/src/web_search_extension.rs` 一致：`[C:UPSTREAM]`

```rust
use std::sync::Arc;

use ody_core::config::Config;
use ody_extension_api::{
    ConfigContributor, ExtensionData, ExtensionFuture, ExtensionRegistryBuilder,
    ThreadLifecycleContributor, ThreadStartInput, ToolCall, ToolContributor,
};
use ody_browser_control::{
    config::BrowserControlConfig,
    session::{BrowserSession, BrowserThreadState},
};

#[derive(Clone)]
struct BrowserControlExtension;

type StoredState = Arc<BrowserThreadState>;

impl BrowserControlExtension {
    fn should_enable(config: &Config) -> bool {
        config.features.enabled(ody_features::Feature::BrowserUse)
            || config.features.enabled(ody_features::Feature::ComputerUse)
            || config.features.enabled(ody_features::Feature::BrowserUseExternal)
    }

    async fn create_state(
        cfg: &BrowserControlConfig,
        external_ws_url: Option<&str>,
    ) -> Option<StoredState> {
        let session = if let Some(url) = external_ws_url {
            BrowserSession::connect(url).await
        } else {
            BrowserSession::launch(cfg.clone()).await
        };
        match session {
            Ok(session) => {
                let page = match session.new_page().await {
                    Ok(page) => page,
                    Err(e) => {
                        tracing::error!("failed to create initial page: {e}");
                        let _ = session.close().await;
                        return None;
                    }
                };
                Some(Arc::new(BrowserThreadState::new(session, page)))
            }
            Err(e) => {
                tracing::error!("failed to start browser session: {e}");
                None
            }
        }
    }

    fn config_from_services(services: &ody_web_search::config::ServicesConfig) -> Option<&BrowserControlConfig> {
        services.browser.as_ref()
    }
}

impl ThreadLifecycleContributor<Config> for BrowserControlExtension {
    fn on_thread_start<'a>(
        &'a self,
        input: ThreadStartInput<'a, Config>,
    ) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            if !Self::should_enable(input.config) {
                return;
            }
            let Some(services) = input.config.services.as_ref() else {
                return;
            };
            let Some(browser_cfg) = Self::config_from_services(services) else {
                // 使用默认配置启动
                let default_cfg = BrowserControlConfig::default();
                if let Some(state) = Self::create_state(&default_cfg, None).await {
                    input.thread_store.insert(state);
                }
                return;
            };

            let external_url = if input.config.features.enabled(ody_features::Feature::BrowserUseExternal) {
                browser_cfg.connect_url.as_deref()
            } else {
                None
            };

            if let Some(state) = Self::create_state(browser_cfg, external_url).await {
                input.thread_store.insert(state);
            }
        })
    }
}

impl ConfigContributor<Config> for BrowserControlExtension {
    fn on_config_changed(
        &self,
        _session_store: &ExtensionData,
        thread_store: &ExtensionData,
        previous_config: &Config,
        new_config: &Config,
    ) {
        // 如果功能被整体关闭，移除已存储的 state。
        if !Self::should_enable(new_config) {
            let _: Option<StoredState> = thread_store.remove();
            return;
        }

        // 标记 state 为 stale：下次工具调用时检查是否需要重启 session。
        if let Some(state) = thread_store.get::<StoredState>() {
            let prev_cfg = previous_config.services.as_ref().and_then(Self::config_from_services);
            let new_cfg = new_config.services.as_ref().and_then(Self::config_from_services);
            if let (Some(prev), Some(new)) = (prev_cfg, new_cfg) {
                if prev.requires_restart(new) {
                    state.mark_stale();
                }
            }
        }
    }
}

impl ToolContributor for BrowserControlExtension {
    fn tools(
        &self,
        session_store: &ExtensionData,
        thread_store: &ExtensionData,
    ) -> Vec<Arc<dyn ody_extension_api::ToolExecutor<ToolCall>>> {
        let Some(state) = thread_store.get::<StoredState>() else {
            return Vec::new();
        };
        // config 从 session_store 或 thread_store 获取
        let config = session_store
            .get::<Config>()
            .or_else(|| thread_store.get::<Config>())
            .expect("config available in extension store");

        build_browser_tools(state, &config.features)
    }
}

pub fn install(registry: &mut ExtensionRegistryBuilder<Config>) {
    let extension = Arc::new(BrowserControlExtension);
    registry.thread_lifecycle_contributor(extension.clone());
    registry.config_contributor(extension.clone());
    registry.tool_contributor(extension);
}

## 审批集成

工具实现 `ToolExecutor<ToolCall>` 时，对于需要审批的操作返回 `FunctionCallError::NeedsApproval(BrowserControlApprovalTicket)` 或类似中间状态；`app-server` 扩展层捕获该状态并转换为 guardian 请求：`[C:INFERRED]`

```rust
impl ToolExecutor<ToolCall> for BrowserNavigateTool {
    fn handle(&self, call: ToolCall) -> ToolExecutorFuture<'_> {
        let state = self.state.clone();
        Box::pin(async move {
            let input = parse_input(&call)?;
            if let Some(deny_reason) = is_url_blocked(&input.url) {
                return Ok(JsonToolOutput::new(json!({
                    "blocked": true,
                    "block_reason": deny_reason,
                })));
            }
            let ticket = BrowserControlApprovalTicket {
                tool_name: "browser__navigate".to_string(),
                action_kind: BrowserActionKind::Navigate,
                url: Some(input.url.clone()),
                script_preview: None,
                selector: None,
                raw_method: None,
                raw_params: None,
            };
            // 扩展层包装此调用：先审批，再执行 state.navigate(...)
            Err(FunctionCallError::NeedsApproval(ticket))
        })
    }
}

// app-server 扩展层
async fn handle_browser_tool_with_approval(
    session: Arc<Session>,
    turn_ctx: Arc<TurnContext>,
    tool: Arc<dyn ToolExecutor<ToolCall>>,
    call: ToolCall,
) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
    match tool.handle(call.clone()).await {
        Ok(output) => Ok(output),
        Err(FunctionCallError::NeedsApproval(ticket)) => {
            let request = GuardianApprovalRequest::BrowserAction {
                id: new_guardian_review_id(),
                turn_id: call.turn_id.clone(),
                tool_name: ticket.tool_name,
                action_kind: ticket.action_kind,
                url: ticket.url,
                script: ticket.script_preview,
                selector: ticket.selector,
                raw_method: ticket.raw_method,
                raw_params: ticket.raw_params,
            };
            match review_approval_request(request, &session, &turn_ctx).await {
                Approved => tool.execute_after_approval(call).await,
                Rejected | Cancelled => Err(FunctionCallError::Fatal(
                    "browser action rejected".to_string(),
                )),
            }
        }
        Err(e) => Err(e),
    }
}
```

> 注：如果 `FunctionCallError` 没有 `NeedsApproval` 变体，则改为在扩展层预先判断 `requires_approval()`，先请求审批再调用工具。实现时选择最自然的方案。

### 审批调用栈边界

- `ody-browser-control` 只产生 `BrowserControlApprovalTicket`，不依赖 `ody-core`。
- `app-server` 扩展层持有 `Session` 和 `TurnContext`，负责调用 `review_approval_request`。
- 这避免了 `app-server` → `ody-browser-control` → `ody-core` → `app-server` 的循环依赖。
```

## 注册点

在 `app-server/src/extensions.rs:97` 附近新增：`[C:UPSTREAM]`

```rust
// app-server/src/extensions.rs
#[path = "browser_control_extension.rs"]
mod browser_control_extension;

// 在 web_search_extension::install 之后
browser_control_extension::install(&mut builder);
```

## 配置热更新策略

`ConfigContributor::on_config_changed` 执行以下策略：`[C:INFERRED]`

- 如果 `BrowserUse`/`ComputerUse`/`BrowserUseExternal` 被关闭，立即移除 `thread_store` 中的 state，下一 turn 不再暴露 browser 工具。
- 如果 feature 仍开启，但 `services.browser` 中需要重启进程才能生效的参数（`chrome_executable`、`headless`、`sandbox`、`extra_args`、`viewport`、`max_concurrent_browsers`）发生变化，则标记 `BrowserThreadState` 为 `stale`。
- 下次工具调用时，由 `BrowserThreadState` 检查 `is_stale()`；若 stale，先关闭旧 session 并创建新 session，再执行工具操作。
- 不需要重启的参数变化（如 `command_timeout_ms`、`launch_timeout_ms`）可通过运行时热更新生效；实现时评估是否也纳入 `requires_restart()`。

**优势**：避免每次配置变化都立即重启 Chrome；只有真正影响工具行为的参数变化才触发重启。

**限制**：在线程运行时修改 `services.browser` 不会立即生效，直到下一次工具调用触发 stale 检查。文档中需说明哪些参数需要重启。

## 与现有 WebSearch 扩展的共存

两个扩展共享 `ServicesConfig`，但各自读取自己的字段：`[C:INFERRED]`

- `WebSearchExtension` 读取 `services.web_search`。
- `BrowserControlExtension` 读取 `services.browser`。
- 互不影响，可独立启用/禁用。

## Feature 与配置的组合行为

| `BrowserUse` | `ComputerUse` | `BrowserUseExternal` | `services.browser` | 行为 |
|---|---|---|---|---|
| 开 | 任意 | 关 | 有 | 启动 headless Chrome，暴露完整 `browser__*` 工具 |
| 开 | 任意 | 关 | 无 | 使用默认配置启动 headless Chrome |
| 关 | 开 | 关 | 有/无 | 启动 headless Chrome，主要暴露 `click`/`type` 等 computer 工具 |
| 任意 | 任意 | 开 | 提供 `connect_url` | 连接外部 Chrome，不启动新进程 |
| 关 | 关 | 任意 | 任意 | 不启动 session，`tools()` 返回空 |

## 错误处理与降级

- 如果 `BrowserSession::launch` 失败（Chrome 未找到、启动超时），`on_thread_start` 记录错误日志，**不 panic**，也不在 thread store 中插入 state。结果是模型看不到 browser 工具，但线程其他功能正常。
- 如果启动失败但配置存在，可在后续 `tools()` 调用时再次尝试懒启动（可选优化）。本期采用失败即禁用的策略，更简单可预测。
- `BrowserUseExternal` 启用但没有 `connect_url` 时，回退到本地启动（如果 `BrowserUse` 也启用）；否则不创建 session。

## 测试策略

- **单元测试**：在 `app-server/src/browser_control_extension.rs` 的 `tests` 模块中，使用 mock `BrowserThreadState` 和 `Config` 验证 `tools()` 在 feature 关闭时返回空、在 `BrowserUseFullCdpAccess` 关闭时不包含 `execute_raw_cdp`。
- **配置测试**：在 `ody-web-search/src/config.rs` 的 `tests` 中验证 `ServicesConfig` 新增 `browser` 字段后的序列化/反序列化。
- **集成测试**：通过 `app-server-test-client` 验证线程启动时 browser 工具是否出现（需要真实 Chrome 或 mock session）。

## 依赖关系

```
app-server
├── ody-browser-control (new)
│   ├── chromiumoxide
│   ├── serde, serde_json, schemars, tokio, tempfile
│   └── ody-utils-image (optional)
├── ody-web-search (config only, for ServicesConfig)
├── ody-extension-api
└── ody-features
```

注意：为了避免 `ody-web-search` 与 `ody-browser-control` 之间的循环依赖，
- `ody-browser-control` 不依赖 `ody-web-search`。
- `ody-web-search` 依赖 `ody-browser-control` 仅获取 `BrowserControlConfig` 类型。
- 如果未来拆分 `ody-services-config`，则 `ody-web-search` 和 `ody-browser-control` 都依赖它，进一步解耦。

## 接口草案

```rust
// ody-browser-control/src/lib.rs
pub mod config;
pub mod session;
pub mod tools;

// 导出给 extension 使用
pub use session::{BrowserSession, BrowserThreadState};
```

```rust
// app-server/src/browser_control_extension.rs
pub fn install(registry: &mut ExtensionRegistryBuilder<Config>);
```

## 配置变更追踪

为便于未来实现动态 session 重建，建议在 `BrowserControlConfig` 上实现 `Hash` 或序列化比较：`[C:INFERRED]`

```rust
impl BrowserControlConfig {
    /// 判断是否需要重启 Chrome 进程才能生效。
    pub fn requires_restart(&self, other: &Self) -> bool {
        self.chrome_executable != other.chrome_executable
            || self.headless != other.headless
            || self.sandbox != other.sandbox
            || self.extra_args != other.extra_args
            || self.viewport != other.viewport
            || self.launch_timeout_ms != other.launch_timeout_ms
    }
}
```

本期 `on_config_changed` 不调用此函数，仅作为保留接口。
