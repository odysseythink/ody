# Part 1 — CDP 传输与 chromiumoxide 集成

## 决策

使用 [`chromiumoxide`](https://github.com/mattsse/chromiumoxide) 0.9.x 作为 CDP 客户端库，封装其 `Browser`/`Page`/`Handler` 模型，并在 `ody-browser-control` 内部再做一层 Ody 适配。

选择理由 `[C:USER]`：
- 用户在方案对比中明确选择 chromiumoxide 全栈封装。
- 库提供类型安全的 CDP 命令（`chromiumoxide_cdp` 生成类型）、页面生命周期、事件监听和进程管理，减少自研协议风险。

权衡：
- **收益**：CDP 类型由 PDL 自动生成；启动/连接/等待/关闭流程完整；`Page` 可 clone，便于多工具并发。
- **代价**：引入额外依赖树（`async-tungstenite`、`futures`、`chromiumoxide_cdp`/`_types`），与 workspace 已有的 `tokio-tungstenite` 并存；编译体积和版本锁定增加。

## 关键上游类型

参考已克隆的 `.ody-code/spikes/chromiumoxide-refs`：`[C:UPSTREAM]`

```rust
// src/browser/mod.rs
pub struct Browser {
    sender: Sender<HandlerMessage>,
    config: Option<BrowserConfig>,
    child: Option<Child>,
    debug_ws_url: String,
    browser_context: BrowserContext,
}

impl Browser {
    pub async fn connect(url: impl Into<String>) -> Result<(Self, Handler)>;
    pub async fn launch(config: BrowserConfig) -> Result<(Self, Handler)>;
    pub async fn new_page(&self, params: impl Into<CreateTargetParams>) -> Result<Page>;
    pub async fn pages(&self) -> Result<Vec<Page>>;
    pub async fn close(&mut self) -> Result<CloseReturns>;
    pub async fn wait(&mut self) -> io::Result<Option<ExitStatus>>;
}

// src/page.rs
pub struct Page { inner: Arc<PageInner> }

impl Page {
    pub async fn navigate(&self, url: impl Into<String>) -> Result<&Self>;
    pub async fn evaluate(&self, evaluate: impl Into<Evaluation>) -> Result<EvaluationResult>;
    pub async fn evaluate_expression(&self, evaluate: impl Into<EvaluateParams>) -> Result<EvaluationResult>;
    pub async fn screenshot(&self, params: impl Into<ScreenshotParams>) -> Result<Vec<u8>>;
    pub async fn click(&self, point: Point) -> Result<&Self>;
    pub async fn find_element(&self, selector: impl Into<String>) -> Result<Element>;
    pub async fn close(self) -> Result<()>;
    pub async fn execute<T: Command>(&self, cmd: T) -> Result<CommandResponse<T::Response>>;
    pub fn event_listener<E: IntoEventKind>(&self) -> Result<EventStream<E>>;
}

// src/handler/mod.rs（驱动 WebSocket 的 future）
pub struct Handler { /* ... */ }
impl Future for Handler { /* 驱动 CDP 事件循环 */ }
```

## 线程级连接模型

每个 Ody 线程拥有**一个** `Browser` 实例，由 `ody-browser-control` 的 `BrowserSession` 句柄管理：`[C:INFERRED]`

```rust
pub struct BrowserSession {
    browser: Browser,
    /// chromiumoxide 的 Handler 需要被一个 tokio task 持续驱动，否则 CDP 事件循环不会推进。
    handler_task: JoinHandle<()>,
    /// 每个线程一个临时 profile，线程结束时随 BrowserSession 一起销毁。
    _profile_dir: TempDir,
}

impl BrowserSession {
    /// 启动或连接 Chrome，返回 Self。
    pub async fn launch(config: BrowserControlConfig) -> Result<Self, BrowserControlError>;

    /// 在线程内复用同一个 browser 创建新 page/target。
    pub async fn new_page(&self) -> Result<Page, BrowserControlError>;

    /// 关闭所有 page 并 kill 进程。
    pub async fn close(self) -> Result<(), BrowserControlError>;
}
```

- `BrowserSession` 作为线程级状态，由 `ody_extension_api` 的 `ThreadLifecycleContributor::on_thread_start` 注入 `thread_store`。
- 一个 `BrowserSession` 可以创建多个 `Page`（`browser.new_page(...)`），对应一次 turn 中多个工具调用共享同一进程。
- 线程结束时调用 `BrowserSession::close()` 关闭 browser 并清理 `TempDir`。

## CDP 命令抽象

工具层直接调用 `chromiumoxide::Page` 和 `Browser` 的方法；仅在需要统一处理重试、超时、事件订阅或跨 CDP 版本差异时，才封装为 `CdpCommand`/`CdpEvent` 抽象。首期避免过度抽象：`[C:INFERRED]`

内部保留一个最小化的 `BrowserCommand` trait/枚举，用于 `execute_raw_cdp` 统一分发，但 navigate、screenshot、evaluate、click 等标准工具直接调用 `Page` 方法。

```rust
pub enum CdpCommand {
    Navigate { url: String },
    Evaluate { expression: String, return_by_value: bool },
    Screenshot { full_page: bool, format: ScreenshotFormat },
    Click { x: f64, y: f64 },
    Type { selector: String, text: String },
    GetDom { selector: Option<String> },
    GetConsoleLog { limit: usize },
    GetNetworkLog { limit: usize },
    ExecuteRaw { method: String, params: serde_json::Value },
}

pub enum CdpEvent {
    Console(ConsoleEntry),
    Network(NetworkEntry),
    PageLoad { url: String },
}
```

首期映射到 chromiumoxide 的方法：`[C:UPSTREAM]`

| Ody 命令 | chromiumoxide API | 说明 |
|---|---|---|
| `Navigate` | `Page::navigate` | 等待 `load` 事件完成 |
| `Evaluate` | `Page::evaluate` / `Page::evaluate_expression` | 返回序列化后的 JS 结果 |
| `Screenshot` | `Page::screenshot(ScreenshotParams)` | 返回 PNG bytes |
| `Click` | `Page::click(Point)` | 模拟鼠标点击 |
| `Type` | `Element::type_text` 或 `Page::evaluate` | 先定位元素再输入 |
| `GetDom` | `Page::get_document` / `Page::find_elements` | 返回节点/属性文本 |
| `GetConsoleLog` | `page.event_listener::<EventEntryAdded>()` | 持续收集 console 事件 |
| `GetNetworkLog` | `page.event_listener::<EventNetworkRequested>()` | 持续收集 network 事件 |
| `ExecuteRaw` | `Page::execute<T>(...)` | 需自定义 `Command` trait 实现 |

## 事件收集模型

console 与 network 日志需要在页面生命周期内持续收集，而不是每次调用时重新订阅：`[C:INFERRED]`

```rust
pub struct PageState {
    page: Page,
    console_buffer: Arc<Mutex<Vec<ConsoleEntry>>>,
    network_buffer: Arc<Mutex<Vec<NetworkEntry>>>,
}

impl PageState {
    pub async fn subscribe_events(&self) -> Result<(), BrowserControlError> {
        let mut console_stream = self.page.event_listener::<EventEntryAdded>().await?;
        let console_buf = self.console_buffer.clone();
        tokio::spawn(async move {
            while let Some(event) = console_stream.next().await {
                console_buf.lock().await.push(event.into());
            }
        });
        // network 同理
    }
}
```

- `PageState` 和 `BrowserSession` 一起存放在线程 store 中。
- `browser__read_logs` 工具只读取缓冲区的快照，不触发新 CDP 调用。
- 缓冲区按条目数（默认 1000）和总字节数（默认 1MB）双重限制；任一达到上限时滚动丢弃最旧条目。
- network 事件只保留摘要：url、status、method、timestamp、request/response 大小；不保留 response body。
- console 消息长度超过 4KB 时截断。

## 错误映射

把 `chromiumoxide::error::CdpError` 映射到 `ody-browser-control` 的 `BrowserControlError`：`[C:INFERRED]`

```rust
pub enum BrowserControlError {
    ChromeNotFound { searched_paths: Vec<PathBuf> },
    LaunchFailed { source: CdpError },
    ConnectFailed { source: CdpError },
    CommandFailed { command: String, source: CdpError },
    Timeout { command: String, elapsed_ms: u64 },
    PageCrashed,
    NotAllowed { reason: String },
}

impl std::fmt::Display for BrowserControlError { /* ... */ }
impl std::error::Error for BrowserControlError { /* ... */ }
```

工具层把 `BrowserControlError` 再转换为 `ody_tools::FunctionCallError::Fatal` 或 `Retryable`：`[C:INFERRED]`

- `PageCrashed` / `Timeout` / `ConnectFailed` → `Retryable`（由上层按有限重试策略处理）。
- `NotAllowed` / `ChromeNotFound` / 明确语法错误 → `Fatal`。

## 并发与线程安全

- `chromiumoxide::Page` 是 `Clone`（内部 `Arc<PageInner>`），可安全跨工具调用共享。
- 但 `Page` 上的方法调用会排队进入同一 CDP target session，因此**同一 Page 的调用是串行的**；不同 Page 之间可并发。
- 建议为每个工具调用分配独立 `Page`（或每个 turn 一个默认 Page），避免一个工具阻塞另一个。

## 依赖与冲突

chromiumoxide 依赖 `async-tungstenite` 0.32，而 workspace 已有 `tokio-tungstenite` 0.28/0.29。两者底层都依赖 `tungstenite` 但由不同 facade crate 提供 runtime 集成：`[C:UPSTREAM]`

- 理论上可以同时存在，但会引入两个 WebSocket 运行时 facade。
- 如果 Cargo 解析出 `tungstenite` 版本冲突，可考虑：
  1. 仅使用 chromiumoxide 的 API，不直接使用 workspace 的 `tokio-tungstenite`（本期推荐）。
  2. 未来如果冲突严重，评估 chromiumoxide 版本或 fork 其 transport 层改用 `tokio-tungstenite`。

## 需要验证的假设

1. **编译兼容性**：`chromiumoxide = "0.9.1"` 能否在 workspace 中 `cargo check` 通过。验证方式：在 `.ody-code/spikes/` 下创建临时 crate，仅依赖 `chromiumoxide` 与 workspace 的 `tokio`，运行 `cargo check`。
2. **事件监听稳定性**：`Page::event_listener` 在 `new_page` 后订阅 console/network 事件是否持续有效。验证方式：spike 中启动 Chrome，订阅 `EventEntryAdded`，连续 evaluate 多条 `console.log` 并断言收到事件。
3. **并发调用**：多个 `Page` 实例在同一 `Browser` 上并发调用是否互不阻塞。验证方式：spike 中创建两个 page，一个 navigate 慢速页面，另一个同时 screenshot，记录时序。

## 接口草案（伪代码）

```rust
// ody-browser-control/src/session.rs
pub struct BrowserSession {
    browser: chromiumoxide::Browser,
    handler_task: JoinHandle<()>,
    _profile_dir: TempDir,
}

impl BrowserSession {
    pub async fn launch(config: BrowserControlConfig) -> Result<Self, BrowserControlError>;
    pub async fn connect(ws_url: &str) -> Result<Self, BrowserControlError>;
    pub async fn new_page(&self) -> Result<PageState, BrowserControlError>;
    pub async fn close(self) -> Result<(), BrowserControlError>;
}

// ody-browser-control/src/page_state.rs
pub struct PageState {
    page: chromiumoxide::Page,
    console_log: Arc<Mutex<Vec<ConsoleEntry>>>,
    network_log: Arc<Mutex<Vec<NetworkEntry>>>,
}

impl PageState {
    pub async fn navigate(&self, url: &str) -> Result<(), BrowserControlError>;
    pub async fn evaluate(&self, js: &str) -> Result<serde_json::Value, BrowserControlError>;
    pub async fn screenshot(&self, full_page: bool) -> Result<Vec<u8>, BrowserControlError>;
    pub async fn click(&self, x: f64, y: f64) -> Result<(), BrowserControlError>;
    pub async fn type_text(&self, selector: &str, text: &str) -> Result<(), BrowserControlError>;
    pub async fn read_logs(&self) -> Result<LogsSnapshot, BrowserControlError>;
    pub async fn execute_raw(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value, BrowserControlError>;
}
```

## 关键依赖

- `chromiumoxide` = "0.9.1"（新增）。
- `chromiumoxide_cdp` 通过 chromiumoxide 间接引入（用于 `Command` 和 `Event` 类型）。
- `futures`（已间接存在）用于事件流消费。
- `serde_json`（workspace 已有）用于原始参数和返回值。
- `tokio`（workspace 已有）用于 `handler_task` 和事件流任务。

## 设计影响

- 后续 `process_lifecycle.md` 负责如何把 `BrowserSession::launch` 接入 Chrome 发现与临时 profile。
- 后续 `tool_layer.md` 负责把 `PageState` 的方法包装为 `browser__*` 工具，处理输入 schema 和输出格式。
- 后续 `extension_integration.md` 负责把 `BrowserSession` 注入线程 store 并在 `tools()` 中分发工具实例。
- 后续 `security_observability.md` 负责 `execute_raw` 的审批策略和事件收集的 tracing。
