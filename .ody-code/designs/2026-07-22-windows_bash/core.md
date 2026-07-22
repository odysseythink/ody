# Part 1: Session 启动路径中的 shell 选择流程变更

## 目标

使 `core/src/session/session.rs` 中 shell 选择逻辑能够消费 config 层探测并持久化后的结果，以及将自动切换通知透传给 `SessionConfiguredEvent`。

由于探测与持久化下沉到 `config` crate，core 的改动以**读取最终值**和**转发通知**为主，无需直接调用 `detect_windows_bash`。

## 当前状态（来源确认）

`core/src/session/session.rs:887-919` 的当前选择顺序：

```rust
let default_shell = if let Some(user_shell_override) = session_configuration.user_shell_override.clone() {
    user_shell_override
} else if let Some(configured_shell) = config.shell.as_deref() {
    let requested = PathBuf::from(configured_shell);
    match shell::detect_shell_type(&requested)
        .and_then(|shell_type| shell::get_shell(shell_type, Some(&requested)))
    {
        Some(resolved) => resolved,
        None => {
            warn!("config `shell = \"{configured_shell}\"` is not a recognized or installed shell; falling back to the platform default shell");
            shell::default_user_shell()
        }
    }
} else if use_zsh_fork_shell { ... } else {
    shell::default_user_shell()
};
```

- `shell::default_user_shell()` 在 Windows 上直接返回 `cmd.exe`（`shell-command/src/shell_detect.rs:277-278`）。
- `detect_windows_bash` 当前仅在测试中被调用，生产路径未使用（`shell-command/src/shell_detect.rs:407`）。

## 需要的变更

### 1. `config` 层返回的 shell 信息

`config` crate 在加载配置后需要向 core 暴露两项新信息：

- `config.shell`：最终用于当前会话的 shell 字符串或路径。可能已经被探测并写入 `config.toml`。
- `shell_auto_detected: bool`：标记该 shell 是否是本次由自动探测得到的（且已持久化）。core 需要这个标记来生成通知，而不是通过比较字符串值猜测。

建议新增类型（在 `config` crate 中）：

```rust
pub struct ShellConfigResult {
    /// 最终解析出的 shell 值，等价于 ConfigToml.shell。
    pub shell: Option<String>,
    /// 若本次探测到了 bash 并将其写入 config.toml，则为 true。
    pub auto_detected_and_persisted: bool,
}
```

`ConfigLayerStack` 或 `ConfigToml` 提供一个方法：

```rust
impl ConfigLayerStack {
    pub fn shell_with_detection_metadata(&self) -> ShellConfigResult { ... }
}
```

### 2. core 中的 shell 选择分支

保持现有选择顺序，但将 `config.shell` 的来源从 `config.shell.as_deref()` 改为调用上述方法得到的 `shell` 字段。`auto_detected_and_persisted` 不需要改变 shell 值，只用于后续通知。

`core/src/session/session.rs:887-919` 的伪代码：

```rust
let shell_result = config.shell_with_detection_metadata();
let default_shell = if let Some(user_shell_override) = session_configuration.user_shell_override.clone() {
    user_shell_override
} else if let Some(configured_shell) = shell_result.shell.as_deref() {
    let requested = PathBuf::from(configured_shell);
    match shell::detect_shell_type(&requested)
        .and_then(|shell_type| shell::get_shell(shell_type, Some(&requested)))
    {
        Some(resolved) => resolved,
        None => {
            warn!("config `shell = \"{configured_shell}\"` is not a recognized or installed shell; falling back to the platform default shell");
            shell::default_user_shell()
        }
    }
} else if use_zsh_fork_shell { ... } else {
    shell::default_user_shell()
};

let shell_notification = if shell_result.auto_detected_and_persisted {
    Some(UserNotification {
        level: NotificationLevel::Info,
        message: format!("已自动检测到 Windows bash 并将其设为默认 shell：{}", default_shell.shell_path.display()),
    })
} else {
    None
};
```

### 3. 透传通知到 SessionConfiguredEvent

`SessionConfiguredEvent` 新增字段（详见 `protocol.md`）：

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub user_notification: Option<UserNotification>,
```

在 `core/src/session/session.rs` 中构造 `SessionConfiguredEvent` 的位置（约在 `session.rs:1025` 附近，根据当前代码需要定位），将 `shell_notification` 填入该字段。

```rust
SessionConfiguredEvent {
    ...
    user_notification: shell_notification,
}
```

## 接口与调用点

| 调用点 | 当前代码 | 变更 |
|---|---|---|
| `core/src/session/session.rs:891` | `config.shell.as_deref()` | 改为 `config.shell_with_detection_metadata().shell.as_deref()` |
| `core/src/session/session.rs` 构造 `SessionConfiguredEvent` | 无 `user_notification` | 新增字段赋值 |

## 错误处理

| 场景 | 行为 |
|---|---|
| `config` 层探测成功但写入失败 | core 仍然使用探测到的 shell，但不发送通知。写入失败由 config 层记录 warn。 |
| 探测到的 shell 路径在 core 中无法被 `get_shell` 识别 | 回退到 `shell::default_user_shell()`（Windows 下即 `cmd.exe`），不发送通知。 |
| 用户配置的 shell 无效（文件不存在或不被识别） | core 回退到 `shell::default_user_shell()`，记录 warn。config 层不验证、不修改用户配置。 |
| `user_shell_override` 存在 | 完全跳过 config shell 和探测逻辑，不发送通知。 |
| `zsh fork` 启用 | 保持现有分支，不触发 bash 探测。 |

## 测试断言

- 在 Windows 测试环境中，mock `FsChecker` 返回固定路径 bash，验证 `shell_with_detection_metadata()` 返回 `auto_detected_and_persisted = true` 且 `shell` 为对应路径。
- 验证 core 中 `SessionConfiguredEvent.user_notification` 在自动探测成功后非空，且消息包含 bash 路径。
- 验证已有 `shell` 配置时，`auto_detected_and_persisted = false` 且 `user_notification = None`。
- 验证非 Windows 平台 `shell_with_detection_metadata()` 始终返回 `auto_detected_and_persisted = false`。

## 依赖

- `config` crate 提供 `ShellConfigResult` / `shell_with_detection_metadata()`（见 `config.md`）。
- `protocol` crate 提供 `SessionConfiguredEvent.user_notification`（见 `protocol.md`）。
