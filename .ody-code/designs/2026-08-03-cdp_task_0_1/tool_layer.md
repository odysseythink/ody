# Part 3 — 模型可见工具层（`browser__*` 命名空间）

## 设计原则

- **命名空间**：统一使用 `browser__*` 前缀，与 `core/src/config/mod.rs:3019` 预留的 `"browser"` 命名空间一致 `[C:UPSTREAM]`。
- **工具语义贴近模型**：每个工具描述明确说明可执行的操作、返回格式和限制，降低模型误用。
- **Feature gate 分级**：通过 `BrowserUse`、`BrowserUseFullCdpAccess`、`ComputerUse` 控制工具暴露。
- **审批内聚**：工具实现内部只判断是否需要审批；实际审批逻辑由 Part 4/5 描述的 guardian 集成处理。

## 工具清单

| 工具名 | 操作类型 | 默认暴露 | 审批 | Feature 依赖 |
|---|---|---|---|---|
| `browser__navigate` | 写 | 是 | 是 | `BrowserUse` |
| `browser__go_back` | 写 | 是 | 是 | `BrowserUse` |
| `browser__go_forward` | 写 | 是 | 是 | `BrowserUse` |
| `browser__reload` | 写 | 是 | 是 | `BrowserUse` |
| `browser__screenshot` | 读 | 是 | 否 | `BrowserUse` |
| `browser__evaluate` | 写/执行 | 是 | 是 | `BrowserUse`（外部浏览器模式下默认禁用） |
| `browser__click` | 写 | 是 | 是 | `BrowserUse` 或 `ComputerUse`（外部浏览器模式下默认禁用） |
| `browser__type` | 写 | 是 | 是 | `BrowserUse` 或 `ComputerUse`（外部浏览器模式下默认禁用） |
| `browser__get_dom` | 读 | 是 | 否 | `BrowserUse` |
| `browser__read_logs` | 读 | 是 | 否 | `BrowserUse` |
| `browser__execute_raw_cdp` | 写 | 否 | 是 | `BrowserUseFullCdpAccess`（本地浏览器模式；外部浏览器模式下禁用） |

说明：
- `browser__click` 和 `browser__type` 在 `ComputerUse` 启用时也可以暴露，以支持计算机使用场景下的坐标/元素操作 `[C:INFERRED]`。
- `browser__execute_raw_cdp` 是高风险工具，仅在 `BrowserUseFullCdpAccess` 开启时暴露；默认关闭。
- `browser__new_page` / `browser__close_page` 不暴露给模型，由内部根据工具调用自动管理（一个 turn 一个默认 page）。

## 工具输入/输出 schema

### `browser__navigate`

```json
{
  "type": "object",
  "properties": {
    "url": {
      "type": "string",
      "description": "URL to navigate to. Must be absolute (http://, https://, file://, data:). Localhost, link-local, and private CIDRs are blocked by default; use allow_local_network config to override."
    },
    "wait_for": {
      "type": "string",
      "enum": ["load", "domcontentloaded", "networkidle"],
      "default": "load",
      "description": "When to consider navigation complete."
    }
  },
  "required": ["url"]
}
```

输出：

```json
{
  "success": true,
  "url": "https://example.com",
  "title": "Example Domain",
  "blocked": false,
  "block_reason": null
}
```

- 如果 URL 命中内网 denylist，返回 `blocked: true` 和 `block_reason`，不进入 guardian 审批。

### `browser__screenshot`

```json
{
  "type": "object",
  "properties": {
    "full_page": {
      "type": "boolean",
      "default": false,
      "description": "Capture full scrollable page instead of viewport."
    }
  }
}
```

输出：PNG 图像数据，使用 data URL base64 字符串。

```json
{
  "format": "png",
  "data": "data:image/png;base64,iVBORw0KGgoAAAANSUhEU...",
  "raw_base64": "iVBORw0KGgoAAAANSUhEU...",
  "width": 1280,
  "height": 720
}
```

- `data` 字段为完整 data URL，便于模型直接理解。
- `raw_base64` 为纯 base64 数据，便于后端/协议层转换为 image 类型。
- 截图大小默认限制在 2MB（PNG）；超出时自动压缩或缩放，避免塞满模型上下文。

### `browser__evaluate`

```json
{
  "type": "object",
  "properties": {
    "expression": {
      "type": "string",
      "description": "JavaScript expression to evaluate in the browser. Requires guardian approval. External browser mode disables this tool by default."
    }
  },
  "required": ["expression"]
}
```

输出：

```json
{
  "success": true,
  "result": { "value": "hello" },
  "result_type": "string"
}
```

- 如果 JS 表达式抛出异常，返回 `success: false` 和 `error` 字段。
- 表达式必须走 guardian 审批（即使技术上可视为只读）。
- 静态检查 `is_forbidden_expression` 快速拒绝 `document.cookie`、`localStorage`、`indexedDB` 等关键字；但仅作为第一层，不能替代审批。
- 外部浏览器模式下 `evaluate` 默认禁用，除非 `external_browser_allow_sensitive=true`。
- 表达式执行时间受 `command_timeout_ms` 限制。

### `browser__click`

```json
{
  "type": "object",
  "properties": {
    "selector": {
      "type": "string",
      "description": "CSS selector of the element to click."
    },
    "x": { "type": "number", "description": "Optional viewport X coordinate." },
    "y": { "type": "number", "description": "Optional viewport Y coordinate." }
  },
  "oneOf": [
    { "required": ["selector"] },
    { "required": ["x", "y"] }
  ]
}
```

- 如果提供 `selector`，先 `find_element`，再获取元素中心坐标点击。
- 如果提供 `x`/`y`，直接调用 `Page::click(Point)`。

### `browser__type`

```json
{
  "type": "object",
  "properties": {
    "selector": {
      "type": "string",
      "description": "CSS selector of the input element."
    },
    "text": {
      "type": "string",
      "description": "Text to type into the element."
    },
    "submit": {
      "type": "boolean",
      "default": false,
      "description": "Whether to press Enter after typing."
    }
  },
  "required": ["selector", "text"]
}
```

### `browser__get_dom`

```json
{
  "type": "object",
  "properties": {
    "selector": {
      "type": "string",
      "description": "Optional CSS selector to narrow down returned DOM."
    },
    "max_depth": {
      "type": "integer",
      "default": 3,
      "description": "Maximum depth of DOM tree to return."
    }
  }
}
```

输出：简化后的 DOM 树文本，包含 tag、id、class、role、aria-label 和文本内容（截断后）。

### `browser__read_logs`

```json
{
  "type": "object",
  "properties": {
    "kind": {
      "type": "string",
      "enum": ["console", "network", "all"],
      "default": "all"
    },
    "level": {
      "type": "string",
      "enum": ["verbose", "info", "warning", "error"],
      "default": "info"
    },
    "limit": {
      "type": "integer",
      "default": 100
    }
  }
}
```

输出：

```json
{
  "console": [
    { "level": "error", "message": "Failed to load resource", "source": "network" }
  ],
  "network": [
    { "url": "https://api.example.com/data", "status": 200, "method": "GET" }
  ]
}
```

### `browser__execute_raw_cdp`

```json
{
  "type": "object",
  "properties": {
    "method": {
      "type": "string",
      "description": "CDP method name, e.g. 'Page.printToPDF'."
    },
    "params": {
      "type": "object",
      "description": "CDP method parameters as JSON object."
    }
  },
  "required": ["method"]
}
```

输出：原始 CDP 响应的 JSON 值。

- 仅当 `BrowserUseFullCdpAccess` 启用时暴露。
- 仍然需要 guardian 审批，因为可以执行任意 CDP 命令。

## Feature Gate 映射

`ody_extension_api::ToolContributor::tools()` 中根据 `Config` 的 feature flag 过滤：`[C:INFERRED]`

```rust
impl ToolContributor for BrowserControlExtension {
    fn tools(
        &self,
        session_store: &ExtensionData,
        thread_store: &ExtensionData,
    ) -> Vec<Arc<dyn ToolExecutor<ToolCall>>> {
        let state = thread_store.get::<BrowserThreadState>()?;
        let config = session_store.get::<Config>().or_else(|| thread_store.get::<Config>())?;
        let mut tools: Vec<Arc<dyn ToolExecutor<ToolCall>>> = Vec::new();

        if !config.features.enabled(ody_features::Feature::BrowserUse)
            && !config.features.enabled(ody_features::Feature::ComputerUse) {
            return tools;
        }

        let has_computer_use = config.features.enabled(ody_features::Feature::ComputerUse);
        let has_full_cdp = config.features.enabled(ody_features::Feature::BrowserUseFullCdpAccess);

        tools.push(Arc::new(BrowserNavigateTool::new(state.clone())));
        tools.push(Arc::new(BrowserScreenshotTool::new(state.clone())));
        tools.push(Arc::new(BrowserEvaluateTool::new(state.clone())));
        tools.push(Arc::new(BrowserGetDomTool::new(state.clone())));
        tools.push(Arc::new(BrowserReadLogsTool::new(state.clone())));

        if config.features.enabled(ody_features::Feature::BrowserUse) || has_computer_use {
            tools.push(Arc::new(BrowserClickTool::new(state.clone())));
            tools.push(Arc::new(BrowserTypeTool::new(state.clone())));
        }

        if has_full_cdp {
            tools.push(Arc::new(BrowserExecuteRawCdpTool::new(state.clone())));
        }

        tools
    }
}
```

注意：
- `BrowserUse` 和 `ComputerUse` 任一启用即可暴露基础浏览工具。
- `ComputerUse` 单独启用时，重点暴露 `click`/`type` 等坐标级工具，而 `navigate`/`evaluate` 仍可用（因为浏览器控制是基础）。

## Tool 实现结构

每个工具实现 `ody_tools::ToolExecutor<ToolCall>`，参考 `ody-web-search/src/tool.rs`：`[C:UPSTREAM]`

```rust
pub struct BrowserNavigateTool {
    state: Arc<BrowserThreadState>,
}

impl ToolExecutor<ToolCall> for BrowserNavigateTool {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("browser__navigate")
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function(ody_tools::ResponsesApiTool {
            name: "browser__navigate".to_string(),
            description: "Navigate the browser to a URL.".to_string(),
            strict: true,
            parameters: parse_tool_input_schema(&NAVIGATE_SCHEMA).expect("valid schema"),
            defer_loading: None,
            output_schema: None,
        })
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Direct
    }

    fn handle(&self, call: ToolCall) -> ToolExecutorFuture<'_> {
        let state = self.state.clone();
        Box::pin(async move {
            // 1. 解析输入
            // 2. 如果 need_approval，调用 approval_service
            // 3. 调用 state.current_page.navigate(url).await
            // 4. 返回 JsonToolOutput
        })
    }
}
```

## 审批标记

每个工具内部声明 `requires_approval()`：`[C:INFERRED]`

```rust
trait BrowserTool {
    fn requires_approval(&self) -> bool;
    fn approval_reason(&self, input: &serde_json::Value) -> String;
}

impl BrowserTool for BrowserNavigateTool {
    fn requires_approval(&self) -> bool { true }
    fn approval_reason(&self, input: &serde_json::Value) -> String {
        format!("Navigate browser to {}", input["url"].as_str().unwrap_or("unknown"))
    }
}
```

- 只读工具（`screenshot`、`get_dom`、`read_logs`）返回 `false`。
- 写工具（`navigate`、`evaluate`、`click`、`type`、`execute_raw_cdp`）返回 `true`。
- 具体 approval 流程和 GuardianApprovalRequest 变体在 Part 5 描述。

## 输出截断与模型消耗

- `browser__get_dom` 返回的 DOM 文本必须截断到可配置上限（默认 8KB），避免超出模型上下文。
- `browser__read_logs` 的 `limit` 参数上限为 1000，同时受总字节数限制（默认 1MB）。
- `browser__evaluate` 的返回结果如果是复杂对象，只序列化可 JSON 序列化部分，并截断字符串。
- screenshot 输出大小限制在 2MB（PNG），超出时自动压缩或缩放。

## 错误到工具输出的映射

| 内部错误 | 工具输出 |
|---|---|
| `ChromeNotFound` | `FunctionCallError::Fatal` |
| `LaunchFailed` | `FunctionCallError::Fatal` |
| `QuotaExceeded` | `FunctionCallError::Retryable`（等待全局配额释放） |
| `CommandFailed` | `Retryable` 如果原因是超时或连接断开；否则 `Fatal` |
| `PageCrashed` | `Retryable`：保留最近一次 URL，自动 `new_page` + 重新 `navigate` 后重试一次；若仍失败返回 `Fatal` |
| `ConnectFailed` | `Retryable` 一次；如果 session 完全断开，后续调用 `Fatal` |
| `NotAllowed` | `Fatal`（用户拒绝审批或 URL 被 denylist 拦截） |

### 内网访问限制

`browser__navigate` 默认禁止以下地址：`[C:INFERRED]`

- loopback：`127.0.0.0/8`、`::1`
- link-local：`169.254.0.0/16`、`fe80::/10`
- 私有 CIDR：`10.0.0.0/8`、`172.16.0.0/12`、`192.168.0.0/16`、`fc00::/7`

可配置 `allow_local_network=true` 覆盖（企业环境可强制关闭）。超过限制时返回 `blocked: true` 和 `block_reason`，不进入 guardian 审批。

## 工具层与下层接口

`BrowserThreadState` 由 Part 2 定义，提供以下方法供工具调用：`[C:INFERRED]`

```rust
impl BrowserThreadState {
    pub async fn navigate(&self, url: &str, wait_for: WaitCondition) -> Result<NavigationResult, BrowserControlError>;
    pub async fn screenshot(&self, full_page: bool) -> Result<ScreenshotResult, BrowserControlError>;
    pub async fn evaluate(&self, expression: &str) -> Result<EvaluateResult, BrowserControlError>;
    pub async fn click(&self, selector: Option<&str>, point: Option<Point>) -> Result<(), BrowserControlError>;
    pub async fn type_text(&self, selector: &str, text: &str, submit: bool) -> Result<(), BrowserControlError>;
    pub async fn get_dom(&self, selector: Option<&str>, max_depth: usize) -> Result<String, BrowserControlError>;
    pub async fn read_logs(&self, kind: LogKind, level: LogLevel, limit: usize) -> Result<LogsSnapshot, BrowserControlError>;
    pub async fn execute_raw_cdp(&self, method: &str, params: Value) -> Result<Value, BrowserControlError>;
}
```

## 测试策略

- **单元测试**：每个工具使用 mock `BrowserThreadState`，验证输入解析、输出格式、feature gate 过滤。
- **集成测试**：在 `ody-browser-control/tests` 中启动真实 Chrome，端到端测试 navigate + screenshot + evaluate。
- **审批测试**：在 `app-server` 扩展测试中验证写工具是否生成 `GuardianApprovalRequest::BrowserAction`。

## 未来扩展

- `browser__computer_move` / `browser__computer_scroll`：在 `ComputerUse` 启用时支持更细粒度的鼠标/键盘操作。
- `browser__download`：处理文件下载，保存到线程级临时目录。
- `browser__upload`：处理文件上传，需要从 Ody 文件系统读取文件。
