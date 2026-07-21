# Windows 默认 shell 优先探测 bash 并持久化到配置

## Scope In / Out

### Scope In
- Windows 平台下，在每次 session 初始化时主动探测 `bash.exe` 是否可用。
- 探测范围包括：
  - `PATH` 环境变量（通过 `which::which("bash")`）。
  - 常见固定安装路径：Git Bash、MSYS2、Cygwin 等。
- 仅当 `~/.ody-code/config.toml` 中没有配置 `shell` 路径，或者已配置的 `shell` 路径为空时，才探测可用 bash。
- 探测到可用 bash 后，将其设为本次 session 的默认 shell，并写入 `~/.ody-code/config.toml` 的 `shell` 字段。
- 写入成功后，通过 TUI 信息提示告知用户已自动切换到 bash。
- 写入失败时，仍使用探测到的 bash 作为本次 shell（仅内存生效）。

### Scope Out
- 不修改 Unix / macOS 的 shell 选择逻辑。
- 不修改 `shell` 字段的 schema 或解析规则（仍接受字符串或路径）。
- 不处理 zsh / sh 等其他 shell 的 Windows 探测。
- 不迁移现有 profile 配置（`*.config.toml`），只写入基础 `~/.ody-code/config.toml`。

---

## Architecture / Design

### 组件与数据流

```text
┌─────────────────────────────────────┐
│  core/src/session/session.rs          │
│  (session init, shell resolution)     │
└──────────────┬──────────────────────┘
               │
               │ 1. Windows + no usable bash yet
               ▼
┌─────────────────────────────────────┐
│  shell-command/src/shell_detect.rs   │
│  detect_windows_bash()                │
│  check PATH + known fixed paths       │
└──────────────┬──────────────────────┘
               │
               │ 2. Some(DetectedShell { Bash, path })
               ▼
┌─────────────────────────────────────┐
│  config/src/shell_edit.rs (new)       │
│  set_user_shell(ody_home, path)       │
│  write shell = "<path>" to config.toml│
└──────────────┬──────────────────────┘
               │
               │ 3. Ok / Err
               ▼
┌─────────────────────────────────────┐
│  protocol/src/protocol.rs             │
│  SessionConfiguredEvent.user_notification │
│  (optional info message)              │
└──────────────┬──────────────────────┘
               │
               │ 4. app-server → TUI
               ▼
┌─────────────────────────────────────┐
│  tui/src/chatwidget/session_flow.rs   │
│  display user_notification as info    │
└─────────────────────────────────────┘
```

### 关键决策
- 在 `core/src/session/session.rs:887-919` 的 shell 选择逻辑中插入 Windows bash 探测分支。
  - 该分支在 `user_shell_override` 之后、且仅当 `configured_shell` 未配置或为空时才执行；若 `configured_shell` 已配置且非空，则完全跳过探测，直接解析并使用用户显式配置。
  - 原因：`user_shell_override`（CLI/测试）优先级最高；`configured_shell` 是显式配置，只要已配置且非空就必须尊重用户选择，不再回退到探测；`zsh_fork` 是特性开关，仍可作为更高优先级特性保留；仅当 `configured_shell` 未配置或为空时，才进入 Windows bash 探测分支。
- 探测到的 bash 路径以 `PathBuf` 形式写入 `shell` 字段，而不是只写 `"bash"`。
  - 这样即使 PATH 后续变化，config.toml 仍指向可用的 bash 可执行文件。

---

## Data Models

### 1. `shell-command/src/shell_detect.rs`

新增公共函数：

```rust
pub fn detect_windows_bash() -> Option<DetectedShell>;
```

返回 `Some(DetectedShell { shell_type: ShellType::Bash, shell_path: PathBuf })` 或 `None`。

### 2. `config/src/shell_edit.rs`（新文件）

```rust
pub fn set_user_shell(ody_home: &std::path::Path, shell_path: &std::path::Path) -> std::io::Result<()>;
```

行为：
- 读取 `ody_home/config.toml`；不存在则创建空 `DocumentMut`。
- 设置顶层键 `shell` 为 `shell_path` 的字符串表示（使用 `to_string_lossy`）。
- 写回文件。

### 3. `protocol/src/protocol.rs`

在 `SessionConfiguredEvent` 中新增可选字段：

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
#[ts(optional)]
pub user_notification: Option<String>,
```

### 4. `tui/src/session_state.rs`

在 `ThreadSessionState` 中新增：

```rust
pub(crate) user_notification: Option<String>,
```

### 5. `tui/src/chatwidget/session_flow.rs`

在 `on_session_configured_with_display_and_fork_parent_title` 中，Normal 显示模式下，如果 `session.user_notification` 有值，调用 `self.add_info_message(...)`。

---

## Algorithms

### A. Windows bash 探测

```text
function detect_windows_bash() -> Option<DetectedShell>:
    if not cfg!(windows):
        return None

    // 1. 信任 PATH 中的 bash.exe
    if which::which("bash") succeeds and file exists:
        return Some(DetectedShell { Bash, path })

    // 2. 常见固定路径，按安装流行程度排序
    for candidate in WINDOWS_BASH_CANDIDATES:
        if file_exists(candidate):
            return Some(DetectedShell { Bash, path: candidate })

    return None
```

候选路径列表（待最终确认）：
- `C:\Program Files\Git\bin\bash.exe` (Git Bash)
- `C:\Program Files\Git\usr\bin\bash.exe` (Git Bash alternate)
- `C:\msys64\usr\bin\bash.exe` (MSYS2 64-bit)
- `C:\msys32\usr\bin\bash.exe` (MSYS2 32-bit)
- `C:\cygwin64\bin\bash.exe` (Cygwin)
- `C:\cygwin\bin\bash.exe` (Cygwin legacy)
- `%SystemDrive%\Program Files\Git\bin\bash.exe`（如果 SystemDrive 不是 C）

### B. Session 初始化 shell 选择

修改 `core/src/session/session.rs:887-919`：

```text
if user_shell_override present:
    use override
else if configured_shell present and not empty:
    requested = PathBuf::from(configured_shell)
    resolved = detect_shell_type(requested) && get_shell(type, Some(&requested))
    if resolved:
        use resolved
    else:
        warn and fallback to default_user_shell()
else if use_zsh_fork_shell:
    ... existing zsh fork logic ...
else if windows:
    detected = detect_windows_bash()
    if detected:
        best_effort_persist_bash(ody_home, detected.shell_path)
        resolved = detected
    if resolved:
        use resolved
    else:
        default_user_shell()
else:
    default_user_shell()
```

### C. 配置持久化

```text
function best_effort_persist_bash(ody_home, bash_path):
    try:
        set_user_shell(ody_home, bash_path)
        log info: "Windows bash detected; persisted shell = <bash_path>"
        return Ok
    catch err:
        log warn: "Failed to write shell to ~/.ody-code/config.toml: {err}"
        return Err
```

### D. TUI 信息提示

```text
function on_session_configured_with_display_and_fork_parent_title(session, display, ...):
    ... existing logic ...
    if display == Normal and session.user_notification is Some(msg):
        self.add_info_message(msg, None)
```

---

## Error Handling / Degradation

| 场景 | 行为 |
|------|------|
| Windows 且 bash 存在 | 使用 bash；写入 config.toml；TUI 提示 |
| Windows 且 bash 不存在 | 走现有 `default_user_shell()`（cmd） |
| bash 存在但 config.toml 只读/写入失败 | 使用 bash；记录 warning；不中断 session；无 TUI 提示（因为持久化失败） |
| 探测到的 bash 路径无法执行 | 不应发生，因为 `detect_windows_bash` 只返回存在的文件；但 `get_shell_by_model_provided_path` 会回退 |
| 非 Windows 平台 | 探测函数返回 `None`，逻辑不变 |
| 用户显式配置了 `shell = "powershell"` | 尊重用户配置，不探测 bash，直接尝试解析并使用；若解析失败则 warning 并回退 `default_user_shell()` |

---

## Self-Review

- **最昂贵的错误决策**：在 `config.toml` 中写入的 `shell` 路径格式错误，导致后续 session 无法解析 shell。必须确保写入的是可执行文件绝对路径，且 `detect_shell_type` 能识别它（`bash.exe` 会被识别为 Bash）。
- **覆盖行为**：本次更新后，不再覆盖用户显式配置的非空 `shell`；仅当 `shell` 未配置或为空时才进行探测。设计假设表已同步更新。
- **TUI 提示链**：涉及 protocol + app-server + TUI 三个层级。如果用户希望最小改动，可以改为仅日志记录。当前按用户选择的“TUI 提示”实现。
- **测试覆盖**：需要新增 Windows 下的单元测试，包括 bash 存在/不存在、固定路径命中、PATH 命中、写入成功/失败。
- **风险**：新增 `EventMsg` 变体会影响所有客户端。如果希望降低风险，可复用 `WarningEvent` 作为折中方案。

---

## User Approval

- 用户已确认：需要写文件、探测范围包括 PATH + 常见固定路径；**显式配置非空时不再覆盖**（2026-07-21 更新）。
- 用户已确认：写入时机为 session 初始化时；写入失败仍使用 bash 内存生效。
- 用户已选择：写入成功后 TUI 提示用户。

---

## Reuse Analysis

- `shell-command/src/shell_detect.rs`：复用 `which::which` 和已有的 `file_exists` 工具函数；复用 `DetectedShell` 结构。
- `config/src/shell_edit.rs`：复用 `config/src/marketplace_edit.rs` 中 `read_or_create_document` + `toml_edit` 的编辑模式；复用 `CONFIG_TOML_FILE` 常量。
- `core/src/session/session.rs`：复用现有的 shell 选择分支和 `config.ody_home`。
- `protocol/src/protocol.rs`：复用 `SessionConfiguredEvent` 的可选字段模式（如 `thread_name`、`service_tier`）。
- TUI 层：复用 `ChatWidget::add_info_message` 和 `on_session_configured_with_display_and_fork_parent_title`。

---

## Assumptions & Unverified Items

| # | Assumption | Confidence | Impact if wrong | How to verify |
|---|------------|------------|-----------------|---------------|
| 1 | 写入 `~/.ody-code/config.toml` 的 `shell` 字段使用绝对路径，能被 `detect_shell_type` 识别为 `Bash` | high | 写入后无法解析，下回 session 回退 cmd | 在测试中调用 `detect_shell_type` 对 Git Bash 路径断言 |
| 2 | 不再覆盖用户显式配置的非空 `shell`；探测仅在配置不存在或为空时执行 | high | 用户配置被覆盖 | 本次更新后的需求；测试验证显式配置非空时不触发探测 |
| 3 | `SessionConfiguredEvent` 增加可选字段向后兼容现有 rollout/TS 生成 | medium | TS 客户端编译失败或旧 rollout 反序列化失败 | 运行 `cargo test -p ody-protocol` 和 TS 生成检查（若 CI 覆盖） |
| 4 | Git Bash / MSYS2 的 bash 接受 `-lc` 参数（`Shell::derive_exec_args` 对 Bash 使用 `-lc`） | high | bash 执行命令失败 | 在 Windows 测试环境或 Git Bash 中运行 `bash -lc "echo ok"` |
| 5 | 常见固定路径列表覆盖了大多数用户安装位置 | medium | 部分用户安装路径未命中 | 用户反馈 / 遥测；可先按常见路径实现，后续再扩展 |

