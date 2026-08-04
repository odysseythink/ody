# Part 5 — 安全审批、错误处理与可观测性

## 设计原则

- ** fail-closed**：任何需要审批的操作，如果审批基础设施不可用，默认拒绝。
- **最小权限**：只暴露 AI 编程任务必需的浏览操作；`execute_raw_cdp` 必须显式 feature 开启。
- **线程隔离**：profile、cookie、storage、缓存随线程结束销毁，不污染用户主浏览器环境。
- **透明可观测**：所有 CDP 命令、浏览器进程生命周期、审批事件都有 tracing 记录。

## Guardian 审批模型

### 新增审批变体

在 `core/src/guardian/approval_request.rs` 的 `GuardianApprovalRequest` 枚举中新增 `BrowserAction`：`[C:UPSTREAM]`

```rust
pub(crate) enum GuardianApprovalRequest {
    // ... 现有变体 ...
    BrowserAction {
        id: String,
        turn_id: String,
        tool_name: String,
        action_kind: String, // 或 BrowserActionKind，序列化后为字符串
        url: Option<String>,          // navigate/reload 时提供
        script: Option<String>,       // evaluate 时提供（截断预览）
        selector: Option<String>,     // click/type 时提供
        raw_method: Option<String>,   // execute_raw_cdp 时提供
        raw_params: Option<serde_json::Value>,
    },
}
```

### 序列化与评估动作

在 `guardian_approval_request_to_json` 和 `guardian_assessment_action` 中增加 `BrowserAction` 分支：`[C:UPSTREAM]`

```rust
GuardianApprovalRequest::BrowserAction {
    tool_name,
    action_kind,
    url,
    script,
    selector,
    raw_method,
    raw_params,
    ..
} => {
    serialize_guardian_action(json!({
        "tool": tool_name,
        "action_kind": action_kind.as_str(),
        "url": url,
        "script_preview": script.as_ref().map(|s| truncate_text(s, 500)),
        "selector": selector,
        "raw_method": raw_method,
        "raw_params": raw_params,
    }))
}
```

评估动作（guardian assessment action）使用新的 `GuardianAssessmentAction::BrowserAction`：

```rust
pub(crate) enum GuardianAssessmentAction {
    // ... 现有变体 ...
    BrowserAction {
        tool_name: String,
        action_kind: BrowserActionKind,
        url: Option<String>,
        script_preview: Option<String>,
        selector: Option<String>,
        raw_method: Option<String>,
    },
}
```

### 审批流程

工具层调用审批的伪代码（已由 index 明确：扩展层负责实际 guardian 调用，工具层只产生 ticket）：`[C:INFERRED]`

```rust
async fn execute_with_browser_approval<T>(
    tool: &BrowserTool,
    input: &serde_json::Value,
    action: impl FnOnce() -> Future<Output = Result<T, BrowserControlError>>,
) -> Result<T, BrowserControlError> {
    if !tool.requires_approval() {
        return action().await;
    }

    let ticket = BrowserControlApprovalTicket {
        tool_name: tool.tool_name().to_string(),
        action_kind: tool.action_kind(),
        url: input.get("url").and_then(|v| v.as_str()).map(String::from),
        script: input.get("expression").and_then(|v| v.as_str()).map(|s| truncate_text(s, 500)),
        selector: input.get("selector").and_then(|v| v.as_str()).map(String::from),
        raw_method: input.get("method").and_then(|v| v.as_str()).map(String::from),
        raw_params: input.get("params").cloned(),
    };

    // 返回给调用方（app-server 扩展层），由扩展层转换为 GuardianApprovalRequest 并调用 review_approval_request
    Err(BrowserControlError::NeedsApproval(ticket))
}
```

> 扩展层审批通过后，再调用 `action().await` 执行实际操作。

### 审批豁免

以下情况可自动通过（但仍记录日志）：`[C:INFERRED]`

- `localhost` 或 `127.0.0.1` 的 URL（navigate）。
- `file://` 指向线程 cwd 下文件的 URL（navigate）。
- `data:` URI 且内容长度小于 1KB（navigate）。
- 内部测试 mock 模式（`#[cfg(test)]`）。

豁免规则需要在 guardian 评估前由工具层执行，避免滥用。更安全的做法是：即使豁免也生成 `guardian_auto_approved` 记录，但跳过用户弹窗。

## 敏感操作与只读操作

### 需要审批的操作

| 工具 | 原因 |
|---|---|
| `browser__navigate` | 可能访问外部网站、下载资源、改变网络状态；默认还受内网 denylist 限制 |
| `browser__evaluate` | 可执行任意 JS，读取 cookie/storage，发起网络请求；即使只读表达式也纳入审批 |
| `browser__click` | 可能触发导航、提交表单、执行破坏性操作 |
| `browser__type` | 可能输入敏感数据并提交 |
| `browser__execute_raw_cdp` | 任意 CDP 命令，能力上限最高 |
| `browser__reload` / `go_back` / `go_forward` | 状态变更 |

外部浏览器模式（`BrowserUseExternal`）下，`evaluate`/`click`/`type` 默认禁用，即使 `BrowserUse` 开启；只允许 `navigate`（需审批）、`screenshot`、`get_dom`、`read_logs`。`[C:INFERRED]`

### 不需要审批的操作

| 工具 | 原因 |
|---|---|
| `browser__screenshot` | 只读取当前页面视觉状态 |
| `browser__get_dom` | 只读取当前页面结构化内容 |
| `browser__read_logs` | 只读取已收集的 console/network 日志 |

## 安全审计清单

### Profile 隔离

- 每个线程启动时使用 `tempfile::TempDir::with_prefix("ody-browser-")` 作为 `user-data-dir` `[C:INFERRED]`。
- 强制覆盖 `chromiumoxide` 的默认 `user_data_dir`，避免读取用户默认 profile。
- 线程结束时 `TempDir` 自动删除，包含 Cookie、localStorage、IndexedDB、Cache、Service Worker。
- 配置项中不提供 `user_data_dir` 覆盖，防止用户/模型意外指向主 profile。

### 敏感数据读取防护

`browser__evaluate` 必须走 guardian 审批；在进入 CDP 前做静态检查快速拒绝：`[C:INFERRED]`

```rust
fn is_forbidden_expression(expr: &str) -> Option<&'static str> {
    let lower = expr.to_lowercase();
    // 静态检查仅作为第一层；绕过方式很多（eval、拼接、iframe.contentWindow、prototype 链），无法替代 guardian 审批。
    if lower.contains("document.cookie") || lower.contains("document.cookies")
        || (lower.contains("cookie") && lower.contains("document"))
    {
        return Some("reading document.cookie is not allowed");
    }
    if lower.contains("localstorage")
        || lower.contains("indexeddb")
        || lower.contains("sessionstorage")
    {
        return Some("reading web storage APIs is not allowed");
    }
    None
}
```

- `evaluate` 纳入 guardian 审批是主控；静态检查只是快速拒绝层。
- 外部浏览器模式下 `evaluate` 默认禁用（即使 `BrowserUse` 开启），除非 `external_browser_allow_sensitive=true`。
- 技术上无法完全阻止所有数据外泄路径（例如通过 fetch 把数据发送出去），因此审批机制是根本性控制。

### Raw CDP 限制

- `browser__execute_raw_cdp` 仅在 `BrowserUseFullCdpAccess` 启用且**本地浏览器模式**下暴露；外部浏览器模式下禁用。
- 企业配置可通过关闭该 feature 禁用此工具。
- 即使启用，每次调用仍需 guardian 审批。
- 额外配置黑名单：禁止调用 `Storage.getCookies`、`Network.getAllCookies`、`Fetch.continueRequest`、`*getCookies*` 等高度敏感方法；黑名单方法调用直接返回 `NotAllowed`，不进入 guardian 审批。

### 网络访问与 URL 限制

- `browser__navigate` 默认允许任意 URL；但 guardian 可基于企业策略拒绝。
- 未来可扩展 `allowed_url_patterns` / `blocked_url_patterns` 配置，在工具层直接拒绝。
- 本期不实现网络 allowlist，依赖 guardian 和企业 feature flag。

### 日志敏感信息

- console 和 network 日志可能包含用户输入、token、PII。
- 返回模型前，对敏感 header 做 redaction；network 事件只保留摘要，不保留请求/响应 body：`[C:INFERRED]`

```rust
const REDACTED_HEADERS: &[&str] = &[
    "authorization",
    "cookie",
    "set-cookie",
    "x-api-key",
    "x-auth-token",
    "x-csrf-token",
    "x-requested-with",
];

fn redact_network_entry(entry: &mut NetworkEntry) {
    // 1. 只保留 network 摘要，不保留 response body。
    entry.response_body = None;
    if entry.request_body.as_ref().map(|b| b.len()).unwrap_or(0) > 1024 {
        entry.request_body = Some("[truncated]".to_string());
    }
    // 2. 对敏感 header 进行 redaction。
    if let Some(headers) = entry.request_headers.as_mut() {
        for key in REDACTED_HEADERS {
            if let Some(v) = headers.get_mut(*key) {
                *v = "[REDACTED]".to_string();
            }
        }
    }
    if let Some(headers) = entry.response_headers.as_mut() {
        for key in REDACTED_HEADERS {
            if let Some(v) = headers.get_mut(*key) {
                *v = "[REDACTED]".to_string();
            }
        }
    }
}
```

- 默认不读取 network 请求/响应 body；如 future 需要，需额外 feature 和审批。
- console 消息中的 URL 如果是 `data:` 或包含 token，也做模糊化截断。
- 截断过长的 console 消息，避免塞满模型上下文。

## 错误处理与降级

### 重试策略

`BrowserControlError` 到重试行为的映射：`[C:INFERRED]`

| 错误 | 是否重试 | 重试次数 | 说明 |
|---|---|---|---|
| `CommandFailed` 因超时 | 是 | 2 次 | 每次间隔 1s，可能页面加载过慢 |
| `PageCrashed` | 是 | 1 次 | 重新 `new_page` 并保留最近一次 URL 自动重新导航 |
| `ConnectFailed` | 否 | — | 连接失败说明进程已崩溃或配置错误 |
| `LaunchFailed` / `QuotaExceeded` | 否 | — | 避免反复启动失败进程；配额不足时等待或失败 |
| `ChromeNotFound` | 否 | — | 配置/环境问题 |
| `NotAllowed` | 否 | — | 用户已拒绝或审批取消 |

重试逻辑封装在 `BrowserThreadState` 的方法中，对工具层透明。

### 页面崩溃恢复

```rust
impl BrowserThreadState {
    async fn with_page_recovery<T>(
        &self,
        action: impl FnOnce(&PageState) -> Future<Output = Result<T, BrowserControlError>>,
    ) -> Result<T, BrowserControlError> {
        let current = self.current_page.lock().await;
        match action(&current).await {
            Ok(v) => Ok(v),
            Err(BrowserControlError::PageCrashed) => {
                tracing::warn!("page crashed, creating new page and restoring last url");
                let last_url = current.last_url().clone();
                drop(current);
                let new_page = self.session.new_page().await?;
                if let Some(url) = last_url {
                    new_page.navigate(&url).await
                        .map_err(BrowserControlError::CommandFailed)?;
                }
                *self.current_page.lock().await = new_page;
                action(&self.current_page.lock().await).await
            }
            Err(e) => Err(e),
        }
    }
}
```

### 连接断开处理

- 如果 `handler_task` 因 WebSocket 断开而退出，工具调用应检测到 `ConnectFailed`。
- 由于线程级隔离，一个线程的浏览器进程崩溃不影响其他线程。
- 当前线程后续工具调用会失败，模型可收到错误提示并决定是否重新启动线程（新线程会在 start 时重新启动浏览器）。

## 可观测性

### Tracing Spans

在 `ody-browser-control` 中统一使用 `tracing`：`[C:INFERRED]`

```rust
#[tracing::instrument(skip(self), fields(tool = "browser__navigate", url = %url))]
pub async fn navigate(&self, url: &str) -> Result<(), BrowserControlError> {
    // ...
}

#[tracing::instrument(skip(self), fields(tool = "browser__screenshot"))]
pub async fn screenshot(&self, full_page: bool) -> Result<Vec<u8>, BrowserControlError> {
    // ...
}
```

关键事件：`[C:INFERRED]`

```rust
tracing::info!(chrome_path = ?path, "chrome executable detected");
tracing::info!(profile_dir = ?dir, "browser profile created");
tracing::info!("browser process launched");
tracing::info!("browser process connected");
tracing::warn!(error = ?e, "cdp command failed, retrying");
tracing::error!(error = ?e, "browser session unrecoverable");
tracing::info!(tool = %tool_name, approved = %approved, "browser action approval result");
```

### 指标（可选增强）

如果后续需要监控，可增加以下指标：`[C:INFERRED]`

- `ody.browser.launch.count` / `status`
- `ody.browser.command.duration_ms` / `method`
- `ody.browser.approval.count` / `tool_name` / `decision`
- `ody.browser.process.count`（活跃 Chrome 进程数）

本期以 tracing 为主，metrics 在第二部分增强。

### 日志保密

- tracing event 中不输出完整的 `script` 或 `raw_params`，只输出 truncated preview。
- screenshot 数据不进入 tracing（只记录大小）。
- network log 在 tracing 中保持 redacted。

## 安全审计检查表

实现完成后必须逐项验证：`[C:INFERRED]`

- [ ] `user-data-dir` 始终指向 `tempfile::TempDir` 创建的目录，绝不指向用户默认 profile。
- [ ] 线程结束时 `TempDir` 被清理，无残留 Chrome 进程和 profile 目录。
- [ ] `browser__execute_raw_cdp` 在 `BrowserUseFullCdpAccess` 关闭时不可见。
- [ ] `browser__navigate`/`evaluate`/`click`/`type` 在非豁免场景下触发 guardian 审批。
- [ ] `browser__evaluate` 必须走 guardian 审批；外部浏览器模式下默认禁用。
- [ ] `browser__evaluate` 的 `document.cookie`/`localStorage`/`indexedDB` 静态检查生效。
- [ ] network log 中的 `Authorization`/`Cookie`/`Set-Cookie`/`X-Api-Key`/`X-Auth-Token` 被 redacted。
- [ ] network log 不返回请求/响应 body。
- [ ] 外部浏览器连接（`BrowserUseExternal`）关闭时无法连接非本进程 Chrome。
- [ ] 所有审批拒绝/超时/取消都返回清晰错误，不暴露内部路径或命令细节。

## 测试策略

- **安全单元测试**：
  - `is_forbidden_expression` 对各类敏感表达式返回正确拒绝原因。
  - `redact_network_entry` 对敏感 header 进行 redaction。
  - `tools()` 在 `BrowserUseFullCdpAccess` 关闭时不包含 `execute_raw_cdp`。
- **审批集成测试**：
  - mock guardian 返回 `Approved`，验证工具执行成功。
  - mock guardian 返回 `Rejected`，验证工具返回 `NotAllowed` 错误。
- **profile 隔离测试**：
  - 在一个线程中设置 localStorage，结束线程后在新线程中读取同一 URL 的 localStorage，验证为空。
- **进程清理测试**：
  - 创建 100 个线程并关闭，验证无僵尸 Chrome 进程。

## 与外部 Browser Use MCP 的冲突避免

代码中已存在 `ODY_APPS_MCP_SERVER_NAME` 和 `browser-use` connector 的测试夹具。本内置 crate 上线后需明确分工：`[C:INFERRED]`

- 内置 `ody-browser-control`：服务本地/headless 浏览器，工具名 `browser__*`。
- 外部 browser-use MCP connector：服务用户已打开的外部浏览器，工具名由 MCP server 定义（通常也是 `browser_*` 或 `computer_*`）。
- 当两者同时启用时，模型可能看到两套工具。建议通过配置文档引导用户选择其一；本期不实现自动去重。

## 文档交付

- `docs/browser-control.md`：功能启用方式、配置示例、安全注意事项、已知限制。
- 更新 `AGENTS.md`：在相关工具章节说明 `browser__*` 工具的行为和审批要求。
- 企业管理员文档：如何关闭 `BrowserUseFullCdpAccess`、如何配置 URL allowlist（未来）。
