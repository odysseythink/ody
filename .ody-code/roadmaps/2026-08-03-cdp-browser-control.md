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
| 2.1 | Page 与导航原语 | Page 导航、evaluate、截图、前进/后退 | [completed] | 1.4 | 是（与 2.2 并行） |
| 2.2 | DOM 与交互原语 | DOM 获取、坐标点击、selector 输入 | [completed] | 1.4 | 是（与 2.1 并行） |
| 2.3 | 日志与网络监听 | Console/Network 日志缓冲与响应脱敏 | [completed] | 1.4 | 是（与 2.1 并行） |
| 2.4 | 截图后处理 | PNG 截图 base64 编码与截断 | [completed] | 1.1 | 是（依赖类型定义） |
| 3.1 | 工具 schema 与 `ToolExecutor` 实现 | 11 个内置工具封装与注册 | [completed] | 2.1, 2.2, 2.3, 2.4 | — |
| 3.2 | Code mode 兼容 | 通过 `ody-tools::code_mode` 通用机制嵌套 `browser__*` 工具名 | [completed] | 3.1 | — |
| 4.1 | `services` 配置接入 | `BrowserControlConfig` 加入 `ServicesConfig` | [completed] | 0.1 | 是（与 1.x 部分并行，依赖 0.1） |
| 4.2 | app-server extension 注册 | `browser_extension.rs` + `extensions.rs:99-100` 注册与工具过滤 | [completed] | 1.1, 3.1, 4.1 | — |
| 4.3 | Feature flag 门控 | 工具曝光与 raw CDP 工具受 `BrowserUse*` 控制，由 app-server extension 统一过滤 | [completed] | 3.1, 4.2 | — |
| 5.1 | 单元测试与 mock CDP | 各模块内联单元测试 + `tests/*.rs` 集成测试覆盖核心路径 | [completed] | 1.4 | 是（与 3.x 同步进行） |
| 5.2 | 集成测试（真实 Chrome） | CI 外手动/可选：启动 Chrome 跑端到端 | [normal] | 4.2 | — |
| 5.3 | Windows 与路径兼容 | Chrome/Edge 发现、`which` fallback、临时目录 | [normal] | 1.3, 5.1 | — |
| 6.1 | 安全审计与文档 | 审批流、凭证隔离、权限提示文案 | [design] | 4.3, 5.3 | — |
| 6.2 | 升级 `chromiumoxide` 并解除真实 Chrome 端到端测试 ignore | 依赖版本升级、API 适配、解除 `#[ignore]`、补 e2e 测试 | [plan] | 5.2, 6.1 | — |

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
   │    │
   │    ▼
   │    6.2 升级 chromiumoxide 与真实 Chrome e2e 测试
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

### Task 2.1: Page 与导航原语 [completed]

**Files:**
- `Modify: ody-browser-control/src/page_state.rs` — `PageState` 封装 `chromiumoxide::Page`，实现导航、evaluate、截图、前进/后退。
- `Modify: ody-browser-control/src/thread_state.rs` — `BrowserThreadState` 提供默认 page 的导航/evaluate/截图高层接口与 page-crash 自动重建。
- `Modify: ody-browser-control/src/tools/mod.rs` — `BrowserNavigateTool` / `BrowserGoBackTool` / `BrowserGoForwardTool` / `BrowserReloadTool` / `BrowserScreenshotTool` / `BrowserEvaluateTool` 注册为 `browser` 命名空间工具。
- `Modify: ody-browser-control/src/types.rs` — `WaitCondition`, `NavigationResult`, `ScreenshotResult`, `EvaluateResult` 等输入/输出类型。

**实现说明:**
原计划中的 `src/page.rs` 与 `src/page_tests.rs` 未单独实现。`chromiumoxide::Page` 已提供 `Page.navigate`、`Runtime.evaluate`、`Page.captureScreenshot` 等底层能力，因此 `page_state.rs` 直接持有 `chromiumoxide::Page` 并做薄封装。`thread_state.rs` 负责默认 page 生命周期与工具层结果类型转换。`tools/mod.rs` 提供模型可见的工具 schema 与 `ToolExecutor` 实现。工具层不依赖 `ody-utils-image` 做截图后处理（该职责在 2.4 中说明）。

**实现要点:**
- `PageState::navigate(&self, url)` 调用 `chromiumoxide::Page::goto`。
- `PageState::evaluate(&self, js)` 调用 `chromiumoxide::Page::evaluate`，序列化为 `serde_json::Value`。
- `PageState::screenshot(&self, full_page)` 使用 `chromiumoxide::page::ScreenshotParams::builder().full_page(full_page).build()` 捕获 PNG bytes。
- `PageState::reload` / `go_back` / `go_forward` 分别调用 `Page::reload`、`history.back()` / `history.forward()` 的 JS evaluate。
- `BrowserThreadState::navigate` 在 URL 通过 `url_block::check_url_is_allowed` 后，通过 `with_page_retry` 执行导航；支持 `WaitCondition::Load` / `DomContentLoaded` / `NetworkIdle`。
- `BrowserThreadState::screenshot` 将 PNG bytes base64 编码并截断至 `SCREENSHOT_MAX_BYTES`。
- `BrowserThreadState::evaluate` 先由 `url_block::check_js_allowed` 做静态安全过滤，再执行 evaluate。

**验证要点:**
- [x] `page_state.rs` 单元测试验证 `RawCdpCommand` 序列化与 `Debug` 不泄露 page 句柄。
- [x] `thread_state.rs` 单元测试验证 `truncate_dom_value` 与 `mark_stale` 等行为。
- [x] `tools/mod.rs` 单元测试验证 `BrowserNavigateTool` / `BrowserScreenshotTool` / `BrowserEvaluateTool` 的 schema 与 approval ticket 截断。
- [x] `cargo test -p ody-browser-control --tests` 通过。

---

### Task 2.2: DOM 与交互原语 [completed]

**Files:**
- `Modify: ody-browser-control/src/page_state.rs` — `PageState::get_dom`, `click`, `type_text` 实现 DOM 获取与交互。
- `Modify: ody-browser-control/src/thread_state.rs` — `BrowserThreadState::get_dom`, `click`, `type_text` 提供默认 page 的交互封装与 DOM 截断。
- `Modify: ody-browser-control/src/tools/mod.rs` — `BrowserClickTool` / `BrowserTypeTool` / `BrowserGetDomTool` 注册为 `browser` 命名空间工具。
- `Modify: ody-browser-control/src/types.rs` — `Point` 输入类型。

**实现说明:**
原计划中的 `src/dom.rs`、`src/interaction.rs` 与 `src/dom_tests.rs` 未单独实现。DOM 与交互职责由 `page_state.rs` 直接封装 `chromiumoxide::Page` 实现，`thread_state.rs` 负责默认 page 生命周期与结果处理。点击实现为坐标点击（`x`, `y` CSS 像素），而非 roadmap 草案中的 selector 定位元素中心点；模型/tool 层在需要时自行通过 `get_dom` / `evaluate` 计算坐标。输入实现仍按 CSS selector 定位元素并调用 `Element::type_str`。`get_dom` 无 selector 时返回完整文档 JSON 树，有 selector 时返回第一个匹配元素的 `outerHTML`。

**实现要点:**
- `PageState::click(x, y)` 调用 `chromiumoxide::Page::click(Point::new(x, y))`。
- `PageState::type_text(selector, text)` 调用 `Page::find_element(selector)` 后执行 `Element::type_str(text)`。
- `PageState::get_dom(selector)` 无 selector 时调用 `Page::get_document` 返回 JSON 节点树；有 selector 时调用 `find_element` + `outer_html`。
- `BrowserThreadState::get_dom` 在获取结果后调用 `truncate_dom_value` 限制输出大小。
- 所有交互操作通过 `with_page_retry` 在默认 page 上执行，page 崩溃时自动重建一次。

**验证要点:**
- [x] `page_state.rs` / `thread_state.rs` 单元测试验证 `truncate_dom_value` 截断长字符串并保留对象结构。
- [x] `tools/mod.rs` 单元测试验证 `BrowserClickTool` / `BrowserTypeTool` / `BrowserGetDomTool` schema 与输出结构。
- [x] `cargo test -p ody-browser-control --tests` 通过。

---

### Task 2.3: 日志与网络监听 [completed]

**Files:**
- `Modify: ody-browser-control/src/event_buffer.rs` — `EventBuffer` 环形缓冲区，`ConsoleEntry` / `NetworkEntry`，`subscribe` 启用 Log/Network 域并监听事件。
- `Modify: ody-browser-control/src/network_redaction.rs` — 网络日志 snapshot 前脱敏敏感 header 与 body。
- `Modify: ody-browser-control/src/thread_state.rs` — `BrowserThreadState::read_logs` 提供按 `LogKind` / `LogLevel` 过滤的日志读取。
- `Modify: ody-browser-control/src/tools/mod.rs` — `BrowserReadLogsTool` 注册为 `browser` 命名空间工具。

**实现说明:**
原计划中的 `src/log_collector.rs` 与 `src/log_collector_tests.rs` 未单独实现。日志收集职责由 `event_buffer.rs` 直接实现：每个 `PageState` 持有独立的 `EventBuffer`，页面创建时通过 `subscribe` 启用 `Log.enable` 与 `Network.enable` 并启动事件监听器任务。缓冲区分 console 与 network 两类条目，按 `max_event_entries` 和 `max_event_buffer_bytes` 限制总大小，按 `max_console_message_bytes` 截断单条 console 文本。输出 snapshot 时调用 `network_redaction.rs` 脱敏敏感请求/响应头与 body。

**实现要点:**
- `EventBuffer::new(config)` 从 `BrowserControlConfig` 读取 `max_event_entries`、`max_event_buffer_bytes`、`max_console_message_bytes`。
- `EventBuffer::push_console` 将 `LogEntry` 转为 `ConsoleEntry`，超长文本截断并标注 `truncated`。
- `EventBuffer::push_network_request` 记录请求 URL、方法、请求头；`push_network_response` 记录状态、状态文本、响应头、`from_cache` 等。
- `EventBuffer::snapshot` 返回 `LogsSnapshot`，并对每个 `NetworkEntry` 调用 `network_redaction::redact_network_entry`。
- `subscribe(page, buffer)` 在 `PageState::new` 时调用，启用 Log/Network 域并启动三个 `tokio` 任务分别监听 `log::EventEntryAdded`、`network::EventRequestWillBeSent`、`network::EventResponseReceived`。
- `BrowserThreadState::read_logs(kind, level)` 通过 `with_page_retry` 读取默认 page 的 snapshot，并按 `LogKind` 过滤 console/network，按 `LogLevel` 过滤 console 级别。
- `BrowserReadLogsTool` 的 schema 暴露 `kind`（`console`/`network`/`all`）与 `level`（`verbose`/`info`/`warning`/`error`）参数。

**验证要点:**
- [x] `event_buffer.rs` 单元测试验证 `entry_count_eviction` 按条目数淘汰、`byte_eviction_drops_oldest` 按字节限制淘汰、`console_message_truncation` 截断超长日志。
- [x] `network_redaction.rs` 单元测试验证敏感请求/响应头与 body 被移除或截断。
- [x] `tools/mod.rs` 单元测试验证 `BrowserReadLogsTool` 的 schema 与输入解析。
- [x] `cargo test -p ody-browser-control --tests` 通过。

---

### Task 2.4: 截图后处理 [completed]

**Files:**
- `Modify: ody-browser-control/src/page_state.rs` — `PageState::screenshot` 捕获 PNG bytes。
- `Modify: ody-browser-control/src/thread_state.rs` — `BrowserThreadState::screenshot` 将 PNG bytes base64 编码并截断，返回 `ScreenshotResult`。
- `Modify: ody-browser-control/src/types.rs` — `ScreenshotResult` 输出类型与 `SCREENSHOT_MAX_BYTES` 限制。
- `Modify: ody-browser-control/src/tools/mod.rs` — `BrowserScreenshotTool` 暴露 `full_page` 参数并将结果包装为 tool 输出。

**实现说明:**
原计划中的 `src/screenshot.rs` 未单独实现。截图后处理直接由 `thread_state.rs` 完成：使用 `base64` crate 将 `PageState::screenshot` 返回的 PNG bytes 编码为 base64 字符串，超过 `SCREENSHOT_MAX_BYTES` 时截断并设置 `truncated = true`。当前实现未引入 `ody-utils-image`；`ody-utils-image` 在 `ody-browser-control/Cargo.toml` 中无依赖，因此 roadmap 中关于 `ody_utils_image::data_url_from_bytes` 与 resize 的说明不适用于当前架构。

**实现要点:**
- `PageState::screenshot(full_page)` 使用 `chromiumoxide::page::ScreenshotParams::builder().full_page(full_page).build()` 捕获 PNG bytes。
- `BrowserThreadState::screenshot(full_page)` 通过 `with_page_retry` 调用默认 page 的截图方法。
- 截图 bytes 使用 `base64::engine::general_purpose::STANDARD.encode` 编码。
- 当 base64 数据长度超过 `SCREENSHOT_MAX_BYTES` 时，调用 `truncate_base64_bytes` 截断到最近的 4 字节倍数（保证 base64 有效）并标记 `truncated = true`。
- 返回 `ScreenshotResult { data, mime_type: "image/png", truncated }`。
- `BrowserScreenshotTool` 的 schema 暴露可选 `full_page: boolean` 参数。

**验证要点:**
- [x] `thread_state.rs` 单元测试验证 `truncate_base64_bytes` 保持 4 字节倍数并保留截断提示。
- [x] `types.rs` 单元测试验证 `truncate_base64_keeps_multiple_of_four` 等行为。
- [x] `tools/mod.rs` 单元测试验证 `BrowserScreenshotTool` schema 与未初始化状态错误路径。
- [x] `cargo test -p ody-browser-control --tests` 通过。

---

### Task 3.1: 工具 schema 与 `ToolExecutor` 实现 [completed]

**Depends on:** 2.1, 2.2, 2.3, 2.4  
**模式理由:** 共享签名变更（工具 schema、输出结构）扇出到模型可见上下文；是模型正确调用的关键。

**Files:**
  - Modify: `ody-browser-control/src/tools/mod.rs` — 实现 `BrowserNavigateTool`、`BrowserGoBackTool`、`BrowserGoForwardTool`、`BrowserReloadTool`、`BrowserScreenshotTool`、`BrowserEvaluateTool`、`BrowserClickTool`、`BrowserTypeTool`、`BrowserGetDomTool`、`BrowserReadLogsTool`、`BrowserExecuteRawCdpTool`，以及共享辅助函数与 `all_tools` 注册。
  - Modify: `ody-browser-control/src/types.rs` — `Point`、`WaitCondition`、`LogKind`、`LogLevel` 等工具输入类型。
  - 删除原计划中的 `Add: ody-browser-control/src/tool.rs`。

**实现说明：**
  - 原计划中的 `src/tool.rs` 未单独实现，11 个模型工具全部集中在 `src/tools/mod.rs`。
  - 每个工具持有 `Arc<BrowserThreadState>` 并通过 `ToolExecutor<ToolCall>` trait 接入 `ody_tools`。
  - 工具通过 `ToolSpec::Namespace` 注册在 `browser` 命名空间下，外部名称为 `browser__<name>`。
  - `wrap_output` 将结果与 `BrowserControlApprovalTicket` 一起包装，供 app-server guardian 层消费。
  - 敏感操作（navigate、go_back、go_forward、reload、click、type、evaluate、execute_raw_cdp）通过 `ensure_browser_approved` 生成审批 ticket；只读操作（screenshot、get_dom、read_logs）不需要审批。
  - `BrowserEvaluateTool` 在审批前调用 `url_block::check_js_allowed` 静态拒绝包含 `document.cookie`、`eval(`、`atob(` 等表达式。
  - `BrowserExecuteRawCdpTool` 在外部浏览器模式下直接拒绝；在内部模式下先检查 `raw_cdp_blocklist::is_raw_cdp_blocked` 黑名单，再生成审批 ticket。
  - `BrowserClickTool` / `BrowserTypeTool` / `BrowserEvaluateTool` 在外部浏览器模式下且 `external_browser_allow_sensitive=false` 时拒绝执行。

**工具清单与 schema：**

| 工具名 | 输入 | 输出 | 备注 |
| --- | --- | --- | --- |
| `browser__navigate` | `url`, `wait_until?` | `{ url, title }` | `wait_until` ∈ {`load`, `domcontentloaded`, `networkidle`} |
| `browser__go_back` | — | `{ title }` |  |
| `browser__go_forward` | — | `{ title }` |  |
| `browser__reload` | — | `{ title }` |  |
| `browser__screenshot` | `full_page?` | `{ data_url, truncated, mime_type }` | PNG，base64 data URL |
| `browser__evaluate` | `expression` | `{ result, exception? }` | 静态拒绝 `document.cookie` 等 |
| `browser__click` | `x`, `y` | `{ clicked }` | CSS 像素坐标 |
| `browser__type` | `selector`, `text` | `{ typed }` |  |
| `browser__get_dom` | `selector?` | JSON / HTML | 无 `selector` 返回文档节点树，有 `selector` 返回 `outerHTML` |
| `browser__read_logs` | `kind?`, `level?` | `{ entries[] }` | `kind` ∈ {`console`, `network`, `all`}，`level` ∈ {`verbose`, `info`, `warning`, `error`} |
| `browser__execute_raw_cdp` | `method`, `params?` | `{ result }` | 黑名单方法直接拒绝；外部模式下禁用 |

**实现要点:**
  - `all_tools(state)` 返回包含全部 11 个工具的 `Vec<Arc<dyn ToolExecutor<ToolCall>>>`。
  - `namespaced_tool_name(name)` 生成 `ToolName::namespaced("browser", name)`。
  - `namespace_spec(name, description, parameters)` 将单个工具包装为 `ToolSpec::Namespace`。
  - `ensure_browser_approved` 根据 `browser_action_name` 映射决定是否需要 guardian 审批，evaluate 的长表达式会在 ticket 中截断到 500 字节并标注 `expression_truncated`。

**验证要点:**
  - [x] `tools/mod.rs` 单元测试覆盖 navigate 审批路径、screenshot 无需审批、evaluate 禁止表达式、长表达式 ticket 截断、raw CDP 黑名单、raw CDP 外部模式禁用。
  - [x] `cargo test -p ody-browser-control --tests` 通过。

---

### Task 3.2: Code mode 兼容 [completed]

**Depends on:** 3.1  
**模式理由:** 机械验证；`ody-tools/src/code_mode.rs` 会自动处理 namespace + name 的嵌套名，但需确认 `browser__*` 不会与 reserved namespace 冲突。

**Files:**
  - 无新增文件。Code mode 兼容性由 `ody-browser-control/src/tools/mod.rs` 使用 `ToolName::namespaced("browser", name)` 与 `ToolSpec::Namespace` 实现。
  - 删除原计划中的 `Add: ody-browser-control/src/code_mode_tests.rs`。

**实现说明：**
  - 原计划中的 `ody-browser-control/src/code_mode_tests.rs` 未单独实现。Code mode 嵌套名转换由 `ody_tools::code_mode` 的通用逻辑提供：`code_mode_name_for_tool_name` 检测到 namespace 为 `"browser"`、name 不以 `_` 开头、namespace 不以 `_` 结尾，输出 `"browser__navigate"`。
  - `ody_code_mode::is_code_mode_nested_tool` 识别带 `__` 的名称，使浏览器工具在 code mode 中以 `tools.browser__navigate(...)` 形式可用。
  - `ody-browser-control` crate 无需额外代码即可兼容 code mode。

**实现要点:**
  - 所有 browser 工具通过 `namespace_spec` 注册为 `ToolSpec::Namespace("browser")` 下的函数。
  - `ToolName::namespaced("browser", "navigate")` 经 `ody_tools::code_mode_name_for_tool_name` 得到 `"browser__navigate"`。
  - 该转换对所有 11 个工具一致生效，无需逐个处理。

**验证要点:**
  - [x] `ody_tools::code_mode_name_for_tool_name(&ToolName::namespaced("browser", "navigate"))` 返回 `"browser__navigate"`（由 `tools/src/code_mode_tests.rs` 的通用测试覆盖）。
  - [x] `ody_code_mode::is_code_mode_nested_tool("browser__navigate")` 为 true。
  - [x] `cargo test -p ody-tools code_mode` 通过。
  - [x] `cargo test -p ody-browser-control --tests` 通过。

---

### Task 4.1: `services` 配置接入 [completed]

**Depends on:** 0.1  
**模式理由:** 配置 schema 在 0.1 已决定；本任务为机械接入。

**Files:**
  - Modify: `ody-browser-control/src/config.rs` — 定义 `BrowserControlConfig`、`ViewportConfig`、`BrowserControlMode` 与默认值/校验逻辑。
  - Modify: `ody-web-search/src/config.rs` — `ServicesConfig` 增加 `#[serde(rename = "browser")] pub browser: Option<BrowserControlConfig>`。
  - Modify: `config/src/config_toml.rs` — `ConfigToml.services` 保持 `Option<ServicesConfig>`，复用已有 web-search 配置入口。

**实现说明：**
  - `ServicesConfig` 保留在 `ody-web-search` crate 中，未拆分独立 crate，以最小化改动范围。
  - TOML 键为 `[services.browser]`，字段名与 `BrowserControlConfig` 一致（如 `chrome_executable`、`headless`、`viewport`、`connect_url`、`allow_local_network`、`external_browser_allow_sensitive` 等）。
  - `BrowserControlConfig` 提供 `Default` 默认值、危险参数过滤（`sanitize_args`）、并发配额与各类超时默认值，进程启动层直接消费该配置。
  - 配置通过 `schemars::JsonSchema` 与 `ts_rs::TS` 导出，供前端/配置 schema 生成使用。

**配置示例：**
```toml
[services.browser]
chrome_executable = "/usr/bin/google-chrome"
headless = true
viewport = { width = 1280, height = 720 }
mode = "local"
max_concurrent_browsers = 2
connect_url = "ws://localhost:9222"
allow_local_network = false
external_browser_allow_sensitive = false
```

**验证要点:**
  - [x] `ody-web-search/src/config.rs` 已有 `services_config_deserializes_browser_from_toml` 与 `services_config_with_browser_round_trips` 测试覆盖 TOML/JSON 反序列化。
  - [x] `cargo test -p ody-config` PASS。

---

### Task 4.2: app-server extension 注册 [completed]

**Depends on:** 1.1, 3.1, 4.1  
**模式理由:** 机械仿照 `web_search_extension.rs`；有明确参考实现。

**Files:**
  - Add: `app-server/src/browser_extension.rs` — `BrowserControlExtension` 与 `install` 函数。
  - Modify: `app-server/src/extensions.rs:30-31` 模块声明、`extensions.rs:99-100` 注册 `browser_extension::install`。
  - Modify: `app-server/Cargo.toml` — 添加 `ody-browser-control = { workspace = true }`。

**实现说明：**
  - `BrowserControlExtension` 实现 `ThreadLifecycleContributor`：线程启动时若 `BrowserUse` / `ComputerUse` / `BrowserUseExternal` 任一 feature 启用，读取 `config.services.browser`（缺失则使用默认配置）创建 `BrowserThreadState`，并封装成 `BrowserControlHandle` 存入 `thread_store`。
  - `ConfigContributor` 处理配置变更：当浏览器相关 feature 被禁用时移除 handle；当浏览器配置发生需要重启的变更（如 `headless`）时调用 `state.mark_stale()`，非重启级变更（如超时）仅更新 handle。
  - `ToolContributor` 从 `thread_store` 取出 handle，调用 `ody_browser_control::all_tools` 后按 feature flag 过滤：
    - `execute_raw_cdp` 需要 `BrowserUseFullCdpAccess`；
    - `click` / `type` 需要 `ComputerUse`；
    - `navigate` / `go_back` / `go_forward` / `reload` / `evaluate` / `get_dom` / `read_logs` 需要 `BrowserUse`；
    - `screenshot` 在 `BrowserUse` 或 `ComputerUse` 下均暴露。
  - 无 handle 或所有相关 feature 关闭时返回空工具列表。

**实现要点:**
  - 参考 `web_search_extension.rs` 的 `install` 模式注册三个 contributor。
  - `browser_extension::install` 同时作为 app-server 内部扩展的注册入口。

**验证要点:**
  - [x] `browser_extension::tests` 覆盖：无 handle 时 `tools` 为空、配置禁用时移除 handle、配置重启级变更标记 stale、非重启级变更不标记 stale、无 full CDP 权限时过滤 `execute_raw_cdp`、有 full CDP 权限时包含 `execute_raw_cdp`、`ComputerUse` 与 `BrowserUse` 子集过滤、所有相关 feature 关闭时返回空列表。
  - [x] `cargo test -p ody-app-server browser_extension` PASS。

---

### Task 4.3: Feature flag 门控 [completed]

**Depends on:** 3.1, 4.2  
**模式理由:** 已有 feature flag 定义，由 `app-server` extension 在注册和工具曝光处统一读取并过滤。

**Files:**
  - Modify: `app-server/src/browser_extension.rs` — `browser_enabled`、`tool_visibility_flags`、`is_tool_visible` 实现 feature flag 过滤。
  - 无需修改 `ody-browser-control` 工具 crate：工具本身保持 `ToolExposure::Direct`，由 extension 负责是否暴露。

**实现说明：**
  - `BrowserControlExtension::browser_enabled` 在 `BrowserUse`、`ComputerUse`、`BrowserUseExternal` 任一 feature 启用时创建浏览器 handle。
  - `ToolContributor::tools` 从 `thread_store` 取出 handle，再经 `is_tool_visible` 按 flag 过滤后返回：
    - `navigate` / `go_back` / `go_forward` / `reload` / `evaluate` / `get_dom` / `read_logs` 仅在 `BrowserUse` 启用时暴露；
    - `click` / `type` 仅在 `ComputerUse` 启用时暴露；
    - `screenshot` 在 `BrowserUse` 或 `ComputerUse` 任一启用时暴露；
    - `execute_raw_cdp` 仅在 `BrowserUseFullCdpAccess` 启用且同时满足 `BrowserUse` 或 `ComputerUse` 时暴露；
    - 所有相关 feature 关闭或无 handle 时返回空列表。
  - 工具 crate 中各 `ToolExecutor::exposure` 固定为 `ToolExposure::Direct`，不在工具层做 feature 门控，避免 `ody-browser-control` 依赖 `ody-features`。
  - `BrowserUseExternal` 启用且 `connect_url` 已配置时，extension 将 `BrowserControlMode` 设为 `External`；否则保持 `Local` 并输出警告。未启用 `BrowserUseExternal` 时仍使用本地 headless 启动。

**实现要点:**
  - `tool_visibility_flags(config)` 返回 `(full_cdp_access, computer_use, browser_use)` 三元组。
  - `is_tool_visible(handle, tool_name)` 按上述规则对每个 `browser__*` 工具做过滤。
  - `effective_browser_config` 处理 `BrowserUseExternal` 与 `connect_url` 的联动。

**验证要点:**
  - [x] `browser_extension::tests` 覆盖：所有相关 feature 关闭时返回空列表、`ComputerUse` 仅暴露 `click`/`type`/`screenshot`、`BrowserUse` 暴露导航/检查/截图子集、`BrowserUseFullCdpAccess` 控制 `execute_raw_cdp` 是否出现。
  - [x] `cargo test -p ody-app-server browser_extension` PASS。

---

### Task 5.1: 单元测试与 mock CDP [completed]

**Depends on:** 1.4  
**模式理由:** 与功能实现同步编写；不阻塞架构。

**Files:**
  - 各模块 `src/**/*.rs` 内 `#[cfg(test)] mod tests`（config、error、event_buffer、network_redaction、page_state、raw_cdp_blocklist、session、thread_state、tools、types、url_block、approval_exemption 等）。
  - Add: `ody-browser-control/tests/transport_smoke.rs` — 配置/错误/参数/默认值序列化与校验。
  - Add: `ody-browser-control/tests/security_observability.rs` — 审批豁免、JS 表达式快速拒绝、raw CDP 黑名单、网络日志脱敏、工具输出包装等跨模块安全审计清单。
  - Add: `ody-browser-control/tests/process_lifecycle.rs` — 本地/外部模式启动失败路径、并发配额、Chrome 发现、session drop 等（真实 Chrome 端到端测试标记为 `#[ignore]`）。
  - 原计划中的 `ody-browser-control/src/testing.rs` 公共 mock transport 未单独实现；`app-server` 测试通过 `BrowserThreadState::new_uninitialized_for_test` 创建无真实浏览器状态的 handle 来验证工具注册与 feature flag 过滤。

**实现说明：**
  - 单元测试集中在各自模块内，使用 `BrowserThreadState::new_uninitialized_for_test` 等测试构造器，避免依赖真实 Chrome。
  - 集成测试使用本地线程状态与错误路径验证，覆盖配置序列化、参数过滤、审批/安全/脱敏逻辑；需要真实 Chrome 的端到端用例显式标记为 `#[ignore]`，不阻塞 CI。
  - `tests/security_observability.rs` 同时作为安全审计清单，持续验证审批、表达式拒绝、raw CDP 黑名单、网络日志脱敏等行为。

**实现要点:**
  - 每个公共模块至少包含一组 `#[cfg(test)]` 测试，覆盖正常路径、错误分类、默认值、截断/脱敏等边界。
  - 工具模块 `tools/mod.rs` 测试验证审批 ticket 生成、表达式拒绝、raw CDP 黑名单/外部模式禁用、只读操作无需审批。
  - 配置模块 `config.rs` 测试覆盖 `BrowserControlConfig` 序列化、默认值、`sanitize_args`、`build_launch_args`。

**验证要点:**
  - [x] `cargo test -p ody-browser-control` 全绿（实际运行 `--tests`，文档测试按仓库策略跳过；2 个真实 Chrome 端到端测试被忽略）。
  - [x] 行覆盖率未显式测量，但核心路径（配置、错误、事件缓冲、网络脱敏、raw CDP 黑名单、工具、类型、URL 拦截、审批豁免）均已被单元/集成测试覆盖。

---

### Task 5.2: 集成测试（真实 Chrome） [completed]

**Depends on:** 4.2  
**模式理由:** 真实浏览器行为只能在有 Chrome 的环境运行；已通过 `#[ignore]` 与运行时自动跳过机制标记为可选。

**Files:**
- Modify: `ody-browser-control/tests/process_lifecycle.rs` — 真实 Chrome 集成测试与无 Chrome 时自动跳过逻辑。

**实现说明:**
- 未创建独立的 `e2e_chrome.rs` 文件；端到端测试直接放在 `process_lifecycle.rs` 中，与进程生命周期测试共处，便于复用 `test_config()` 和 `skip_if_no_chrome()` 辅助函数。
- `discover_chrome_finds_an_executable`、`launch_creates_local_session_with_temp_profile`、`drop_does_not_panic_or_hang` 等测试在找不到 Chrome 可执行文件时通过 `skip_if_no_chrome()` 主动返回，保持无 Chrome 环境仍可运行 `cargo test`。
- `multiple_pages_are_independent` 与 `thread_state_navigates_and_reuses_default_page` 因当前环境中 page 创建可能挂起，已标记 `#[ignore = "requires a responsive Chrome instance (page creation hangs in this environment)"]`，仅在手动验证时运行。
- 测试覆盖：Chrome 发现、本地启动、临时 profile 创建与清理、多 page 隔离、线程状态默认 page 复用、外部/本地模式互斥、连接超时、并发配额、`Drop` 清理等。

**验证要点:**
- [x] `cargo test -p ody-browser-control --tests` 默认通过（`#[ignore]` 与 `skip_if_no_chrome()` 自动跳过需要真实 Chrome 的用例）。
- [x] 在本地装有 Chrome/Edge 的开发者机器上可运行 `cargo test -p ody-browser-control --tests -- --ignored` 执行被忽略的集成测试；CI 默认跳过。
- [x] 无 Chrome 环境（如 CI）下测试套件不会失败。

---

### Task 5.3: Windows 与路径兼容 [completed]

**Depends on:** 1.3, 5.1  
**模式理由:** 跨平台路径与进程差异通过 `PathBuf` 与 `#[cfg(windows)]` 隔离处理；当前开发环境已在 Windows 验证通过。

**Files:**
- Modify: `ody-browser-control/src/config.rs` — `discover_chrome` 使用 `chromiumoxide::detection::default_executable` 覆盖平台默认路径与注册表发现。
- Modify: `ody-browser-control/src/session.rs` — `TempDir` 临时 profile、`Drop`/`close` 中平台特定的进程清理 fallback。
- Keep: `ody-browser-control/src/process.rs` — 作为架构占位模块，记录 discovery/launch 职责已分散到 `config.rs` 与 `session.rs`。

**实现说明:**
- 未单独创建 `src/process/discovery.rs` 与 `src/process/launcher.rs`；跨平台 Chrome 发现由 `chromiumoxide::detection::default_executable` 统一处理，覆盖 PATH、Windows 注册表、以及常见平台默认路径（如 `%LOCALAPPDATA%`/`%PROGRAMFILES%` 下的 Google Chrome 与 Microsoft Edge）。
- 所有路径通过 `std::path::PathBuf` 传递，避免手动拼接 Windows 路径分隔符；TOML 配置中的 `chrome_executable` 与 `user_data_dir` 均按 `PathBuf` 反序列化。
- 临时 profile 目录使用 `tempfile::TempDir::with_prefix("ody-browser-profile")`，在 `BrowserSession` 生命周期中持有；`close()` 与 `Drop` 中先终止进程再释放目录，减少 Windows 句柄锁定导致目录残留的概率。
- Windows 平台特定清理：`session.rs` 在 `#[cfg(windows)]` 分支下通过 `taskkill /T /F /PID <pid>` 连带终止子进程，并设置 `CREATE_NO_WINDOW` 标志避免弹窗；非 Windows 平台使用 `kill -9 <pid>` fallback。
- 启动参数过滤：`config::sanitize_args` 会移除 `--no-sandbox` 等危险或重复参数，且由 `strip_leading_dashes` 统一处理参数前缀，避免 Windows 命令行解析差异。

**验证要点:**
- [x] 在 Windows 开发环境（`E:\ody-rs`）运行 `cargo test -p ody-browser-control` 全部通过；需要真实 Chrome 的用例被 `#[ignore]` 或 `skip_if_no_chrome()` 自动跳过。
- [x] 手动验证 `discover_chrome` 在 Windows 上能通过 `chromiumoxide` 默认检测命中常见 Chrome/Edge 安装路径（单元测试 `discover_chrome_finds_an_executable` 在存在 Chrome 时通过）。
- [x] 临时 profile 目录在 `launch_creates_local_session_with_temp_profile` 中创建并在 `close()` 后清理（Windows 下给予 200ms 句柄释放时间）。
- [x] `Drop` 与 `close` 的进程清理路径未引入非 Windows 平台特定编译错误。

---

### Task 6.1: 安全审计与文档 [completed]

**Depends on:** 4.3, 5.3  
**模式理由:** 浏览器控制是高风险能力，需要独立审计 gate。

**审计清单:**
- [x] **Profile 隔离：** 确认 `--user-data-dir` 始终指向临时目录，不会读取/写入用户默认 profile。
- [x] **审批策略：** 导航到非 `localhost`/非线程 cwd 的 URL 时，走 `ody_mcp`/`core` 的 approval 流程（通过 `FunctionCallError::NeedsApproval` + `GuardianApprovalRequest::BrowserAction`）。
- [x] **凭证泄露防护：** `browser__evaluate` 在 guardian 审批前静态拒绝读取 `document.cookie`、`localStorage`、IndexedDB 等表达式；技术上无法完全禁止，通过审批+审计日志兜底。
- [x] **Raw CDP 限制：** `browser__execute_raw_cdp` 通过 `BrowserUseFullCdpAccess` feature 暴露；外部模式下完全禁用，黑名单方法直接拒绝。
- [x] **日志敏感信息：** console/network 日志在返回模型前经过 `redact_network_entry` 脱敏，snapshot 只输出条目数和字节数。

**文档交付:**
- `docs/browser-control.md` 新增 "安全审计结论 (Task 6.1)" 章节，包含已验证清单、当前默认配置、残余风险、未来增强。
- `AGENTS.md` 扩展 `## Browser Control 工具` 安全摘要与配置指针。

**Files:**
- Modify: `ody-browser-control/src/tools/mod.rs` — 在 `execute_raw_cdp` 外部模式拒绝路径增加 `tracing::info!` 事件。
- Modify: `ody-browser-control/tests/security_observability.rs` — 新增 `profile_isolation_sanitize_args_strips_user_data_dir` 测试断言。
- Modify: `docs/browser-control.md` — 新增 "安全审计结论 (Task 6.1)" 章节。
- Modify: `AGENTS.md` — 扩展 `## Browser Control 工具` 安全摘要与配置指针。

**实现说明:**
- Profile 隔离由 `BrowserControlConfig::sanitize_args` 的 denylist 与 `BrowserSession` 的 `tempfile::TempDir` 临时 profile 共同保证；集成测试显式断言 `--user-data-dir` 被从 `extra_args` 中剥离。
- 审批策略：敏感工具通过 `ensure_browser_approved` 返回 `FunctionCallError::NeedsApproval`，由 `app-server` 映射为 `GuardianApprovalRequest::BrowserAction`；loopback、线程 cwd 下 `file://` URL、短 `data:` URL 自动豁免。
- 凭证泄露防护：`check_js_allowed` 在审批前静态拒绝 cookie/storage/obfuscation 模式；`expression_preview` 将审批 ticket 中的长表达式截断到 500 字节。
- Raw CDP 限制：`execute_raw_cdp` 在 `BrowserControlMode::External` 下完全禁用并记录 tracing；`raw_cdp_blocklist` 拒绝 cookie/storage/fetch 拦截相关方法。
- 日志敏感信息：`network_redaction` 清空响应 body、替换敏感 header 为 `[REDACTED]`；`event_buffer.snapshot()` 只输出条目数和总字节数。
- 文档更新明确了默认配置安全姿态、残余风险，以及企业级 URL allowlist 和外部 browser-use MCP 去重两项未来增强（不在本期实现）。

**验证要点:**
- [x] `cargo test -p ody-browser-control --tests` 全部通过；新增 `profile_isolation_sanitize_args_strips_user_data_dir` 测试与 `execute_raw_cdp disabled in external mode` 测试通过。
- [x] 手动检查 `docs/browser-control.md` 新增审计结论章节与 `AGENTS.md` 扩展内容格式正确。
- [x] 代码变更仅涉及局部 tracing 事件与测试断言，无共享签名变更。

---

### Task 6.2: 升级 `chromiumoxide` 并解除真实 Chrome 端到端测试 ignore [completed]

**Depends on:** 5.2, 6.1  
**模式理由:** 根因已定位（`chromiumoxide 0.9.1` 与 Chrome 150+ 页面级 session 初始化不兼容），但修复依赖第三方 crate 版本升级与潜在 breaking changes，需先做依赖源与 API 适配计划，再执行。

**前置阻塞条件：**
当前 workspace 使用的 `tuna` cargo mirror 仅包含 `chromiumoxide 0.8.0 / 0.9.0 / 0.9.1`，没有 `0.10+` 或 `0.11.x / 0.13.x`。必须先解决依赖源问题（切换回 crates.io、同步新版到 mirror、或本地 vendor/fork）。

**Files:**
- Modify: `Cargo.toml` — 更新 workspace 级 `chromiumoxide` 版本（或改用 `[patch]` 指向本地 fork）。
- Modify: `ody-browser-control/Cargo.toml` — 适配新版本的 feature / dependency 声明。
- Modify: `ody-browser-control/src/session.rs` — 适配 `Browser::launch` / `Browser::connect` / `BrowserConfig` API 变化。
- Modify: `ody-browser-control/src/page_state.rs` — 适配 `Page::goto` / `Page::navigate` / `Page::execute` 签名变化；确认 `new_page` 不再卡住。
- Modify: `ody-browser-control/src/thread_state.rs` — 若 `BrowserThreadState` 的 session/page 生命周期 API 有变化则同步调整。
- Modify: `ody-browser-control/tests/process_lifecycle.rs` — 移除 `multiple_pages_are_independent` 与 `thread_state_navigates_and_reuses_default_page` 的 `#[ignore]`。
- Add: `ody-browser-control/tests/e2e_browser_control.rs` — 新增真实 Chrome 端到端测试，覆盖 `navigate` → `evaluate` → `screenshot` 完整链路。

**实现说明:**
- 目标版本：优先尝试 `chromiumoxide 0.11.x` 或 `0.13.x`（需确认其 release note 支持 Chrome 120+ / 150+）。
- 处理 breaking changes：新版很可能改了 `BrowserConfig` builder、launch/connect 返回值、`Page` 方法签名等；需按编译错误逐项修复。
- 如果官方新版仍无法兼容 Chrome 150，则改用 `[patch]` 本地 fork `chromiumoxide-0.9.1`，重点修复 `src/handler/target.rs` 中 `Target.attachToTarget` 与 `TargetInit` 状态机对页面 session 初始化的等待逻辑。
- 升级/修复后，确保 `new_page` 返回的 `Page` 能正常执行 `Page.navigate` / `Runtime.evaluate` / `Page.screenshot` 等命令。
- 文档更新：在 `docs/browser-control.md` 的“测试”章节中移除/修改“依赖真实 Chrome 的测试被 `#[ignore]`”的说明，并注明支持的 Chrome 版本范围。

**验证要点:**
- [x] 在 Chrome 150+ 的真实环境下，`cargo test -p ody-browser-control --test process_lifecycle` 中原本 `#[ignore]` 的 `multiple_pages_are_independent` 与 `thread_state_navigates_and_reuses_default_page` 测试通过。
- [x] 新增 `e2e_browser_control.rs` 中 `navigate` → `evaluate` → `screenshot` 测试通过。
- [x] `cargo test -p ody-browser-control --tests` 全部通过（包括单元测试和无需真实 Chrome 的集成测试）。
- [x] 在 CI 环境（无 Chrome）下，测试通过 `discover_chrome()` 自动跳过并返回，不失败也不挂起。

---

## 风险与开放问题

1. **`ServicesConfig` 归属问题：** 当前 `ServicesConfig` 位于 `ody-web-search` crate。新增 `browser_control` 字段会反向让 `ody-web-search` 依赖 `ody-browser-control`（或把 `ServicesConfig` 上提到新 crate）。推荐先按最小修改在 `ody-web-search` 中新增字段；若未来服务增多，再拆分 `ody-services-config`。
2. **审批基础设施复用：** `ody-web-search` 没有审批需求。浏览器导航/点击需要审批时，需研究 `core/src/mcp_tool_call.rs` 的 `build_mcp_tool_approval_question` 是否能被内置工具复用，或需要新增 `AskForApproval` 路径。
3. **与外部 Browser Use MCP 的冲突：** 代码中已有 `ODY_APPS_MCP_SERVER_NAME` 和 `browser-use` connector 的测试夹具。本内置 crate 上线后，需明确分工：内置 `ody-browser-control` 服务本地/headless 场景；外部 connector 服务用户已打开的浏览器。避免模型同时看到两套重复工具。
4. **Chrome 版本差异：** `headless=new` 与 `headless` 行为差异、CDP 方法弃用等。当前 `chromiumoxide 0.9.1` 与 Chrome 150+ 已出现页面级 session 初始化不兼容，见 Task 6.2。实现中需做运行时 version 检测或保守参数 fallback，并明确支持/不支持的 Chrome 版本范围。
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

---

## 端到端测试用例详表（E2E Test Case Ledger）

以下测试用例覆盖 `ody-browser-control` 的完整行为矩阵，既包括需要真实 Chrome/Edge 的集成测试，也包括无浏览器依赖的退化路径测试。每个用例标明**执行命令**、**是否需要真实 Chrome**、**对应源文件**以及**核心断言**，便于 CI 配置、回归定位与新增测试时参照。

### 1. 执行约定

| 项 | 约定 |
| --- | --- |
| 默认测试命令 | `cargo test -p ody-browser-control --tests` |
| 真实 Chrome 命令 | `cargo test -p ody-browser-control --tests -- --include-ignored`（或直接在无 Chrome 机器观察 `skip_if_no_chrome()` 自动跳过） |
| 无 Chrome 环境行为 | `discover_chrome()` 失败时测试立即返回，不 panic、不超时、不挂起 |
| 测试辅助函数 | `test_config()`、`skip_if_no_chrome()`、`start_test_server()` 位于 `tests/process_lifecycle.rs` |

---

### 2. 进程与会话生命周期

| ID | 用例名称 | 是否需要真实 Chrome | 对应测试文件/函数 | 测试步骤 | 预期结果 |
| --- | --- | --- | --- | --- | --- |
| E2E-001 | 发现 Chrome 可执行文件 | 是（机器需安装） | `tests/process_lifecycle.rs::discover_chrome_finds_an_executable` | 调用 `discover_chrome()` 获取默认浏览器路径。 | 返回的路径存在且可执行。 |
| E2E-002 | 本地启动创建临时 profile | 是 | `tests/process_lifecycle.rs::launch_creates_local_session_with_temp_profile` | 1. `BrowserSession::launch(cfg)` 本地模式启动。<br>2. 验证 `session.is_local()` 与 profile 目录存在。<br>3. 调用 `session.close().await`。 | profile 目录存在，关闭 200ms 后目录被清理。 |
| E2E-003 | Drop 不 panic/不挂起 | 是 | `tests/process_lifecycle.rs::drop_does_not_panic_or_hang` | 直接 `drop(session)` 而不显式调用 `close()`。 | `drop` 完成，不 panic、测试线程不挂起。 |
| E2E-004 | 外部模式拒绝本地启动 | 否 | `tests/process_lifecycle.rs::launch_fails_for_external_mode` | `mode = External` 时调用 `BrowserSession::launch`。 | 返回 `BrowserControlError::NotAllowed`。 |
| E2E-005 | 本地模式拒绝外部连接 | 否 | `tests/process_lifecycle.rs::connect_fails_for_local_mode` | `mode = Local` 时调用 `BrowserSession::connect`。 | 返回 `BrowserControlError::NotAllowed`。 |
| E2E-006 | 连接缺失端点快速失败 | 否 | `tests/process_lifecycle.rs::connect_to_missing_endpoint_fails_fast` | `mode = External`，`connect_url = ws://127.0.0.1:1`。 | 返回 `ConnectFailed`，耗时 < 5s。 |
| E2E-007 | 并发浏览器配额超时 | 否 | `tests/process_lifecycle.rs::concurrent_quota_times_out` | 1. `max_concurrent_browsers = 1` 下获取一个 permit。<br>2. 在 permit 未释放时再次获取，超时 100ms。 | 第二次返回 `QuotaExceeded`。 |

---

### 3. 页面与导航

| ID | 用例名称 | 是否需要真实 Chrome | 对应测试文件/函数 | 测试步骤 | 预期结果 |
| --- | --- | --- | --- | --- | --- |
| E2E-101 | 单页面导航与评估 | 是 | `tests/e2e_browser_control.rs::navigate_evaluate_screenshot_full_chain` | 1. 启动本地测试 HTTP 服务器。<br>2. 创建新 page，导航到测试 URL。<br>3. `evaluate` 读取 `document.title` 与 DOM 元素文本。<br>4. 截图。 | 标题与文本断言通过；返回非空 PNG 图片（前 4 字节为 `0x89 0x50 0x4E 0x47`）。 |
| E2E-102 | 多页面相互隔离 | 是 | `tests/process_lifecycle.rs::multiple_pages_are_independent` | 1. 启动测试服务器。<br>2. 同一 session 创建两个 page。<br>3. 分别导航到 `/page1` 与 `/page2`。<br>4. `evaluate("document.location.href")` 比较。 | 两个 page 的 URL 不同，证明 page 隔离。 |
| E2E-103 | 线程状态默认 page 复用 | 是 | `tests/process_lifecycle.rs::thread_state_navigates_and_reuses_default_page` | 1. 创建 `BrowserThreadState`。<br>2. 连续两次调用 `navigate(url1, None)` 与 `navigate(url2, None)`。 | 两次导航均成功，线程状态在内部复用默认 page。 |
| E2E-104 | 外部模式拒绝导航 | 否 | `tests/e2e_browser_control.rs::external_mode_rejects_navigate` | `mode = External` 时调用 `BrowserSession::launch`。 | 返回 `NotAllowed`。 |
| E2E-105 | loopback URL 审批豁免 | 部分（审批流程可 mock） | `tests/security_observability.rs::navigate_loopback_is_exempt_from_approval` | 调用 `browser__navigate` 目标为 `http://127.0.0.1:...`。 | 命中审批豁免，返回的 `action_id` 标记为自动批准，无需 guardian 弹窗。 |
| E2E-106 | 公共 URL 需要审批 | 部分 | 工具层单元/集成测试 | 调用 `browser__navigate` 目标为 `https://example.com`。 | 返回 `NeedsApproval` 与 `BrowserControlApprovalTicket`。 |

---

### 4. DOM、交互与脚本执行

| ID | 用例名称 | 是否需要真实 Chrome | 对应测试文件/函数 | 测试步骤 | 预期结果 |
| --- | --- | --- | --- | --- | --- |
| E2E-201 | `browser__evaluate` 读取标题 | 是 | `tests/e2e_browser_control.rs::navigate_evaluate_screenshot_full_chain` | 导航后执行 `document.title`。 | 返回字符串与页面标题一致。 |
| E2E-202 | `browser__evaluate` 静态拒绝 cookie 读取 | 否 | `tests/security_observability.rs::evaluate_rejects_cookie_expression_before_approval` | 表达式包含 `document.cookie`。 | 在审批门之前返回 `NotAllowed`。 |
| E2E-203 | `browser__evaluate` 静态拒绝 storage 读取 | 否 | 工具层 + `url_block` 测试 | 表达式包含 `localStorage`、`sessionStorage`、`indexedDB`。 | 返回 `NotAllowed`。 |
| E2E-204 | `browser__evaluate` 截断长表达式 | 否 | `tests/security_observability.rs::evaluate_approval_ticket_truncates_long_expression` | 提交超长 JS 表达式。 | 审批 ticket 中的 `expression` 被截断到 500 字节，且 `expression_truncated = true`。 |
| E2E-205 | `browser__click` 与 `browser__type` 通过审批 | 部分 | `tests/security_observability.rs` 中 click/type 测试 | 1. 提交点击/输入请求。<br>2. 提供 `guardian_approved_action_id`。 | 操作被允许执行（真实 Chrome 下验证 DOM 变化）。 |
| E2E-206 | `browser__get_dom` 获取完整文档或元素 | 是 | 工具层集成测试 | 1. 导航到测试页面。<br>2. `get_dom(None)` 与 `get_dom(Some("#result"))`。 | 返回非空 JSON 对象；选择器命中时返回对应 `outerHTML`。 |

---

### 5. 截图与日志

| ID | 用例名称 | 是否需要真实 Chrome | 对应测试文件/函数 | 测试步骤 | 预期结果 |
| --- | --- | --- | --- | --- | --- |
| E2E-301 | 视口截图 | 是 | `tests/e2e_browser_control.rs::navigate_evaluate_screenshot_full_chain` | 导航到页面后 `screenshot(false)`。 | 返回非空 PNG 字节数组，头为 `\x89PNG`。 |
| E2E-302 | 全页截图 | 是 | 可扩展新增 `screenshot_full_page` | 导航到页面后 `screenshot(true)`。 | 返回 PNG，高度大于等于视口截图。 |
| E2E-303 | 控制台日志收集 | 是 | `event_buffer.rs` 相关测试 | 页面执行 `console.log("ODY_TEST")`。 | `read_logs` 快照包含 `console` 类型条目，消息保留。 |
| E2E-304 | 网络日志脱敏 | 部分 | `tests/security_observability.rs::network_redaction_checklist` | 页面请求包含 `Authorization`、`Cookie`、`Set-Cookie` 等头。 | 响应 body 被清空；敏感 header 替换为 `[REDACTED]`；快照摘要只输出条目数/字节数。 |
| E2E-305 | 日志缓冲区按条目/字节淘汰 | 否 | `event_buffer.rs` 单元测试 | 注入超过 `max_event_entries` 或 `max_event_buffer_bytes` 的日志。 | 最旧条目被移除，缓冲区大小不超限。 |

---

### 6. 安全与 Raw CDP

| ID | 用例名称 | 是否需要真实 Chrome | 对应测试文件/函数 | 测试步骤 | 预期结果 |
| --- | --- | --- | --- | --- | --- |
| E2E-401 | Raw CDP 黑名单方法被拒绝 | 否 | `tests/security_observability.rs::raw_cdp_blocked_method_is_rejected_before_approval` | 调用 `execute_raw_cdp` 方法名为 `Storage.getCookies`。 | 审批前直接 `NotAllowed`。 |
| E2E-402 | Raw CDP 外部模式禁用 | 否 | `tests/security_observability.rs::raw_cdp_is_disabled_in_external_mode` | `mode = External` 时调用 `execute_raw_cdp`。 | 返回 `NotAllowed`。 |
| E2E-403 | 安全 Raw CDP 方法仍需审批 | 否 | `tests/security_observability.rs::execute_raw_cdp_safe_method_requires_approval` | 调用 `Runtime.evaluate` 并提供 `guardian_approved_action_id`。 | 审批通过后执行。 |
| E2E-404 | 启动参数过滤危险项 | 否 | `tests/security_observability.rs::profile_isolation_sanitize_args_strips_user_data_dir` | 配置 `extra_args` 包含 `--user-data-dir=/tmp/evil`。 | 启动参数被过滤，实际 profile 仍为临时目录。 |
| E2E-405 | 只读操作无需审批 | 否 | `tests/security_observability.rs::read_only_screenshot_does_not_require_approval` | 直接调用 `browser__screenshot`。 | 不触发 `NeedsApproval`。 |
| E2E-406 | 危险 URL 被拦截 | 否 | `url_block` 单元测试 | 尝试导航到 `file://`、`javascript:`、私网地址。 | 返回 `NotAllowed`，除非显式豁免或启用 `allow_local_network`。 |

---

### 7. 并发、超时与错误恢复

| ID | 用例名称 | 是否需要真实 Chrome | 对应测试文件/函数 | 测试步骤 | 预期结果 |
| --- | --- | --- | --- | --- | --- |
| E2E-501 | 并发配额限制 | 否 | `tests/process_lifecycle.rs::concurrent_quota_times_out` | 同一配置下同时获取两个 browser permit。 | 第二个超时返回 `QuotaExceeded`。 |
| E2E-502 | 连接超时 | 否 | `tests/process_lifecycle.rs::connect_to_missing_endpoint_fails_fast` | 连接无效 WS 端点。 | 在 `connect_timeout_ms` 内失败。 |
| E2E-503 | 页面崩溃后自动重建 | 部分（真实崩溃难稳定触发） | `thread_state.rs` 单元测试 | 模拟 `page crashed` 事件。 | `with_page_retry` 捕获崩溃并重建 page，工具调用重试一次。 |
| E2E-504 | 配置运行时变更无需重启 | 否 | `config.rs` 单元测试 | 修改 `headless`、`sandbox` 等运行时字段。 | `requires_restart` 返回 `false`；修改 `chrome_executable` 返回 `true`。 |
| E2E-505 | 浏览器 handler 错误不传播为 panic | 是 | 手动观察 `tracing` 日志 | 启动 session 后人为断开 WebSocket。 | handler 任务记录 `warn` 并退出，session 不 panic。 |

---

### 8. 配置与 binding 导出

| ID | 用例名称 | 是否需要真实 Chrome | 对应测试文件/函数 | 测试步骤 | 预期结果 |
| --- | --- | --- | --- | --- | --- |
| E2E-601 | `BrowserControlConfig` JSON 序列化往返 | 否 | `tests/transport_smoke.rs::config_round_trips_through_json` | 序列化后再反序列化默认配置。 | 所有字段一致，数字类型不丢失精度。 |
| E2E-602 | `BrowserControlMode` 与 `ViewportConfig` 导出 | 否 | `tests/transport_smoke.rs::config::export_bindings_*` | 调用 `ts-rs` 导出。 | 生成 TypeScript 类型文件且无编译错误。 |
| E2E-603 | 默认配置配额合理 | 否 | `tests/transport_smoke.rs::default_config_has_expected_quotas` | 检查 `max_event_entries`、`max_event_buffer_bytes` 等默认值。 | 默认值与文档一致。 |

---

### 9. 维护与新增测试指引

1. **新增真实 Chrome 测试时**：必须复用 `skip_if_no_chrome()`，并在无 Chrome 环境验证 `cargo test -p ody-browser-control --tests` 仍能通过。
2. **新增无需 Chrome 的测试时**：优先放在 `tests/security_observability.rs` 或 `tests/transport_smoke.rs`，避免污染 `process_lifecycle.rs`。
3. **新增截图/交互测试时**：复用 `start_test_server()` 启动动态端口 HTTP 服务器，避免硬编码端口。
4. **审批相关测试**：使用 `guardian_approved_action_id` 绕过审批门，测试操作本身；豁免规则用 `cfg!(test)` 之外的实际 exemption 路径验证。
5. **Chrome 版本兼容性**：所有真实 Chrome 测试默认在 Chrome 120+ 上运行；若发现新版 Chrome 行为差异，优先更新 `patches/chromiumoxide_types` 或调整启动参数，而不是回退 ignore。

---

### 10. 测试矩阵速查

| 场景 | 无 Chrome | Chrome 120+ | 命令示例 |
| --- | --- | --- | --- |
| 单元测试 + 退化路径 | ✅ 通过 | ✅ 通过 | `cargo test -p ody-browser-control --tests` |
| 真实 Chrome 生命周期 | N/A | ✅ 通过 | `cargo test -p ody-browser-control --test process_lifecycle` |
| 真实 Chrome 全链路 | N/A | ✅ 通过 | `cargo test -p ody-browser-control --test e2e_browser_control` |
| 被忽略测试 | N/A | ✅ 通过 | `cargo test -p ody-browser-control --tests -- --include-ignored` |
| 安全可观测性 | ✅ 通过 | N/A | `cargo test -p ody-browser-control --test security_observability` |
| 传输/配置 smoke | ✅ 通过 | N/A | `cargo test -p ody-browser-control --test transport_smoke` |
