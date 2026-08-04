# Part 2 — 浏览器进程生命周期管理

## 设计原则

- **线程级隔离**：每个 Ody 线程拥有独立的 Chrome 进程 + 临时 profile，线程结束时完整清理。
- **自动发现优先**：利用 `chromiumoxide` 内置 detection 查找 Chrome/Edge；允许 `[services.browser]` 覆盖。
- **安全默认**：headless、临时 profile、禁用默认扩展、无持久 cookie/storage。
- **资源可预测**：一个线程内可创建多个 `Page`（target），但进程本身只有一个。

## Chrome 发现

复用 `chromiumoxide::detection::default_executable`：`[C:UPSTREAM]`

```rust
use chromiumoxide::detection::{default_executable, DetectionOptions};

pub fn discover_chrome(preferred: Option<&Path>) -> Result<PathBuf, BrowserControlError> {
    if let Some(path) = preferred {
        if path.exists() {
            return Ok(path.to_path_buf());
        }
        return Err(BrowserControlError::ChromeNotFound {
            searched_paths: vec![path.to_path_buf()],
        });
    }

    default_executable(DetectionOptions {
        msedge: true,
        unstable: false,
    })
    .map_err(|msg| BrowserControlError::ChromeNotFound {
        searched_paths: vec![], // 后续增强为记录所有搜索路径
    })
    .map(PathBuf::from)
}
```

发现顺序（由 chromiumoxide 实现）`[C:UPSTREAM]`：
1. `CHROME` 环境变量。
2. PATH 中可执行文件名：`chrome`、`google-chrome-stable`、`chromium`、`msedge`、`microsoft-edge` 等。
3. Windows 注册表 `App Paths\chrome.exe`。
4. 平台常见安装路径（Windows Edge、macOS `/Applications`、Linux `/opt`）。

Ody 配置可以覆盖：

```toml
[services.browser]
chrome_executable = "/usr/bin/google-chrome-stable"
```

## 启动配置

`BrowserControlConfig` 是 Ody 侧配置，映射到 `chromiumoxide::browser::BrowserConfig`：`[C:INFERRED]`

```rust
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
#[schemars(deny_unknown_fields)]
pub struct BrowserControlConfig {
    pub chrome_executable: Option<PathBuf>,
    #[serde(default = "default_headless")]
    pub headless: bool,
    #[serde(default = "default_viewport")]
    pub viewport: ViewportConfig,
    #[serde(default)]
    pub sandbox: bool,            // 默认 false：headless 场景下关闭沙箱以避免 CI 问题
    #[serde(default)]
    pub disable_extensions: bool, // 默认 true
    #[serde(default)]
    pub extra_args: Vec<String>,
    #[serde(default = "default_timeout_ms")]
    pub launch_timeout_ms: u64,
    #[serde(default = "default_timeout_ms")]
    pub command_timeout_ms: u64,
    #[serde(default = "default_max_concurrent_browsers")]
    pub max_concurrent_browsers: usize,
    #[serde(default)]
    pub external_browser_allow_sensitive: bool, // 默认 false
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq)]
pub struct ViewportConfig {
    pub width: u32,
    pub height: u32,
}

fn default_headless() -> bool { true }
fn default_viewport() -> ViewportConfig { ViewportConfig { width: 1280, height: 720 } }
fn default_timeout_ms() -> u64 { 30_000 }
fn default_max_concurrent_browsers() -> usize { 4 }
```

映射到 `chromiumoxide::BrowserConfig`：`[C:UPSTREAM]`

```rust
use chromiumoxide::browser::{BrowserConfig, HeadlessMode};

fn build_chromiumoxide_config(
    ody_cfg: &BrowserControlConfig,
    profile_dir: &Path,
    chrome_path: &Path,
) -> BrowserConfig {
    let extra_args = sanitize_extra_args(&ody_cfg.extra_args)
        .expect("extra_args already validated during config load");
    BrowserConfig::builder()
        .chrome_executable(chrome_path)
        .user_data_dir(profile_dir)
        .headless_mode(if ody_cfg.headless { HeadlessMode::New } else { HeadlessMode::False })
        .viewport(ody_cfg.viewport.into())
        .no_sandbox() // 等价于 sandbox(false)
        .args(extra_args)
        .launch_timeout(Duration::from_millis(ody_cfg.launch_timeout_ms))
        .request_timeout(Duration::from_millis(ody_cfg.command_timeout_ms))
        .ignore_https_errors()
        .build()
        .expect("valid config")
}

/// 安全过滤：拒绝会覆盖 profile、remote-debugging-port、代理、证书或沙箱的额外参数。
fn sanitize_extra_args(args: &[String]) -> Result<Vec<String>, BrowserControlError> {
    let forbidden = [
        "--user-data-dir",
        "--remote-debugging-port",
        "--proxy-server",
        "--proxy-bypass-list",
        "--ignore-certificate-errors",
        "--no-sandbox",
        "--profile-directory",
        "--app",
    ];
    for arg in args {
        if forbidden.iter().any(|f| arg.starts_with(f)) {
            return Err(BrowserControlError::NotAllowed {
                reason: format!("forbidden chrome argument: {arg}"),
            });
        }
    }
    Ok(args.to_vec())
}
```

默认启动参数（由 chromiumoxide 提供，Ody 保留并追加）：`[C:UPSTREAM]`
- `--no-first-run`
- `--no-default-browser-check`
- `--disable-default-apps`
- `--disable-background-timer-throttling`
- `--disable-renderer-backgrounding`
- `--disable-backgrounding-occluded-windows`
- `--disable-background-networking`
- `--disable-breakpad`
- `--disable-client-side-phishing-detection`
- `--disable-cast-streaming-hw-encoding`
- `--disable-features=Translate,BackForwardCache,AcceptCHFrame,MediaRouter,OptimizationHints`
- `--enable-features=NetworkService,NetworkServiceInProcess`
- `--remote-debugging-port=<port>`
- `--user-data-dir=<temp>`

Ody 强制追加（不可被用户覆盖）：`[C:INFERRED]`
- `--no-first-run`
- `--no-default-browser-check`
- `--password-store=basic`（避免使用系统 keyring）
- `--use-mock-keychain`
- `--disable-extensions`（如果 `disable_extensions: true`）
- `--disable-blink-features=AutomationControlled`（可选，降低 bot 检测）

## 临时 Profile 与数据隔离

每个线程启动时创建 `tempfile::TempDir` 作为 `user-data-dir`：`[C:INFERRED]`

```rust
pub struct BrowserSession {
    browser: Browser,
    handler_task: JoinHandle<()>,
    _profile_dir: TempDir, // 随 Session drop 自动删除
}

impl BrowserSession {
    pub async fn launch(ody_cfg: BrowserControlConfig) -> Result<Self, BrowserControlError> {
        let chrome_path = discover_chrome(ody_cfg.chrome_executable.as_deref())?;
        let profile_dir = tempfile::TempDir::with_prefix("ody-browser-")?;
        let cx_cfg = build_chromiumoxide_config(&ody_cfg, profile_dir.path(), &chrome_path);

        let (browser, handler) = Browser::launch(cx_cfg).await
            .map_err(BrowserControlError::LaunchFailed)?;

        let handler_task = tokio::spawn(async move {
            if let Err(e) = handler.await {
                tracing::error!("chromiumoxide handler exited with error: {e}");
            }
        });

        Ok(Self { browser, handler_task, _profile_dir: profile_dir })
    }
}
```

- `TempDir` 在 `BrowserSession` drop 时递归删除整个 profile 目录，保证不会污染用户默认 profile。
- 如果浏览器崩溃或进程残留，需要单独清理进程（见下文）。

## Target / Page 复用

一个线程一个 `Browser`，但一个 `Browser` 可创建多个 `Page`：`[C:UPSTREAM]`

```rust
impl BrowserSession {
    /// 为一次工具调用创建新的 Page；调用结束后应关闭以释放资源。
    pub async fn new_page(&self) -> Result<PageState, BrowserControlError> {
        let page = self.browser.new_page("about:blank").await
            .map_err(BrowserControlError::CommandFailed)?;
        PageState::new(page).await
    }

    /// 保留一个默认 Page 用于连续操作（如先 navigate 再 screenshot）。
    pub async fn default_page(&self) -> Result<PageState, BrowserControlError> {
        // 首次创建，后续 clone Page 返回
        // Page 内部是 Arc，clone 是安全的
    }
}
```

Page 复用策略：`[C:INFERRED]`

| 场景 | 行为 |
|---|---|
| `browser__navigate` | 复用当前默认 Page，navigate 到新 URL |
| `browser__screenshot` | 复用当前默认 Page |
| `browser__evaluate` | 复用当前默认 Page |
| `browser__click` | 复用当前默认 Page |
| `browser__new_page` | 创建新 Page，成为新的默认 Page |
| `browser__close_page` | 关闭当前默认 Page，回退到上一个或 about:blank |

线程 store 中保存 `BrowserSession` 和当前 `PageState`：`[C:INFERRED]`

```rust
pub struct BrowserThreadState {
    session: BrowserSession,
    current_page: PageState,
}
```

## 外部浏览器连接（BrowserUseExternal）

当 `BrowserUseExternal` feature 启用且配置提供 `connect_url` 时，不启动新进程，而是连接现有 Chrome：`[C:INFERRED]`

```rust
impl BrowserSession {
    pub async fn connect(ws_url: &str) -> Result<Self, BrowserControlError> {
        let (browser, handler) = Browser::connect(ws_url).await
            .map_err(BrowserControlError::ConnectFailed)?;
        let handler_task = tokio::spawn(handler);
        Ok(Self {
            browser,
            handler_task,
            _profile_dir: tempfile::TempDir::with_prefix("ody-browser-dummy-")?,
        })
    }
}
```

- 外部浏览器连接时**不创建**真实临时 profile；`TempDir` 仅作为占位，避免 `Option<TempDir>`。
- 外部浏览器场景下，安全隔离较弱，因此 `BrowserUseExternal` 默认启用但企业可关闭。

## 全局并发控制

使用 `tokio::sync::Semaphore` 限制同时运行的 Chrome 进程数（默认 4，可配置 `max_concurrent_browsers`）：`[C:INFERRED]`

```rust
use tokio::sync::Semaphore;
use std::sync::LazyLock;

static BROWSER_LAUNCH_SEMAPHORE: LazyLock<Semaphore> =
    LazyLock::new(|| Semaphore::new(4));

impl BrowserSession {
    pub async fn launch(ody_cfg: BrowserControlConfig) -> Result<Self, BrowserControlError> {
        let _permit = BROWSER_LAUNCH_SEMAPHORE
            .acquire()
            .await
            .map_err(|_| BrowserControlError::QuotaExceeded)?;
        // ... 后续启动逻辑
        // permit 随 BrowserSession 持有，drop 时释放配额
    }
}
```

- 当配额耗尽时，新线程的 `on_thread_start` 等待；如果等待超时（默认 30s），则本次启动失败，不暴露 browser 工具。
- 配额针对整个 app-server 进程，避免多个线程同时启动大量 Chrome 进程耗尽内存/句柄。

## 进程清理

正常路径：`[C:INFERRED]`

```rust
impl BrowserSession {
    pub async fn close(mut self) -> Result<(), BrowserControlError> {
        // 1. 关闭所有 page
        // 2. 关闭 browser
        self.browser.close().await
            .map_err(BrowserControlError::CommandFailed)?;
        // 3. 等待 handler 处理 close 响应（带 5 秒超时）
        let shutdown = tokio::time::timeout(
            Duration::from_secs(5),
            self.handler_task.as_ref(),
        );
        if shutdown.await.is_err() {
            tracing::warn!("handler did not shut down within 5s, aborting");
            self.handler_task.abort();
        }
        let _ = self.handler_task.await;
        // 4. 如果进程仍未退出，kill 并等待
        let _ = self.browser.wait().await;
        // 5. 释放全局并发配额
        drop(self._launch_permit);
        // 6. TempDir 随 self drop 删除
        Ok(())
    }
}
```

异常路径（线程结束时可能未调用 close）：`[C:INFERRED]`

```rust
impl Drop for BrowserSession {
    fn drop(&mut self) {
        // 同步 best-effort：kill 主进程、abort handler、释放配额、尝试清理 profile。
        if let Some(child) = self.browser.child() {
            let _ = child.kill();
        }
        self.handler_task.abort();
        drop(self._launch_permit);
        // profile 清理：TempDir RAII 会尝试删除；Windows 句柄未释放时记录路径告警
        #[cfg(windows)]
        if let Some(path) = self._profile_dir.path().to_str() {
            tracing::warn!("profile directory may need manual cleanup: {path}");
        }
    }
}
```

Windows 特有问题：`[C:INFERRED]`
- Chrome 启动后可能产生多个子进程（renderer、gpu 等）。
- `Browser::close()` 通常能级联关闭；若进程残留，使用 `taskkill /T /F /PID <pid>` 作为 fallback。
- 正常关闭路径中也调用 `taskkill /T /F /PID` 兜底，确保子进程不残留。
- profile 目录删除失败时，最多重试 3 次（间隔 100ms），仍失败则记录 `error` 并保留路径到 tracing 告警（供后续清理）。
- 需要把 `Child` 的 PID 暴露出来，或在 `BrowserSession` 中保存 `u32` pid。

## 发现与启动的错误处理

| 场景 | 行为 | 重试 |
|---|---|---|
| Chrome 未找到 | 返回 `BrowserControlError::ChromeNotFound`，带搜索路径 | 否 |
| 启动超时 | 返回 `LaunchFailed` | 否（避免反复启动失败进程） |
| 端口冲突 | chromiumoxide 使用 port 0 让 OS 分配；冲突概率低 | 否 |
| 进程崩溃 | 在 `PageState` 操作失败时检测，触发 `PageCrashed` | 是：可重新 `new_page` |
| 连接外部浏览器失败 | `ConnectFailed` | 否 |

## 配置示例

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

## 与 CDP 传输层的接口

Part 1 定义的 `BrowserSession` 由本部分实现 `launch`/`connect`/`close`；`PageState` 由 Part 1 消费其 `page` 字段执行 CDP 命令。`[C:INFERRED]`

```rust
// 来自 Part 1
pub struct BrowserSession {
    browser: chromiumoxide::Browser,
    handler_task: JoinHandle<()>,
    _profile_dir: TempDir,
}

// 本部分负责的方法
impl BrowserSession {
    pub async fn launch(cfg: BrowserControlConfig) -> Result<Self, BrowserControlError>;
    pub async fn connect(ws_url: &str) -> Result<Self, BrowserControlError>;
    pub async fn close(self) -> Result<(), BrowserControlError>;
    pub async fn new_page(&self) -> Result<PageState, BrowserControlError>;
}
```

## 关键验证项

1. **Windows 路径发现**：在 Windows 开发机上运行 `discover_chrome(None)`，验证能命中 Chrome/Edge 安装路径。
2. **临时 profile 清理**：启动后结束线程，验证 `TempDir` 被删除且 `user-data-dir` 不存在残留。
3. **进程清理**：在 Linux/macOS/Windows 上启动并关闭 100 次，检查无僵尸 Chrome 进程残留。
4. **多 page 创建**：一个线程内创建 5 个 page，验证各自 navigate 到不同 URL 互不干扰。
