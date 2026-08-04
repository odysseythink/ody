# Browser Control 安全与可观测性

`ody-browser-control` 是 Ody 内置的浏览器自动化 crate，基于 `chromiumoxide` 提供 CDP 操作。它同时支持本地启动（Local）和连接外部 Chrome 调试端点（External）两种模式。本文档描述安全模型、审批流程、敏感操作拦截、网络日志脱敏以及可观测性约定。

## 安全模型：三层控制

1. **工具层（Tool Layer）**
   - 每个模型可见的 browser 工具在 `ody-browser-control/src/tools/mod.rs` 中实现。
   - 敏感操作（`navigate`、`evaluate`、`click`、`type`、`go_back`、`go_forward`、`reload`、`execute_raw_cdp`）必须先通过 guardian 审批门。
   - 只读操作（`screenshot`、`get_dom`、`read_logs`）不触发 guardian 审批。

2. **Guardian 审批层**
   - 工具层返回 `FunctionCallError::NeedsApproval`，并携带 `BrowserControlApprovalTicket`。
   - `app-server` 扩展层将 ticket 映射为 `GuardianApprovalRequest::BrowserAction` 并弹出用户审批。
   - 用户批准后，`guardian_approved_action_id` 被回写，工具层跳过审批门继续执行。

3. **线程/会话层**
   - 每个线程持有一个 `BrowserThreadState`，本地模式下每个会话对应一个独立 Chrome 进程和临时 profile。
   - 页面崩溃时自动重试一次；连接/启动/超时错误被标记为 `is_retryable`。

## 审批豁免规则（T01）

`browser__navigate` 在满足以下任一条件时自动放行，不弹 guardian 审批窗：

- 目标 host 是 loopback：`localhost`、`127.0.0.1`、`::1` 以及 `*.localhost`。
- `file://` URL 的路径位于当前线程 `cwd` 之下。
- `data:` URL 的完整字符串长度小于 1 KiB。
- 当前 crate 以 `cfg!(test)` 编译（仅用于单元测试，集成测试不会命中此路径）。

豁免路径仍会记录 `tracing::info!`：

```text
tool="browser__navigate", auto_approved=true, url_preview=..., reason=...
```

**注意**：豁免只跳过审批窗，目标 URL 仍会通过 `check_url_is_allowed` 进行网络范围校验（默认拒绝 `file:`、`javascript:`、私网地址，除非 `allow_local_network` 启用）。

## `browser__evaluate` 敏感表达式静态检查（T02）

`evaluate` 在 guardian 审批之前执行快速拒绝，拦截以下模式：

- `document.cookie`、`window.cookie` 以及任何 `cookie` + `document` 组合。
- `localStorage`、`sessionStorage`、`indexedDB` 等 Web Storage API。
- 常见混淆/间接执行：`eval(`、`function(`、`new function`、`atob(`、`setTimeout(`、`setInterval(`、`contentWindow`。

这是**第一层快速拒绝**，不能替代 guardian 审批。单元测试覆盖常见绕过写法。

对于过长的表达式，审批 ticket 中的 `expression` 会被截断到 500 字节，并设置 `expression_truncated = true`，防止日志和弹窗中泄漏完整脚本。

## Raw CDP 黑名单（T03）

`browser__execute_raw_cdp` 在以下情况下被禁止：

- 外部浏览器模式下完全禁用（`BrowserControlMode::External`）。
- 黑名单方法直接返回错误，不弹 guardian 审批窗。

黑名单包括：

- `Storage.getCookies`、`Storage.setCookies`、`Storage.deleteCookies`
- `Network.getAllCookies`、`Network.getCookies`
- `Fetch.continueRequest`、`Fetch.continueResponse`、`Fetch.fulfillRequest`、`Fetch.failRequest`
- 任何方法名包含 `getCookies` 子串（大小写不敏感）

安全方法（如 `Runtime.evaluate`、`DOM.querySelector`）仍需要 guardian 审批。

## 网络日志脱敏（T04）

`EventBuffer` 在 `snapshot()` 返回网络日志前执行 `redact_network_entry`：

- 响应 body 始终清空。
- 请求 body 超过 1024 字节时替换为 `[truncated]`。
- 敏感 header 统一替换为 `[REDACTED]`，覆盖：
  `Authorization`、`Cookie`、`Set-Cookie`、`X-Api-Key`、`X-Auth-Token`、
  `X-Csrf-Token`、`X-Requested-With`（大小写不敏感）。

`snapshot()` 同时记录 `tracing::info!` 摘要：

```text
console_entries=..., network_entries=..., total_bytes=...
```

## 可观测性（T05）

### instrument 约定

- `session.rs`：`launch`/`connect`/`close`/`new_page` 均带 `#[tracing::instrument]`。
- `thread_state.rs`：公开方法 `new`/`new_page`/`close`/`navigate`/`evaluate`/`execute_raw`/`click`/`type_text`/`screenshot`/`get_dom`/`read_logs`/`go_back`/`go_forward`/`reload` 均带 `#[tracing::instrument]`。
- `page_state.rs`：所有公开操作（页面生命周期、CDP 命令、DOM/日志读取）均带 `#[tracing::instrument]`。
- `tools/mod.rs`：每个 `ToolExecutor::handle` 均带 `#[tracing::instrument]`，字段包含 `tool_name`、`call_id`、`turn_id`、`model`、`approved`；不会输出完整 `call` 参数。

### 敏感字段截断

所有 `tracing` 字段只输出截断预览或元数据：

- URL 和 CSS 选择器截断到 120 字节（`truncate_string_bytes`）。
- JavaScript 表达式使用 `expression_preview`（200 字节截断）。
- raw CDP 参数只输出 `params_size`（JSON 序列化后字节数），不输出完整 JSON。
- 输入文本只输出长度（`text_size`）。
- 网络日志 summary 只输出条目数和总字节数。

### 关键事件

- `browser navigate auto-approved by exemption`：豁免自动放行。
- `browser tool requires guardian approval`：进入审批流程。
- `event buffer snapshot`：日志快照生成。
- 页面崩溃、handler 错误、profile 目录清理失败均有 `tracing::warn!`。

## 配置字段

`BrowserControlConfig` 中与安全相关的字段：

- `mode`：`Local`（本地启动）或 `External`（外部调试端点）。
- `headless`：是否无头运行；本地模式下影响 profile 隔离。
- `sandbox`：是否启用 Chrome OS 沙箱；本地测试常关闭。
- `allow_local_network`：是否允许导航到私网/loopback 地址。
- `external_browser_allow_sensitive`：外部模式下是否允许 `evaluate`/`click`/`type`（默认 false）。
- `disable_extensions`：禁止加载浏览器扩展。
- `max_event_entries` / `max_event_buffer_bytes` / `max_console_message_bytes`：事件缓冲区上限，防止日志无限增长。
- `command_timeout_ms` / `navigation_timeout_ms` / `launch_timeout_ms` / `connect_timeout_ms`：超时控制。

危险启动参数（`--user-data-dir`、`--remote-debugging-port`、`--proxy-server`、`--load-extension`）会被 `sanitize_args` 过滤。

## 测试

```bash
# 单元测试 + 当前 crate 的集成测试
cargo test -p ody-browser-control --tests

# 仅安全可观测性集成测试
cargo test -p ody-browser-control --test security_observability
```

- 集成测试中 `navigate` 的 loopback 豁免不依赖 `cfg!(test)`，而是真实命中豁免规则。
- 涉及真实 Chrome 的测试被 `#[ignore]`，仅在手动验证时运行。

## 不在本期的范围

- 外部 browser-use MCP connector 的自动去重。
- 企业级 URL allowlist（`allowed_url_patterns` / `blocked_url_patterns`）。
- 结构化 metrics，将在第二期以 tracing 为基础增强。
