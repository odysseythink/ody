# Windows 默认 shell 优先探测 bash 并持久化到配置（修订版）

## Scope In / Out

### Scope In [C:USER]
- Windows 平台下，在每次 session 初始化时主动探测 `bash.exe` 是否可用。
- 探测范围包括：
  - Windows 注册表（Git for Windows 安装路径 `HKLM/HKCU\SOFTWARE\GitForWindows\InstallPath`）。
  - `PATH` 环境变量（通过 `which::which("bash")`），但排除 WSL 入口点。
  - 常见固定安装路径：Git Bash、MSYS2、Cygwin、用户级安装目录等。
- 探测时验证候选文件可执行性（通过 `--version` 或至少文件存在且非空）。
- 只要探测到可用 bash，就将其设为本次 session 的默认 shell。
- 首次探测到 bash 时，以“自动探测”模式写入 `~/.ody-code/config.toml`：`shell = <abs_path>` + `shell_auto_detect = true`。
- 在 `shell_auto_detect = true` 模式下，每次 session 初始化都会重新验证并更新 `shell`；若用户显式删除 `shell_auto_detect` 或设为 `false`，则尊重其 `shell` 配置不再覆盖。
- 写入成功后，通过 TUI 信息提示告知用户已自动切换到 bash（仅首次或路径发生变化时）。
- 写入失败时，仍使用探测到的 bash 作为本次 shell（仅内存生效）。

### Scope Out [C:USER]
- 不修改 Unix / macOS 的 shell 选择逻辑。
- 不修改 `shell` 字段的 schema 或解析规则（仍接受字符串或路径）。
- 不处理 zsh / sh 等其他 shell 的 Windows 探测。
- 不迁移现有 profile 配置（`*.config.toml`），只写入基础 `~/.ody-code/config.toml`。
- 不处理非 Windows 平台的 `shell_auto_detect` 字段（该字段在 Windows 之外无意义，解析时忽略）。

---

## Architecture / Design

### 组件与数据流 [C:INFERRED]

```text
┌──────────────────────────────────────────┐
│  core/src/session/session.rs:887-919      │
│  (session init, shell resolution)          │
│  read shell_auto_detect from config        │
└──────────────┬─────────────────────────┘
               │
               │ 1. Windows + (no configured shell OR shell_auto_detect == true)
               ▼
┌──────────────────────────────────────────┐
│  shell-command/src/shell_detect.rs        │
│  detect_windows_bash() [C:NEW]            │
│  registry → PATH (excluding WSL) → fixed paths│
│  validate executable                       │
└──────────────┬─────────────────────────┘
               │
               │ 2. Some(DetectedShell { Bash, path })
               ▼
┌──────────────────────────────────────────┐
│  config/src/shell_edit.rs (new)           │
│  set_user_shell(ody_home, path, auto_detect)
│  atomic write shell = "<path>"             │
│         shell_auto_detect = true           │
└──────────────┬─────────────────────────┘
               │
               │ 3. Ok / Err → user_notification
               ▼
┌──────────────────────────────────────────┐
│  protocol/src/protocol.rs:3689              │
│  SessionConfiguredEvent.user_notification   │
│  (optional info message) [C:NEW]            │
└──────────────┬─────────────────────────┘
               │
               │ 4. app-server converts to thread response
               ▼
┌──────────────────────────────────────────┐
│  app-server-protocol/src/protocol/v2/     │
│  thread.rs ThreadStartResponse/Resume      │
│  Response.user_notification [C:NEW]         │
│  ThreadForkResponse.user_notification       │
│  always None in this feature [C:INFERRED]   │
└──────────────┬─────────────────────────┘
               │
               │ 5. TUI app-server session converts to state
               ▼
┌──────────────────────────────────────────┐
│  tui/src/app_server_session.rs:1589-1642   │
│  thread_session_state_from_thread_response   │
│  carries user_notification to ThreadSessionState [C:NEW]
└──────────────┬─────────────────────────┘
               │
               │ 6. ChatWidget handles SessionConfigured
               ▼
┌──────────────────────────────────────────┐
│  tui/src/chatwidget/session_flow.rs:6-60    │
│  on_session_configured_with_display_...     │
│  display user notification as info [C:NEW]  │
└──────────────────────────────────────────┘
```

### 关键决策 [C:USER]
- 在 `core/src/session/session.rs:887-919` 的 shell 选择逻辑中插入 Windows bash 探测分支。
  - 该分支在 `user_shell_override` 之后、在 `configured_shell` 与 `use_zsh_fork_shell` 之前执行。
  - 优先级：
    1. `user_shell_override`（CLI/测试）最高。
    2. 若 `shell_auto_detect == true` 或用户未配置 `shell`，则执行 Windows bash 探测。
    3. 用户显式 `configured_shell`（且 `shell_auto_detect != true`）保留。
    4. `use_zsh_fork_shell` 特性开关。
    5. `default_user_shell()` 回退。
- 探测到的 bash 路径以绝对 `PathBuf` 写入 `shell` 字段，但仅在 `shell_auto_detect = true` 时允许后续 session 重新探测并更新。用户可通过删除 `shell_auto_detect` 或设为 `false` 来固定自己的选择。

### 平台与兼容层决策 [C:INFERRED]
- 从 `SessionConfiguredEvent` 到 TUI 的信息提示需要经过 `app-server-protocol` 的线程响应结构（`ThreadStartResponse`、`ThreadResumeResponse`），因为 TUI 通过 app-server 会话 API 获取 session 状态。
- `ThreadForkResponse` 也添加 `user_notification` 字段以保持协议对称，但当前功能下其值始终为 `None`；fork 操作不应重复触发 bash 探测通知。

---

## Data Models

### 1. `config` crate 配置层 [C:INFERRED]

在 `ConfigToml` 中新增可选布尔字段：

```rust
/// If true, Ody is allowed to auto-detect and overwrite the shell on each session init.
#[serde(default, skip_serializing_if = "Option::is_none")]
#[ts(optional)]
pub shell_auto_detect: Option<bool>,
```

### 2. `shell-command/src/shell_detect.rs` [C:INFERRED]

使用条件编译：

```rust
#[cfg(target_os = "windows")]
pub fn detect_windows_bash() -> Option<DetectedShell>;

#[cfg(not(target_os = "windows"))]
pub fn detect_windows_bash() -> Option<DetectedShell> {
    None
}
```

新增私有常量：

```rust
#[cfg(target_os = "windows")]
const WINDOWS_BASH_CANDIDATES: &[&str] = &[
    r"C:\Program Files\Git\bin\bash.exe",
    r"C:\Program Files\Git\usr\bin\bash.exe",
    r"C:\msys64\usr\bin\bash.exe",
    r"C:\msys32\usr\bin\bash.exe",
    r"C:\cygwin64\bin\bash.exe",
    r"C:\cygwin\bin\bash.exe",
    r"%LOCALAPPDATA%\Programs\Git\bin\bash.exe",
    r"%SystemDrive%\Program Files\Git\bin\bash.exe",
];
```

### 3. `config/src/shell_edit.rs`（新文件） [C:INFERRED]

```rust
pub fn set_user_shell(
    ody_home: &std::path::Path,
    shell_path: &std::path::Path,
    auto_detect: bool,
) -> std::io::Result<()>;
```

行为：
- 读取 `ody_home/config.toml`；不存在则创建空 `DocumentMut`。
- 设置顶层键 `shell` 为 `shell_path` 的字符串表示（使用 `to_string_lossy`）。
- 设置 `shell_auto_detect = auto_detect`。
- 原子写入：写入临时文件 `config.toml.tmp` 后通过 `std::fs::rename` 替换。

### 4. `protocol/src/protocol.rs:3689` [C:INFERRED]

在 `SessionConfiguredEvent` 中新增可选字段：

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
#[ts(optional)]
pub user_notification: Option<String>,
```

### 5. `app-server-protocol/src/protocol/v2/thread.rs` [C:INFERRED]

在 `ThreadStartResponse`、`ThreadResumeResponse`、`ThreadForkResponse` 中各新增：

```rust
/// Optional user-facing notification to display on session init.
/// For ThreadForkResponse this should always be None in the current feature scope.
#[serde(default, skip_serializing_if = "Option::is_none")]
#[ts(optional)]
pub user_notification: Option<String>,
```

### 6. `tui/src/session_state.rs:30` [C:INFERRED]

在 `ThreadSessionState` 中新增：

```rust
pub(crate) user_notification: Option<String>,
```

### 7. `tui/src/chatwidget/session_flow.rs:6` [C:INFERRED]

在 `on_session_configured_with_display_and_fork_parent_title` 中，Normal 显示模式下，如果 `session.user_notification` 有值，调用 `self.add_info_message(...)`。

---

## Algorithms

### A. Windows bash 探测 [C:INFERRED]

```text
#[cfg(target_os = "windows")]
function detect_windows_bash() -> Option<DetectedShell>:
    // 1. 注册表：Git for Windows 安装路径
    if registry HKLM\SOFTWARE\GitForWindows\InstallPath exists:
        candidate = InstallPath + "\bin\bash.exe"
        if is_executable_bash(candidate):
            return Some(DetectedShell { Bash, candidate })
    if registry HKCU\SOFTWARE\GitForWindows\InstallPath exists:
        candidate = InstallPath + "\bin\bash.exe"
        if is_executable_bash(candidate):
            return Some(DetectedShell { Bash, candidate })

    // 2. PATH，但排除 WSL 入口
    if which::which("bash") succeeds and path != wsl_bash_path():
        if is_executable_bash(path):
            return Some(DetectedShell { Bash, path })

    // 3. 常见固定路径，按安装流行程度排序
    for candidate in WINDOWS_BASH_CANDIDATES:
        expand environment variables in candidate
        if is_executable_bash(candidate):
            return Some(DetectedShell { Bash, path: candidate })

    return None

function is_executable_bash(path) -> bool:
    if not file_exists(path): return false
    if file_size(path) == 0: return false
    try:
        output = Command::new(path).arg("--version").output()
        return output.status.success() and output.stdout contains "bash"
    catch:
        return false

function wsl_bash_path() -> PathBuf:
    system_root = env("SystemRoot").unwrap_or("C:\\Windows")
    return PathBuf::from(system_root).join("System32").join("bash.exe")
```

候选路径列表（已排序） [C:INFERRED]：
- 注册表 `HKLM\SOFTWARE\GitForWindows\InstallPath` + `\bin\bash.exe`
- 注册表 `HKCU\SOFTWARE\GitForWindows\InstallPath` + `\bin\bash.exe`
- `PATH` 中的 `bash.exe`（排除 `%SystemRoot%\System32\bash.exe`）
- `C:\Program Files\Git\bin\bash.exe` (Git Bash)
- `C:\Program Files\Git\usr\bin\bash.exe` (Git Bash true executable)
- `C:\msys64\usr\bin\bash.exe` (MSYS2 64-bit)
- `C:\msys32\usr\bin\bash.exe` (MSYS2 32-bit)
- `C:\cygwin64\bin\bash.exe` (Cygwin)
- `C:\cygwin\bin\bash.exe` (Cygwin legacy)
- `%LOCALAPPDATA%\Programs\Git\bin\bash.exe` (user-level Git install)
- `%SystemDrive%\Program Files\Git\bin\bash.exe`（如果系统盘不是 C）

### B. Session 初始化 shell 选择 [C:USER]

修改 `core/src/session/session.rs:887-919`：

```text
let shell_auto_detect = config.shell_auto_detect.unwrap_or(false);

if user_shell_override present:
    use override
else if cfg!(windows) and (configured_shell.is_none() or shell_auto_detect):
    detected = detect_windows_bash()
    if detected:
        // 验证 configured_shell 若存在则失效，需要清除并重新探测
        if configured_shell.is_some() and not is_still_usable(configured_shell):
            detected = detect_windows_bash() // 重新探测，可能返回新的路径或 None
        if let Some(detected) = detected:
            (persist_ok, user_notification) = best_effort_persist_bash(ody_home, detected.shell_path)
            resolved = detected
        else if configured_shell.is_some() and is_still_usable(configured_shell):
            resolved = configured_shell
        else:
            warn and fallback to default_user_shell()
    else if configured_shell.is_some() and shell_auto_detect and not is_still_usable(configured_shell):
        // 自动探测模式下原路径已失效，清除配置并回退
        clear_shell_auto_detect(ody_home) best_effort
        warn and fallback to default_user_shell()
    else if configured_shell.is_some():
        resolved = configured_shell
    else:
        fallback to default_user_shell()
else if configured_shell present:
    // 用户显式配置且 shell_auto_detect 为 false/None，或非 Windows
    requested = PathBuf::from(configured_shell)
    match shell::detect_shell_type(&requested).and_then(|t| shell::get_shell(t, Some(&requested))) {
        Some(resolved) => resolved,
        None => {
            warn!("config `shell = \"{configured_shell}\"` is not recognized or installed; falling back")
            shell::default_user_shell()
        }
    }
else if use_zsh_fork_shell:
    ... existing zsh fork logic ...
else:
    default_user_shell()
```

### C. 配置持久化与自愈 [C:INFERRED]

```text
function best_effort_persist_bash(ody_home, bash_path) -> (bool, Option<String>):
    try:
        set_user_shell(ody_home, bash_path, auto_detect = true)
        log info: "Windows bash detected; persisted shell = <bash_path> (auto-detect)"
        return (true, Some("Windows bash detected at <bash_path>; shell preference saved. To keep your current choice, remove shell_auto_detect from ~/.ody-code/config.toml."))
    catch err:
        log warn: "Failed to write shell to ~/.ody-code/config.toml: {err}"
        return (false, None)

function clear_shell_auto_detect(ody_home) -> std::io::Result<()>:
    // 读取 config.toml，移除 shell_auto_detect 键（保留 shell 不动或同时移除）
    // 原子写回

function is_still_usable(shell_path) -> bool:
    return is_executable_bash(PathBuf::from(shell_path))
```

### D. 通知传递链 [C:INFERRED]

```text
// core/src/session/session.rs
let mut event = SessionConfiguredEvent { ... };
event.user_notification = user_notification; // Some(msg) only when persisted successfully
send(event);

// app-server/src/request_processors/thread_processor.rs
// 在构造 ThreadStartResponse / ThreadResumeResponse 时
response.user_notification = session_configured.user_notification;
// 构造 ThreadForkResponse 时始终设为 None

// tui/src/app_server_session.rs:1589-1642
function thread_session_state_from_thread_response(..., user_notification: Option<String>):
    ...
    return ThreadSessionState {
        ...,
        user_notification,
    };

// tui/src/chatwidget/session_flow.rs:6-60
function on_session_configured_with_display_and_fork_parent_title(session, display, ...):
    ... existing logic ...
    if display == Normal and session.user_notification is Some(msg):
        self.add_info_message(msg, None)
        log::info!(displayed_user_notification = msg)
```

---

## Error Handling / Degradation

| 场景 | 行为 |
|------|------|
| Windows 且 bash 存在 | 使用 bash；原子写入 config.toml（`shell_auto_detect = true`）；TUI 提示（仅首次/路径变化） [C:USER] |
| Windows 且 bash 不存在 | 走现有 `default_user_shell()`（cmd）；若 `shell_auto_detect = true` 则清除该标记 [C:INFERRED] |
| bash 存在但 config.toml 写入失败 | 使用 bash；记录 warning；不中断 session；无 TUI 提示（因为持久化失败） [C:USER] |
| 探测到的 bash 路径无法执行 | 跳过该候选，继续探测；全部失败则回退 cmd [C:INFERRED] |
| 写入的绝对路径后续失效 | 若 `shell_auto_detect = true`，下次 session 重新探测并更新；若 `false` 则按用户显式配置处理（可能解析失败回退 cmd） [C:INFERRED] |
| 非 Windows 平台 | 探测函数返回 `None`，逻辑不变 [C:INFERRED] |
| 用户显式配置了 `shell = "powershell"` 且 `shell_auto_detect = false/None | 尊重用户选择，不覆盖 [C:INFERRED] |
| 用户显式配置了 `shell = "powershell"` 且 `shell_auto_detect = true` | 允许覆盖，因为用户处于自动探测模式 [C:INFERRED] |
| 并发 session 写入 config.toml | 原子写临时文件 + rename 保证文件不会损坏；last-writer-wins 语义可接受 [C:INFERRED] |
| 旧客户端/旧 rollout 收到带 user_notification 的响应 | 可选字段可安全忽略 [C:INFERRED] |
| WSL bash 在 PATH 中 | 排除 `%SystemRoot%\System32\bash.exe`，继续探测其他候选 [C:INFERRED] |

---

## Self-Review

- **最昂贵的错误决策**：在 `config.toml` 中写入的 `shell` 路径格式错误，导致后续 session 无法解析 shell。必须确保写入的是可执行文件绝对路径，且 `detect_shell_type` 能识别它（`bash.exe` 会被识别为 Bash）。
- **覆盖行为的约束**：用户原始要求“需要覆盖”，但审计指出这会破坏配置契约。修订版引入 `shell_auto_detect = true` 标记：首次自动探测时设置该标记，后续仅在该标记为 true 时允许重新探测和覆盖；用户删除该标记即可固定自己的选择。这样既满足“自动探测优先”的需求，又保留用户退出机制。
- **原子写入与自愈**：`set_user_shell` 必须采用 write-to-temp + rename 的原子写入，避免文件损坏。同时加入路径失效自愈：若 `shell_auto_detect = true` 且已写入的 `shell` 路径不再可用，清除标记并重新探测。
- **WSL 排除**：PATH 探测必须排除 `%SystemRoot%\System32\bash.exe`，避免将 WSL 入口选为 Windows 原生 shell。
- **可执行性验证**：`detect_windows_bash` 不能只看文件存在，必须验证 `bash --version` 成功，避免空文件或损坏文件被选中。
- **通知传递链完整性**：`SessionConfiguredEvent` → `ThreadStartResponse/ThreadResumeResponse` → `ThreadSessionState` → `add_info_message` 四个层级必须显式传递。`ThreadForkResponse` 始终设为 `None`。建议为每个转换点增加单元测试。
- **注册表探测**：优先查询 `HKLM/HKCU\SOFTWARE\GitForWindows\InstallPath`，覆盖自定义安装路径。
- **TUI 提示时机**：仅在持久化成功时显示通知，避免用户看到“已保存”但实际写入失败的情况。通知文案包含如何退出自动探测的提示。
- **条件编译**：使用 `#[cfg(target_os = "windows")]` 编译 `detect_windows_bash`，非 Windows 平台提供返回 `None` 的 stub。

---

## User Approval

- 用户已确认：需要写文件、需要覆盖、探测范围包括 PATH + 常见固定路径。
- 用户已确认：写入时机为 session 初始化时；写入失败仍使用 bash 内存生效。
- 用户已选择：写入成功后 TUI 提示用户。
- 修订版新增：引入 `shell_auto_detect` 标记以区分自动探测结果与用户显式配置，避免不可逆地破坏用户偏好 [C:INFERRED]。

---

## Reuse Analysis

- `shell-command/src/shell_detect.rs`：复用 `which::which`（workspace 依赖，已在 crate 内使用）和已有的 `DetectedShell` 结构；复用 `file_exists` 工具函数（如存在）。新增注册表查询可使用 `windows-registry` crate 或 `winreg` crate（需评估是否已存在于 workspace 或是否使用 `winapi`）。
- `config/src/shell_edit.rs`：复用 `config/src/marketplace_edit.rs` 中 `read_or_create_document` + `toml_edit` 的编辑模式；复用 `CONFIG_TOML_FILE` 常量。原子写入模式可参考 `mcp_edit.rs` 或 `plugin_edit.rs`（需确认是否已有原子写入实现）。
- `core/src/session/session.rs`：复用现有的 shell 选择分支、`config.ody_home` 和新的 `config.shell_auto_detect` 字段。
- `protocol/src/protocol.rs`：复用 `SessionConfiguredEvent` 的可选字段模式（如 `thread_name`、`service_tier`）。
- `app-server-protocol/src/protocol/v2/thread.rs`：复用 `ThreadStartResponse` / `ThreadResumeResponse` / `ThreadForkResponse` 的 `#[serde(default, skip_serializing_if = "Option::is_none")]` + `#[ts(optional)]` 模式。
- `app-server/src/request_processors/thread_processor.rs`：复用 `SessionConfiguredEvent` 解构和响应构造逻辑；确保 fork 路径不传递 `user_notification`。
- `tui/src/app_server_session.rs`：复用 `thread_session_state_from_thread_response` 的构造流程；需扩展参数列表传递 `user_notification`。
- `tui/src/session_state.rs`：复用 `ThreadSessionState` 结构；需更新测试辅助构造（`history_cell/tests.rs:535`、`app/thread_events.rs:359`、`app/config_persistence.rs:1331` 等）。
- `tui/src/chatwidget/session_flow.rs`：复用 `ChatWidget::add_info_message`（`tui/src/chatwidget.rs:1505`）和 `on_session_configured_with_display_and_fork_parent_title`。

---

## Assumptions & Unverified Items

| # | Assumption | Confidence | Impact if wrong | How to verify |
|---|------------|------------|-----------------|---------------|
| 1 | 写入 `~/.ody-code/config.toml` 的 `shell` 字段使用绝对路径，能被 `detect_shell_type` 识别为 `Bash` | high | 写入后无法解析，下回 session 回退 cmd | 在测试中调用 `detect_shell_type` 对 Git Bash 路径断言 |
| 2 | `shell_auto_detect` 字段可被 `config` crate 解析且默认不影响现有配置 | high | 旧配置无法解析或自动探测行为异常 | 在 `config` loader 测试中添加 `shell_auto_detect` 字段解析测试 |
| 3 | `SessionConfiguredEvent` 和 app-server-protocol 响应增加可选字段向后兼容现有 rollout/TS 生成 | medium | TS 客户端编译失败或旧 rollout 反序列化失败 | 运行 `cargo test -p ody-protocol`、`cargo test -p ody-app-server-protocol` 和 TS 生成检查 |
| 4 | Git Bash / MSYS2 的 bash 接受 `-lc` 参数（`Shell::derive_exec_args` 对 Bash 使用 `-lc`） | medium | bash 执行命令失败 | 在 Windows 测试环境或 Git Bash 中运行 `bash -lc "echo ok"`；优先使用 `usr/bin/bash.exe` 而非 `bin/bash.exe` 启动器 |
| 5 | 常见固定路径 + 注册表覆盖了大多数用户安装位置 | medium | 部分用户安装路径未命中 | 用户反馈 / 遥测；注册表探测优先于固定路径 |
| 6 | `user_notification` 能在 `SessionConfiguredEvent` → app-server thread response → `ThreadSessionState` 的完整链路中传递，不丢失 | medium | TUI 不显示信息提示 | 在 app-server 和 TUI 转换函数中显式传递字段，并增加传递链测试 |
| 7 | `add_info_message` 在 session 初始化阶段调用不会干扰 TUI 初始状态 | medium | 信息提示未显示或与欢迎消息冲突 | 在 TUI 测试或手动验证中检查 Normal 显示模式下的消息顺序 |
| 8 | 原子写入（write-to-temp + rename）在 Windows 上可用且不破坏文件权限/ACL | medium | 配置写入失败或文件损坏 | 在 Windows 测试环境验证；失败时回退到普通写入并记录 warning |
| 9 | 注册表查询 `HKLM/HKCU\SOFTWARE\GitForWindows\InstallPath` 在大多数 Git for Windows 安装中存在 | medium | 自定义安装路径用户无法被注册表探测命中 | 在 Windows 测试环境验证；同时保留 PATH 和固定路径作为后备 |
| 10 | WSL 的 `bash.exe` 排除逻辑不会误排除其他合法的 `System32\bash.exe` | high | 合法 bash 被排除 | 该路径是 WSL 专属入口，非 WSL 不会在此安装 bash |
| 11 | `bash --version` 验证在所有目标 bash 上成功且不会显著增加启动延迟 | medium | session 初始化变慢或验证失败 | 性能测试；超时阈值（如 500ms） |
