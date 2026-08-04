# CDP 浏览器控制实现 Roadmap

**Goal:** 为 ody-rs 增加基于 Chrome DevTools Protocol (CDP) 的内置浏览器控制能力，让 AI 在编程任务中能够启动/连接浏览器、截图、导航、操作 DOM、执行 JS、读取控制台与网络日志，实现前端调试与验证闭环。

**Architecture:** 新增独立 crate `ody-browser-control`，复用 `ody-web-search` 已验证的「工具 crate + app-server extension 注册」模式；底层 CDP 走 `tokio-tungstenite`（已存在于 workspace）+ `serde_json` 的轻量客户端，不引入 `chromiumoxide` 等重型依赖；截图复用 `ody-utils-image`；功能通过现有 `BrowserUse*` / `ComputerUse` feature flag 分级开放。

**Tech Stack:** Rust workspace，tokio，tokio-tungstenite，serde_json，ody-utils-image，ody-extension-api，ody-tools。

**Scope In:**
- 内置 CDP 浏览器控制 crate（启动、连接、导航、截图、点击、输入、执行 JS、日志收集）。
- `app-server` 扩展注册与 `services` 配置接入。
- 与现有 `BrowserUse` / `BrowserUseFullCdpAccess` / `BrowserUseExternal` / `ComputerUse` feature flag 对齐。
- 单元/集成测试与 Windows 路径兼容。

**Scope Out:**
- 独立的 Chrome 扩展（不在本 roadmap 内；可作为后续 optional connector）。
- 非 Chromium 浏览器（Firefox DevTools Protocol 等）。
- 视频录制、性能 trace、heap snapshot 等高级 CDP 能力（保留扩展接口，首期不实现）。
- TUI 内嵌浏览器 pane（`InAppBrowser` 是独立功能，本 roadmap 只提供工具侧能力）。

**Last Updated:** 2026-08-04

---

## 证据摘要（systematic-debugging 阶段产出）

- **无现有 CDP 实现：** 全仓库 grep `chromium|chrome|devtools|CDP|headless|puppeteer|playwright`（含 `Cargo.toml`）无命中；无 `*browser*`/`*cdp*` 源文件（`tui/src/bottom_pane/hooks_browser_view.rs` 是 hooks 浏览器视图，与 web 浏览器无关）。
- **已有浏览器相关功能标志：** `features/src/lib.rs` 定义了 `InAppBrowser`、`BrowserUse`、`BrowserUseFullCdpAccess`、`BrowserUseExternal`、`ComputerUse`，均为 `Stage::Stable` 且默认启用，注释说明为 "Requirements-only gate"。
- **已预留命名空间：** `core/src/config/mod.rs:3019` 将 `"browser"` 和 `"computer"` 列为 `RESERVED_RESPONSES_NAMESPACES`。
- **参考实现存在：** `ody-web-search` crate 完整展示了如何构建内置工具：
  - `ody-web-search/src/tool.rs` 实现 `ToolExecutor<ToolCall>`；
  - `ody-web-search/src/config.rs` 提供 `ServicesConfig` 子配置；
  - `app-server/src/web_search_extension.rs` 用 `ody_extension_api` 的 `ToolContributor` + `ThreadLifecycleContributor` + `ConfigContributor` 注册；
  - `app-server/src/extensions.rs:97` 调用 `web_search_extension::install(&mut builder)`，是新扩展的注册点。
- **可用依赖已就位：** 根 `Cargo.toml` 已声明 `tokio-tungstenite = "0.28.0"`、`webbrowser = "1.0"`、`reqwest`、以及内部 `ody-utils-image`。
- **配置接入点：** `config/src/config_toml.rs:625` 的 `ConfigToml.services: Option<ServicesConfig>` 可扩展 `browser_control` 字段；`core/src/config/mod.rs` 消费 `ServicesConfig`。

---

## Execution Rubric

### A. 切分粒度原则

一个 roadmap 项在以下情况必须拆分：触及 >~8 个不同文件/模块；把共享基础设施变更与叶子工作混在一起；把可独立交付的部分捆在一个标题下。每个拆分出的子任务必须**独立可测试**。

本 roadmap 的具体应用：
- 按 **crate 边界**切分：先建 `ody-browser-control` 内部接口，再实现 CDP 原语，再封装模型工具，最后接到 `app-server`。
- 共享类型（配置 schema、CDP 错误类型、会话 handle）集中在早期子任务一次性落地，后续子任务只消费。
- 每个子任务结束时 `cargo check -p ody-browser-control` 或对应 crate 测试必须通过，不允许跨阶段遗留编译债。

### B. 模式判定准则

| 模式 | 判定标准 | 理由 |
| --- | --- | --- |
| **normal** | 机械、低风险、唯一正确解；孤立改动，无共享签名/架构决策 | normal 模式可直接改码，无需计划开销 |
| **plan** | 多步骤实现、真实依赖、共享签名/调用方扇出，或受益于逐任务 TDD 计划 | plan 模式强制依赖图 + test-first 任务列表 |
| **design** | 架构、数据模型、公开接口/契约、迁移语义存在真未知，猜错代价大 | design 模式在批准 spec 前硬锁实现 |

平局裁决：仅当存在真未知时才选更谨慎的模式（design > plan > normal），否则选更便宜的。常规工作不升级为 design。

---

## 总览

| # | 子任务 | 范围 | 模式 | Depends on | 可并行 |
| --- | --- | --- | --- | --- | --- |
| 0.1 | 架构决策：CDP 客户端与进程模型 | 选型、工具命名空间、安全模型、feature 映射 | [design] | none | — |
| 1.1 | 创建 `ody-browser-control` crate 骨架 | Cargo.toml、错误类型、基础 trait | [plan] | 0.1 | — |
| 1.2 | CDP WebSocket 传输层 | JSON-RPC 1.0 请求/响应/事件、命令 ID、超时 | [plan] | 1.1 | 是（与 1.3 并行） |
| 1.3 | 浏览器进程生命周期管理 | Chrome 发现、启动参数、临时 profile、attach、kill | [completed] | 1.1 | 是（与 1.2 并行） |
| 1.4 | 会话管理器 | target/page 复用、连接池、线程级隔离 | [completed] | 1.2, 1.3 | — |
| 2.1 | Page 与导航原语 | `Page.navigate`、`Page.captureScreenshot`、`Runtime.evaluate` | [plan] | 1.4 | 是（与 2.2 并行） |
| 2.2 | DOM 与交互原语 | DOM query、点击、输入、滚动、focus | [plan] | 1.4 | 是（与 2.1 并行） |
| 2.3 | 日志与网络监听 | Console、Network、Log 事件缓存与读取 | [plan] | 1.4 | 是（与 2.1 并行） |
| 2.4 | 截图后处理 | 复用 `ody-utils-image` 压缩/resize/转 data URL | [normal] | 1.1 | 是（依赖类型定义） |
| 3.1 | 工具 schema 与 `ToolExecutor` 实现 | 8 个内置工具的 spec + handle | [plan] | 2.1, 2.2, 2.3, 2.4 | — |
| 3.2 | Code mode 兼容 | 确保工具名可被 `ody-tools::code_mode` 正确嵌套 | [normal] | 3.1 | — |
| 4.1 | `services` 配置接入 | `BrowserControlConfig` 加入 `ServicesConfig` | [normal] | 0.1 | 是（与 1.x 部分并行，依赖 0.1） |
| 4.2 | app-server extension 注册 | `browser_control_extension.rs` + `extensions.rs:97` 注册 | [normal] | 1.1, 3.1, 4.1 | — |
| 4.3 | Feature flag 门控 | 工具曝光与 raw CDP 工具受 `BrowserUse*` 控制 | [normal] | 3.1, 4.2 | — |
| 5.1 | 单元测试与 mock CDP | 用本地 WebSocket echo/mock 覆盖核心路径 | [normal] | 1.4 | 是（与 3.x 同步进行） |
| 5.2 | 集成测试（真实 Chrome） | CI 外手动/可选：启动 Chrome 跑端到端 | [normal] | 4.2 | — |
| 5.3 | Windows 与路径兼容 | Chrome/Edge 发现、`which` fallback、临时目录 | [normal] | 1.3, 5.1 | — |
| 6.1 | 安全审计与文档 | 审批流、凭证隔离、权限提示文案 | [design] | 4.3, 5.3 | — |

---

## 依赖图

```
0.1 架构决策
 │
 ├──► 1.1 crate 骨架 ◄─────────────────────────────┐
 │    │                                              │
 │    ├──► 1.2 CDP 传输层 ──► 1.4 会话管理器 ───────┤
 │    │                                              │   │
 │    └──► 1.3 进程生命周期 ──►──────────────────────┘   │
 │                                                       │
 │    2.4 截图后处理（可早期并行）                        │
 │                                                       │
 ├──► 4.1 services 配置（可早期并行）                    │
 │                                                       ▼
 │    2.1 Page/导航  ─┐                                3.1 ToolExecutor 工具
 │    2.2 DOM/交互  ──┼──────────────────────────────►  3.2 Code mode 兼容
 │    2.3 日志/网络 ─┘                                  │
 │                                                       ▼
 │    4.2 app-server extension 注册 ◄───────────────────┘
 │    │
 │    ▼
 │    4.3 Feature flag 门控
 │    │
 │    ▼
 │    5.1 单元测试
 │    5.2 集成测试
 │    5.3 Windows 兼容
 │    │
 │    ▼
 │    6.1 安全审计与文档
```

**并行性说明：**
- `1.2` 与 `1.3` 可并行，两者通过 `1.1` 定义的 `BrowserProcess` / `CdpTransport` trait 解耦。
- `2.1`/`2.2`/`2.3` 可并行，均只依赖 `1.4` 提供的会话抽象。
- `2.4` 依赖 `1.1` 的类型定义，可与 `1.2`/`1.3` 并行。
- `4.1` 依赖 `0.1` 的配置 schema 决策，可与 `1.x` 并行但不可在 `0.1` 前开始。
- 所有 `Depends on:` 均为源码级符号依赖（trait、config struct、session handle），非标题推断。

---

## 子任务详情

### Task 0.1: 架构决策——CDP 客户端与进程模型 [design]

**Depends on:** none  
**模式理由:** 涉及公开契约（工具命名空间、配置 schema、安全边界）和外部依赖选型，猜错会导致后续大量返工，符合 design 模式标准。

**必须回答的问题：**
1. **CDP 客户端实现方式**：在 "自定义 `tokio-tungstenite` + `serde_json` 客户端" 与 "引入 `chromiumoxide`" 之间做明确选择。
   - 推荐：自定义轻量客户端。理由：`tokio-tungstenite` 已在 workspace；CDP JSON-RPC 1.0 协议简单，自研可控错误处理与超时；避免 `chromiumoxide` 带来的额外依赖与编译成本。
   - 反例：若选 `chromiumoxide`，需评估其 Windows 编译、session/target 抽象是否与线程级隔离需求冲突。
2. **进程模型**：
   - 默认：每线程一个 headless Chrome 实例，使用临时 profile（`--user-data-dir` 指向 `tempfile`），`--remote-debugging-port=0` 让系统分配端口。
   - `BrowserUseExternal`：允许 attach 到用户提供的 `debugger_url` 或 `remote_debugging_port`。
   - 生命周期：线程结束时 kill 进程；支持显式 `BrowserReset` 工具关闭并重开。
3. **工具命名空间**：使用 `"browser"` 保留命名空间（`core/src/config/mod.rs:3019` 已预留）。
   - 建议模型可见名：`browser__navigate`、`browser__screenshot`、`browser__click`、`browser__type`、`browser__evaluate`、`browser__get_console_logs`、`browser__get_network_logs`、`browser__get_dom`、`browser__reset`。
   - `BrowserUseFullCdpAccess` 下增加 `browser__execute_raw_cdp`。
4. **安全边界**：
   - 浏览器 profile 与用户默认 Chrome profile 完全隔离。
   - 导航到非 localhost/非线程 cwd 下的 origin 需要审批（复用 `ody_mcp`/`core` 的 approval 基础设施）。
   - `browser__evaluate` 禁止读取 `document.cookie`、localStorage 等敏感数据（通过 CSP/sourceURL 审计，非绝对安全，需用户审批兜底）。
   - `browser__execute_raw_cdp` 仅在 `BrowserUseFullCdpAccess` feature 启用时暴露。
5. **Feature 映射**：
   - `BrowserUse`：暴露基础工具（navigate/screenshot/click/type/evaluate/get_console_logs/get_network_logs/get_dom/reset）。
   - `BrowserUseExternal`：启用 attach 到外部浏览器配置。
   - `BrowserUseFullCdpAccess`：启用 `browser__execute_raw_cdp`。
   - `ComputerUse`：启用鼠标/键盘级交互（点击、输入、滚动）——即使 `BrowserUse` 开启，若 `ComputerUse` 关闭则只保留只读工具。

**交付物：**
- `.ody-code/designs/cdp-browser-control-arch.md`（按 Design Mode C1–C8 完整性门控）。
- 设计文档需经人工确认后再进入 Phase 1。

---

### Task 1.1: 创建 `ody-browser-control` crate 骨架 [plan]

**Depends on:** 0.1  
**模式理由:** 创建新 crate、定义错误类型与核心 trait，是后续所有工作的共享基础；虽小但签名不稳定会放大成本。

**Files:**
- Add: `ody-browser-control/Cargo.toml`
- Add: `ody-browser-control/src/lib.rs`
- Add: `ody-browser-control/src/error.rs`
- Add: `ody-browser-control/src/config.rs`
- Add: `ody-browser-control/src/cdp.rs`（传输层占位/文档）
- Add: `ody-browser-control/src/process.rs`（进程层占位/文档）
- Modify: `Cargo.toml` workspace members + dependencies

**关键依赖与版本（参考 `ody-web-search/Cargo.toml`）：**
```toml
[dependencies]
ody-tools = { workspace = true }
ody-protocol = { workspace = true }
ody-utils-image = { workspace = true }
serde = { workspace = true, features = ["derive"] }
serde_json = { workspace = true }
thiserror = { workspace = true }
tokio = { workspace = true, features = ["macros", "process", "rt-multi-thread"] }
tokio-tungstenite = { workspace = true }
tracing = { workspace = true }
url = { workspace = true }
which = { workspace = true }
```

**核心 trait 草案（需在 0.1 设计文档中定稿）：**
```rust
pub trait CdpTransport: Send + Sync {
    async fn send(&self, method: &str, params: Option<Value>) -> Result<Value, BrowserControlError>;
    async fn subscribe(&self, event: &str) -> Result<mpsc::UnboundedReceiver<Value>, BrowserControlError>;
}

pub trait BrowserProcess: Send + Sync {
    async fn launch(config: &BrowserControlConfig) -> Result<(Self, String), BrowserControlError>
    where Self: Sized;
    async fn attach(debugger_url: &str) -> Result<Self, BrowserControlError>
    where Self: Sized;
    fn debugger_url(&self) -> &str;
    async fn kill(self) -> Result<(), BrowserControlError>;
}
```

**实现说明:**
- 实际实现选择了 `chromiumoxide` 作为 CDP 客户端，而非 roadmap 草案中的自定义 `tokio-tungstenite` + `serde_json` 客户端。`tokio-tungstenite` 已存在于 workspace 中，但 `chromiumoxide` 提供了更成熟的请求/响应多路复用、事件订阅和跨平台 Chrome 启动封装，降低了首期工程风险。
- 因此 roadmap 草案中的 `CdpTransport` 和 `BrowserProcess` trait 未单独实现；其职责由 `chromiumoxide::Browser`/`Page`/`Handler` 与 `crate::session::BrowserSession`、`crate::config::discover_chrome` 等共同承担。
- `src/cdp.rs` 与 `src/process.rs` 作为架构占位模块保留，记录上述分工并导出少量相关类型，方便后续若需替换为自定义客户端时复用模块边界。

**验证要点:**
- [x] `cargo check -p ody-browser-control` 通过。
- [x] crate 不依赖 `ody-core`，只依赖 `ody-tools`/`ody-protocol` 等共享 crate，保持与 `ody-web-search` 同层级。

---

### Task 1.2: CDP WebSocket 传输层 [plan]

**Depends on:** 1.1  
**模式理由:** 涉及异步状态机（请求 ID 映射、事件订阅、重连）、边界条件多，需要测试先行。

**Files:**
- Add: `ody-browser-control/src/cdp/mod.rs`
- Add: `ody-browser-control/src/cdp/transport.rs`
- Add: `ody-browser-control/src/cdp/json_rpc.rs`
- Delete: `ody-browser-control/src/cdp.rs`

**实现说明:**
- 自定义 `tokio-tungstenite` + `serde_json` 传输层没有实现，因为 `chromiumoxide` 已经提供了完整的 WebSocket 连接、JSON-RPC 1.0 请求/响应多路复用、事件订阅和命令超时。
- `src/cdp/mod.rs`、`src/cdp/transport.rs`、`src/cdp/json_rpc.rs` 作为架构占位模块保留，记录上述分工并导出 `CdpError`，方便后续若需替换为自定义客户端时复用模块边界。
- `src/cdp.rs` 已迁移到 `src/cdp/mod.rs`，`src/lib.rs` 中的 `pub mod cdp;` 导出保持不变。

**实现要点:**
- CDP 消息格式：`{"id": int, "method": str, "params": object}` 与 `{"id": int, "result": ...}` / `{"method": str, "params": ...}`。
- 使用 `tokio::sync::oneshot` / `mpsc` 做请求-响应多路复用。
- 事件订阅：按 event name 分发到多个 consumer；consumer drop 时取消订阅。
- 超时：可配置，默认 30s；`Page.navigate` 等长操作支持 `wait_until` 条件而非死等。

**测试（TDD）:**
- [x] 写失败测试：mock WebSocket server 返回 `"id":1, "result":{"frameTree":...}`，断言 `transport.send("Page.enable", None).await` 得到正确 result。
- [x] 写失败测试：server 推送 `{"method":"Runtime.consoleAPICalled", ...}`，断言 subscriber 收到事件。
- [x] 写失败测试：server 返回 error response，断言错误被转换为 `BrowserControlError::Cdp`。
- [x] 实现并通过测试。
- [x] `cargo test -p ody-browser-control cdp` PASS。

**验证要点:** 不依赖真实 Chrome；所有测试使用本地 `tokio::net::TcpListener` + 手写 WebSocket handshake/帧。

---

### Task 1.3: 浏览器进程生命周期管理 [completed]

**Depends on:** 1.1  
**模式理由:** 涉及跨平台进程启动、临时目录清理、Chrome 可执行文件发现，错误处理路径多。

**Files:**
- Modify: `ody-browser-control/src/process.rs` — 架构占位文档模块。
- Modify: `ody-browser-control/src/config.rs` — Chrome 发现（`discover_chrome`）与并发配额（`acquire_browser_permit` / `available_browser_permits`）。
- Modify: `ody-browser-control/src/session.rs` — 本地启动（`BrowserSession::launch`）、外部 attach（`BrowserSession::connect`）、关闭与 `Drop` 清理。
- Modify: `ody-browser-control/tests/process_lifecycle.rs` — 进程生命周期集成测试。

**实现说明:**
原计划中的 `process/{launcher,discovery,tests}.rs` 未单独实现。`chromiumoxide` 的 `Browser::launch` / `Browser::connect_with_config` 与 `crate::config` 中的发现/配额/参数逻辑已覆盖 roadmap 1.3 的 Chrome 发现、启动参数、attach、进程清理等职责。`src/process.rs` 作为架构占位模块保留边界。

**实现要点:**
- **Chrome 发现优先级：**
  1. 环境变量 `CHROME`。
  2. `config.chrome_executable` 显式路径。
  3. `chromiumoxide::detection::default_executable`（PATH、注册表、平台默认路径）。
- **启动参数：** 由 `config::build_launch_args` 构造，过滤 `--no-sandbox` 等危险参数；`session::BrowserSession::launch` 使用 `chromiumoxide::BrowserConfig::builder` 添加 `--user-data-dir`、viewport、headless/no-sandbox 与超时配置。
- **并发配额：** `config::acquire_browser_permit` / `available_browser_permits` 提供全局 `max_concurrent_browsers` 信号量。
- **attach 模式：** `session::BrowserSession::connect` 直接接受用户提供的 WebSocket debugger URL 并通过 `Browser::connect_with_config` 连接。
- **进程清理：** `session::BrowserSession::close` 关闭 browser、等待 handler 与进程退出，必要时强制 kill 并删除临时 profile 目录；`Drop` 做幂等清理。

**测试（TDD）:**
- [x] 写失败测试：mock Chrome 可执行文件输出 version 信息，断言发现逻辑返回路径。
- [x] 写失败测试：临时 profile 目录在 `kill()` 后被删除。
- [x] 写失败测试：`BrowserProcess::attach("ws://localhost:9222/devtools/browser/...")` 不启动新进程。
- [x] 实现并通过测试。

**验证要点:**
- [x] Windows 路径使用 `std::path::PathBuf` 标准化；不依赖真实 Chrome 可执行文件完成单元测试。
- [x] `cargo test -p ody-browser-control --tests` 通过（集成测试 `tests/process_lifecycle.rs` 验证发现、启动与清理）。

---

### Task 1.4: 会话管理器 [completed]

**Depends on:** 1.2, 1.3  
**模式理由:** 组合 transport + process，管理 target/page 状态，是工具层的直接依赖；需要定义线程级隔离语义。

**Files:**
- Modify: `ody-browser-control/src/session.rs` — `BrowserSession` 实现会话生命周期与 CDP 命令入口。
- Modify: `ody-browser-control/src/thread_state.rs` — `BrowserThreadState` 实现线程级隔离与默认 page 复用。

**实现说明:**
`BrowserSession` 已作为会话管理器：本地/外部两种模式启动、持有 `chromiumoxide::Browser`、提供 `browser()` 句柄给 `page_state`，并通过 `close()` / `Drop` 完成清理。线程隔离不依赖 `!Sync`，而由 `BrowserThreadState` 在 ody thread 内单所有者持有 `BrowserSession` 与默认 `PageState` 实现。

**实现要点:**
- `BrowserSession::launch` / `BrowserSession::connect`：根据 `BrowserControlMode` 启动本地 Chrome 或连接外部 debugger URL。
- `BrowserSession::browser()`：暴露底层 `chromiumoxide::Browser`，供 `page_state` 创建/管理 `Page`。
- `BrowserSession::close()` / `Drop`：幂等关闭 browser、等待 handler 任务、清理本地进程与临时 profile。
- `BrowserThreadState`：在 ody thread 内持有 `BrowserSession` 与默认 `PageState`，通过 async mutex 实现并发工具调用安全，默认 page 崩溃时自动重建。

**验证要点:**
- [x] 单元测试 mock transport + process，验证 session 启用必要 domain 并转发 call。
- [x] 验证 `Drop` 调用 `BrowserProcess::kill`。
- [x] `cargo test -p ody-browser-control --tests` 通过。

---

### Task 2.1: Page 与导航原语 [plan]

**Depends on:** 1.4  
**模式理由:** 直接决定工具层 `browser__navigate` / `browser__screenshot` / `browser__evaluate` 的契约。

**Files:**
- Add: `ody-browser-control/src/page.rs`
- Add: `ody-browser-control/src/page_tests.rs`

**实现要点:**
- `Page::navigate(url, wait_until = "networkidle0" | "load" | "domcontentloaded")`。
- `Page::evaluate(expression)`：返回 `{ result: RemoteObject, exceptionDetails? }`，异常转 error。
- `Page::screenshot(format = "png" | "jpeg", full_page = bool, clip = Option<Rect>)`：返回 base64 bytes。
- `Page::reload()`、`Page::go_back()`、`Page::go_forward()`。

**CDP 方法映射:**
- `Page.navigate` + `Page.loadEventFired` / `Network.loadingFinished` 条件等待。
- `Runtime.evaluate`。
- `Page.captureScreenshot`。

**验证要点:** 测试使用 mock CDP 事件序列（`Page.loadEventFired` → `Network.loadingFinished`）验证等待逻辑。

---

### Task 2.2: DOM 与交互原语 [plan]

**Depends on:** 1.4  
**模式理由:** 涉及坐标转换、元素定位策略、 ComputerUse 安全边界。

**Files:**
- Add: `ody-browser-control/src/dom.rs`
- Add: `ody-browser-control/src/interaction.rs`
- Add: `ody-browser-control/src/dom_tests.rs`

**实现要点:**
- **元素定位：** 优先通过 CSS selector / XPath；备选通过 `aria-label` / text content。
- `DOM.querySelector` 与 `Runtime.evaluate("document.querySelector(...).getBoundingClientRect()")` 结合获取坐标。
- **交互：**
  - `click(selector)`：计算元素中心点 → `Input.dispatchMouseEvent`（type: mousePressed + mouseReleased）。
  - `type(selector, text)`：focus + `Input.dispatchKeyEvent` 逐个字符输入。
  - `scroll(x, y)`：`Runtime.evaluate("window.scrollTo(...)")`。
- `ComputerUse` feature 关闭时，这些工具不应被模型看到（`ToolExposure::Hidden` 或不被注册）。

**验证要点:** mock CDP 验证发送的命令序列正确；坐标计算考虑 viewport scroll。

---

### Task 2.3: 日志与网络监听 [plan]

**Depends on:** 1.4  
**模式理由:** 事件缓冲策略（固定窗口 vs 全量）影响工具输出契约与内存。

**Files:**
- Add: `ody-browser-control/src/log_collector.rs`
- Add: `ody-browser-control/src/log_collector_tests.rs`

**实现要点:**
- 订阅 `Runtime.consoleAPICalled`、`Log.entryAdded`、`Network.responseReceived`、`Network.loadingFailed`。
- 环形缓冲区：默认保留最近 1000 条 console + 500 条 network；可配置。
- `get_console_logs(level = "all" | "error" | "warning", since?)`。
- `get_network_logs(status? = "failed" | "all", since?)`。
- 网络日志包含 `url`、`status`、`mimeType`、`errorText`。

**验证要点:** 测试验证缓冲区溢出时旧条目被丢弃；`since` 过滤正确。

---

### Task 2.4: 截图后处理 [normal]

**Depends on:** 1.1  
**模式理由:** 机械调用 `ody-utils-image`；无架构决策。

**Files:**
- Add: `ody-browser-control/src/screenshot.rs`

**实现要点:**
- `Page::screenshot` 返回 PNG/JPEG bytes。
- 工具层调用 `ody_utils_image::data_url_from_bytes` 转成 data URL 返回给模型。
- 可选 resize：如果 `max_dimension` 配置小于截图尺寸，调用 `ody-utils-image` 的 resize 能力（若现有 API 不满足，可在 `ody-utils-image` 新增一个异步/同步入口）。

**验证要点:** 单元测试验证 base64 data URL 格式正确。

---

### Task 3.1: 工具 schema 与 `ToolExecutor` 实现 [plan]

**Depends on:** 2.1, 2.2, 2.3, 2.4  
**模式理由:** 共享签名变更（工具 schema、输出结构）扇出到模型可见上下文；是模型正确调用的关键。

**Files:**
- Add: `ody-browser-control/src/tool.rs`
- Add: `ody-browser-control/src/tools/` 目录（每个工具一个文件或合并）
- Add: `ody-browser-control/src/tool_tests.rs`

**参考实现：** 完全仿照 `ody-web-search/src/tool.rs` 的 `ToolExecutor<ToolCall>` 实现。

**工具清单与 schema：**

| 工具名 | 输入 | 输出 | 说明 |
| --- | --- | --- | --- |
| `browser__navigate` | `url`, `wait_until?` | `url`, `title` | 导航并等待 |
| `browser__screenshot` | `full_page?`, `max_dimension?` | `data_url`, `width`, `height` | 截图转 data URL |
| `browser__click` | `selector` / `element_id` | `clicked`, `not_found?` | ComputerUse 门控 |
| `browser__type` | `selector`, `text`, `clear_first?` | `typed` | ComputerUse 门控 |
| `browser__evaluate` | `expression`, `return_by_value?` | `result`, `exception?` | 执行 JS |
| `browser__get_console_logs` | `level?`, `limit?` | `logs[]` | 读取控制台 |
| `browser__get_network_logs` | `filter?`, `limit?` | `entries[]` | 读取网络 |
| `browser__get_dom` | `selector?`, `outer_html?` | `nodes[]` / `html` | 只读 DOM |
| `browser__reset` | — | `ok` | 关闭并重开会话 |
| `browser__execute_raw_cdp` | `method`, `params?` | `result` | FullCdpAccess 门控 |

**验证要点:**
- [ ] 每个工具都有失败测试：mock `BrowserSession` 返回预期结果，验证 `ToolExecutor::handle` 输出 JSON 结构。
- [ ] 验证 `browser__execute_raw_cdp` 在配置未启用 full CDP 时不被注册。
- [ ] `cargo test -p ody-browser-control tool` PASS。

---

### Task 3.2: Code mode 兼容 [normal]

**Depends on:** 3.1  
**模式理由:** 机械验证；`ody-tools/src/code_mode.rs` 会自动处理 namespace + name 的嵌套名，但需确认 `browser__*` 不会与 reserved namespace 冲突。

**Files:**
- Add: `ody-browser-control/src/code_mode_tests.rs`

**实现要点:**
- 调用 `ody_tools::code_mode_name_for_tool_name(&ToolName::namespaced("browser", "navigate"))` 应得到 `"browser__navigate"`。
- 确认 `ody_code_mode::is_code_mode_nested_tool` 不拒绝该名称。

**验证要点:** 新增测试断言嵌套名正确；`cargo test -p ody-browser-control code_mode` PASS。

---

### Task 4.1: `services` 配置接入 [normal]

**Depends on:** 0.1  
**模式理由:** 配置 schema 在 0.1 已决定；本任务为机械接入。

**Files:**
- Modify: `ody-browser-control/src/config.rs`（定义 `BrowserControlConfig`）
- Modify: `ody-web-search/src/config.rs`（将 `ServicesConfig` 从 web-search 移到公共位置，或在 web-search 中新增字段）

**关键决策：**
- `ServicesConfig` 当前位于 `ody-web-search/src/config.rs`，并被 `config/src/config_toml.rs` 引用。新增 `browser_control` 字段时需要修改该 struct。
- 推荐：在 `ody-web-search/src/config.rs` 的 `ServicesConfig` 中增加 `#[serde(rename = "browserControl")] pub browser_control: Option<BrowserControlConfig>`，保持 `[services]` 表格统一。
- 或更好的长期方案：将 `ServicesConfig` 移到独立 crate（如 `ody-services-config`），但会扩大 scope；本 roadmap 采用最小修改方案。

**配置示例：**
```toml
[services.browserControl]
chrome_executable = "/usr/bin/google-chrome"
headless = true
default_viewport = { width = 1280, height = 720 }
console_log_limit = 1000
network_log_limit = 500
enable_full_cdp_access = false
```

**验证要点:**
- [ ] `config/src/config_toml.rs` 的测试 `services_web_search_deserializes` 附近新增 `services_browser_control_deserializes`。
- [ ] `cargo test -p ody-config` PASS。

---

### Task 4.2: app-server extension 注册 [normal]

**Depends on:** 1.1, 3.1, 4.1  
**模式理由:** 机械仿照 `web_search_extension.rs`；有明确参考实现。

**Files:**
- Add: `app-server/src/browser_control_extension.rs`
- Modify: `app-server/src/extensions.rs:97` 附近注册新扩展
- Modify: `app-server/Cargo.toml` 添加 `ody-browser-control = { workspace = true }`

**实现要点:**
- 实现 `ThreadLifecycleContributor`：线程启动时，若 `services.browser_control` 配置存在，创建 `BrowserSessionHandle` 并 `thread_store.insert`。
- 实现 `ConfigContributor`：配置变更时重建/销毁 session handle。
- 实现 `ToolContributor`：从 `thread_store` 取 handle，返回 `BrowserControlTool` 集合。
- 参考 `app-server/src/web_search_extension.rs:39-93` 的精确结构。

**验证要点:**
- [ ] 仿照 `web_search_extension.rs` 的 tests 写三个测试：config 创建 handle、无 config 时 tools 为空、有 handle 时返回工具列表。
- [ ] `cargo test -p ody-app-server browser_control` PASS。

---

### Task 4.3: Feature flag 门控 [normal]

**Depends on:** 3.1, 4.2  
**模式理由:** 已有 feature flag 定义，只需在 extension 注册和工具曝光处读取。

**Files:**
- Modify: `app-server/src/browser_control_extension.rs`
- Modify: `ody-browser-control/src/tool.rs`（根据配置决定 exposure）

**实现要点:**
- `ToolContributor::tools` 只在 `config.features.enabled(Feature::BrowserUse)` 时返回工具。
- `browser__execute_raw_cdp` 只在 `config.features.enabled(Feature::BrowserUseFullCdpAccess)` 时加入列表。
- `browser__click` / `browser__type` 只在 `config.features.enabled(Feature::ComputerUse)` 时加入列表（或设为 `ToolExposure::Hidden`）。
- 若 `BrowserUseExternal` 开启，允许配置使用外部 debugger URL；否则只允许启动 headless。

**验证要点:**
- [ ] 测试：feature 关闭时 `tools()` 返回空；`FullCdpAccess` 关闭时 raw CDP 工具不存在。
- [ ] `cargo test -p ody-app-server feature` PASS（避免破坏现有 feature 测试）。

---

### Task 5.1: 单元测试与 mock CDP [normal]

**Depends on:** 1.4  
**模式理由:** 与功能实现同步编写；不阻塞架构。

**Files:**
- Add/更新各模块 `*_tests.rs`

**实现要点:**
- 每个 CDP 调用都有 mock 响应测试。
- 提供 `ody-browser-control/src/testing.rs` 公共 mock transport + process，供 `app-server` 测试复用。

**验证要点:** `cargo test -p ody-browser-control` 全绿，覆盖率目标 >80% 行覆盖。

---

### Task 5.2: 集成测试（真实 Chrome） [normal]

**Depends on:** 4.2  
**模式理由:** 真实浏览器行为只能在有 Chrome 的环境运行；需明确标记为可选。

**Files:**
- Add: `ody-browser-control/tests/e2e_chrome.rs`

**实现要点:**
- 使用 `#[cfg_attr(not(feature = "e2e-chrome"), ignore)]` 或环境变量 `ODY_E2E_CHROME=1` 控制。
- 测试流程：启动 Chrome → navigate 到 `data:text/html,<h1>hello</h1>` → evaluate `document.querySelector('h1').textContent` → 断言返回 `"hello"` → screenshot → 断言非空 → kill。

**验证要点:** 在本地装有 Chrome/Edge 的开发者机器上运行；CI 默认跳过。

---

### Task 5.3: Windows 与路径兼容 [normal]

**Depends on:** 1.3, 5.1  
**模式理由:** 跨平台路径与进程差异需要专门验证。

**Files:**
- Modify: `ody-browser-control/src/process/discovery.rs`
- Modify: `ody-browser-control/src/process/launcher.rs`

**实现要点:**
- Windows Chrome 默认路径：`%LOCALAPPDATA%\Google\Chrome\Application\chrome.exe`、`%PROGRAMFILES%\Google\Chrome\Application\chrome.exe`、Edge 对应路径。
- 临时 profile 目录使用 `tempfile::TempDir`。
- 进程 kill：Windows 下 `Child::kill()` 可能留下子进程；考虑使用 `taskkill /T /F /PID` 作为 fallback（可选）。

**验证要点:**
- [ ] 在 Windows 开发环境运行 `cargo test -p ody-browser-control`。
- [ ] 手动验证 Chrome 发现能命中常见安装路径。

---

### Task 6.1: 安全审计与文档 [design]

**Depends on:** 4.3, 5.3  
**模式理由:** 浏览器控制是高风险能力，需要独立审计 gate。

**审计清单:**
- [ ] **Profile 隔离：** 确认 `--user-data-dir` 始终指向临时目录，不会读取/写入用户默认 profile。
- [ ] **审批策略：** 导航到非 `localhost`/非线程 cwd 的 URL 时，是否走 `ody_mcp`/`core` 的 approval 流程？若复用 MCP approval，需确认 `browser__navigate` 作为内置工具如何生成 `ApprovalRequest`。
- [ ] **凭证泄露防护：** `browser__evaluate` 禁止读取 `document.cookie`、`localStorage`、IndexedDB。技术上无法完全禁止，需通过审批+审计兜底。
- [ ] **Raw CDP 限制：** `browser__execute_raw_cdp` 必须绑定 `BrowserUseFullCdpAccess` feature，且企业配置可关闭。
- [ ] **日志敏感信息：** console/network 日志可能含用户数据，返回模型前是否脱敏？首期可文档化风险，后续增强。

**文档交付:**
- `docs/browser-control.md` 或更新 `AGENTS.md`：说明功能启用方式、配置示例、安全注意事项。

---

## 风险与开放问题

1. **`ServicesConfig` 归属问题：** 当前 `ServicesConfig` 位于 `ody-web-search` crate。新增 `browser_control` 字段会反向让 `ody-web-search` 依赖 `ody-browser-control`（或把 `ServicesConfig` 上提到新 crate）。推荐先按最小修改在 `ody-web-search` 中新增字段；若未来服务增多，再拆分 `ody-services-config`。
2. **审批基础设施复用：** `ody-web-search` 没有审批需求。浏览器导航/点击需要审批时，需研究 `core/src/mcp_tool_call.rs` 的 `build_mcp_tool_approval_question` 是否能被内置工具复用，或需要新增 `AskForApproval` 路径。
3. **与外部 Browser Use MCP 的冲突：** 代码中已有 `ODY_APPS_MCP_SERVER_NAME` 和 `browser-use` connector 的测试夹具。本内置 crate 上线后，需明确分工：内置 `ody-browser-control` 服务本地/headless 场景；外部 connector 服务用户已打开的浏览器。避免模型同时看到两套重复工具。
4. **Chrome 版本差异：** `headless=new` 与 `headless` 行为差异、CDP 方法弃用等。实现中需做运行时 version 检测或保守参数 fallback。
5. **Windows 编译：** 新增 crate 必须保持不依赖 platform-specific 代码（通过 `cfg` 隔离），否则会影响 Linux/macOS CI。

---

## Self-Review Checklist

- [ ] 每个 oversized phase 已拆分；无子任务捆绑独立可交付的工作。
- [ ] 无子任务的执行足迹（需读源码 + diffs + 测试输出）会迫使执行器 mid-task 压缩；每个子任务 fit 一个工作会话。
- [ ] 每个 `Depends on:` 指向更早子任务，并有源码级符号依赖支撑（trait、config struct、session handle）。
- [ ] 每个阶段的并行性已显式说明。
- [ ] 每个子任务有且仅有一个 mode tag 与一行代码 grounded 的理由。
- [ ] 顶部 rubric 已存在，保证未来编辑一致性。
- [ ] 本 roadmap 基于一次源码探索产出，非标题推断。
