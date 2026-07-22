# Part 2: Config crate 的探测、持久化与写入

## 目标

在 `config` crate 中完成 Windows 平台下 bash 的自动探测、对 `~/.ody-code/config.toml` 的持久化，并向 core 层暴露 `shell` 最终值与是否由本次探测持久化生成的元数据。

## 当前状态（来源确认）

- `config/src/config_toml.rs:310`：`ConfigToml.shell` 是 `Option<String>`。
- `config` crate 目前没有依赖 `ody-shell-command`（`config/Cargo.toml`）。
- 已通过 `cargo tree -p ody-shell-command --invert -e normal` 验证：`ody-shell-command` 没有正常依赖任何其他 crate，因此 `config` 新增对 `ody-shell-command` 的依赖不会引入循环依赖。
- `config` 已有 `mcp_edit.rs` 和 `plugin_edit.rs` 对 `config.toml` 进行原子写的模式：读取 `DocumentMut` → 修改 → `write_atomically`（`plugin_edit.rs:74`）或 `fs::write`（`mcp_edit.rs:95`）。
- `shell-command/src/shell_detect.rs:407` 提供了 `detect_windows_bash(fs: &dyn FsChecker)` 和 `WindowsBashDetection`；`FsChecker` / `RealFsChecker` 已抽象好测试。该函数会先检查 `is_file` 再调用 `bash --version`，不存在路径不会触发 1 秒超时。

## 设计选择

### 方案 A：config 依赖 shell-command（推荐但需谨慎）

在 `config/Cargo.toml` 新增 `ody-shell-command = { workspace = true }`，在 config 加载流程中调用 `ody_shell_command::shell_detect::detect_windows_bash`。

**优点**：
- 复用现有 `WindowsBashDetection` 和 `FsChecker` 抽象。
- 不需要移动测试或复制逻辑。

**缺点**：
- 配置层依赖 shell 执行层，方向性不够干净。
- 如果未来 `shell-command` 依赖 `config`（目前没有），会形成循环依赖。

### 方案 B：将探测逻辑迁移到 config crate

将 `detect_windows_bash`、`FsChecker`、`WindowsBashDetection` 等从 `shell-command/src/shell_detect.rs` 移动到 `config/src/shell_auto_detect.rs`，`shell-command` 保留 `get_shell` / `default_user_shell` 等执行时逻辑。

**优点**：
- 配置层完全自包含，不依赖 shell 执行层。
- 更清晰的依赖方向。

**缺点**：
- 需要移动已有测试和常量。
- 工作量大，影响 `shell-command` 的公开 API（如果该函数已公开给外部）。

### 推荐

**方案 A** [C:INFERRED]：在 `config` crate 中直接复用 `shell-command` 的探测函数。已通过 `cargo tree -p ody-shell-command --invert` 验证 `ody-shell-command` 无反向依赖，新增 `ody-shell-command` 依赖不会引入循环。这是最小改动方案。若后续出现循环依赖，可回退到方案 B。

## 新增模块：config/src/shell_auto_detect.rs

### 数据类型

```rust
use std::path::PathBuf;

/// 由 config 层返回给 core 的 shell 配置结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellConfigResult {
    /// 最终用于当前会话的 shell 值；可能来自用户配置，也可能来自自动探测。
    pub shell: Option<String>,
    /// 本次 session 启动时是否由自动探测写入 config.toml 并命中固定路径。
    pub auto_detected_and_persisted: bool,
}

impl ShellConfigResult {
    pub fn from_config(shell: Option<String>) -> Self {
        Self {
            shell,
            auto_detected_and_persisted: false,
        }
    }
}

/// 探测并可能持久化 Windows bash 的入口。
///
/// 仅在 `cfg!(windows)` 且 `shell` 为空时执行探测。探测成功后，将找到的 bash
/// 绝对路径写入 config.toml；写入后 `auto_detected_and_persisted = true`。
/// 探测使用 `spawn_blocking` 以避免阻塞 tokio 运行时，对 `bash --version`
/// 的调用已有 1 秒超时，且仅对存在的文件路径调用。
pub async fn resolve_windows_shell(
    ody_home: &Path,
    shell: Option<String>,
) -> ShellConfigResult {
    if cfg!(not(windows)) || shell.is_some() {
        // 用户已配置 shell：无论是否有效，都不探测、不修改配置。
        return ShellConfigResult::from_config(shell);
    }

    let detection = tokio::task::spawn_blocking({
        move || {
            let fs = shell_command::shell_detect::RealFsChecker;
            shell_command::shell_detect::detect_windows_bash(&fs)
        }
    })
    .await
    .ok()
    .flatten();

    match detection {
        Some(detection) => {
            let path = detection.shell.shell_path.to_string_lossy().to_string();
            if let Err(err) = persist_shell(ody_home, &path).await {
                tracing::warn!(%err, "auto-detected bash but failed to persist to config.toml");
                return ShellConfigResult {
                    shell: Some(path),
                    auto_detected_and_persisted: false,
                };
            }
            ShellConfigResult {
                shell: Some(path),
                auto_detected_and_persisted: true,
            }
        }
        None => ShellConfigResult::from_config(None),
    }
}

async fn persist_shell(ody_home: &Path, shell_path: &str) -> std::io::Result<()> {
    tokio::task::spawn_blocking({
        let ody_home = ody_home.to_path_buf();
        let shell_path = shell_path.to_string();
        move || persist_shell_blocking(&ody_home, &shell_path)
    })
    .await
    .map_err(|err| std::io::Error::other(format!("persist shell task panicked: {err}")))?
}

fn persist_shell_blocking(ody_home: &Path, shell_path: &str) -> std::io::Result<()> {
    use ody_utils_path::write_atomically;
    use toml_edit::DocumentMut;

    let config_path = ody_home.join("config.toml");
    let mut doc = match std::fs::read_to_string(&config_path) {
        Ok(raw) => raw
            .parse::<DocumentMut>()
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => DocumentMut::new(),
        Err(err) => return Err(err),
    };

    let root = doc.as_table_mut();
    root.insert("shell", toml_edit::value(shell_path));

    write_atomically(&config_path, &doc.to_string())
}
```

## 集成到 ConfigLayerStack

### 推荐位置

在 `load_config_layers_state` 返回 `ConfigLayerStack` 之后，或在该函数末尾、即将返回前，调用 `resolve_windows_shell`。

由于 `ConfigLayerStack` 本身不持有 `ody_home` 路径（由调用方传入），最自然的做法是在加载完成后的某个包装函数中完成：

```rust
pub async fn load_config_and_resolve_shell(
    fs: &dyn ExecutorFileSystem,
    ody_home: &Path,
    cwd: Option<AbsolutePathBuf>,
    cli_overrides: &[(String, TomlValue)],
    options: impl Into<ConfigLoadOptions>,
    thread_config_loader: &dyn ThreadConfigLoader,
) -> io::Result<ConfigLayerStack> {
    let mut stack = load_config_layers_state(
        fs,
        ody_home,
        cwd,
        cli_overrides,
        options,
        thread_config_loader,
    )
    .await?;

    let shell_result = shell_auto_detect::resolve_windows_shell(
        ody_home,
        stack.toml.shell.clone(),
    )
    .await;

    stack.shell_config_result = Some(shell_result);
    Ok(stack)
}
```

### 在 ConfigLayerStack 上新增字段

```rust
pub struct ConfigLayerStack {
    // ... existing fields ...
    pub toml: ConfigToml,
    /// shell 配置结果与自动探测元数据。
    #[serde(skip)]
    pub shell_config_result: Option<ShellConfigResult>,
}
```

或者，如果 `ShellConfigResult` 需要直接替换 `toml.shell`（以便 core 读取 `config.shell` 时就是最终结果），则在探测后修改 `stack.toml.shell` 并额外设置标记：

```rust
stack.toml.shell = shell_result.shell.clone();
stack.shell_config_result = Some(shell_result);
```

**推荐** [C:INFERRED]：保持 `ConfigToml` 的 `shell` 字段为最终值（即探测后的值），同时通过 `shell_config_result` 暴露元数据。这样 core 层无需修改对 `config.shell` 的读取方式，只需额外读取 `shell_config_result` 来生成通知。

## 公开 API 给 core

在 `config/src/lib.rs` 中导出：

```rust
pub mod shell_auto_detect;
pub use shell_auto_detect::{resolve_windows_shell, ShellConfigResult};
```

在 `ConfigLayerStack` 上提供便捷方法：

```rust
impl ConfigLayerStack {
    /// 返回 shell 最终值与是否在本次启动中由自动探测生成。
    pub fn shell_config_result(&self) -> &ShellConfigResult {
        self.shell_config_result
            .as_ref()
            .unwrap_or(&ShellConfigResult::from_config(self.toml.shell.clone()))
    }
}
```

## 并发写入安全

`config.toml` 可能在同进程或不同进程中被并发修改。本设计在 shell 写入路径中使用进程级文件锁（`fs2` 的 `FileExt::lock_exclusive`，锁文件为 `~/.ody-code/.config.toml.lock`），保护 shell 持久化的 `read-modify-write` 过程。

注意：
- 该锁当前仅保护 shell 写入入口；`mcp_edit.rs` / `plugin_edit.rs` 尚未使用同一把锁，因此与这些入口的并发写入仍理论上存在冲突。由于 shell 写入仅在 `config.toml` 中 `shell` 为空时触发，实际冲突概率较低。
- 建议后续统一为 `ConfigEditor` 服务，使所有配置写入共享同一锁。本次设计范围暂不解决 `mcp_edit.rs` / `plugin_edit.rs` 的并发问题。
- `write_atomically` 仍保证写入文件本身的完整性。

## 错误处理与降级

| 场景 | 行为 |
|---|---|
| 非 Windows | 直接返回 `ShellConfigResult::from_config(shell)`，不探测。 |
| `shell` 已配置 | 直接返回，不探测，不修改配置。core 层负责处理无效配置时的回退。 |
| 固定路径或 PATH 探测成功 | 写入 config.toml，返回 `auto_detected_and_persisted = true`。 |
| 探测成功但写入失败 | 返回探测到的路径，但 `auto_detected_and_persisted = false`，记录 warn。 |
| 未命中 | 返回 `shell = None`，`auto_detected_and_persisted = false`，core 回退到 `cmd.exe`。 |
| `bash --version` 超时 | `detect_windows_bash` 返回 `None` 或忽略该候选，回退 `cmd.exe`。 |
| 文件锁获取失败 | 仍尝试写入，但记录 warn；若写入失败则回退。 |

## 测试断言

- 使用 mock `FsChecker` 验证固定路径命中时 `resolve_windows_shell` 返回 `auto_detected_and_persisted = true` 并写入 config.toml。
- 验证 PATH 命中时返回 `auto_detected_and_persisted = true` 且写入 config.toml。
- 验证非 Windows 或 `shell` 已配置时返回原值且不写入、不探测。
- 验证写入失败时（只读目录）返回 `auto_detected_and_persisted = false`，不 panic。
- 验证 WSL 路径被排除（`is_wsl_bash_path` 返回 true 的候选不被使用）。
- 验证文件锁能阻止并发 shell 写入导致的数据丢失（通过两个并发任务测试）。

## 依赖变更

```toml
# 根 Cargo.toml [workspace.dependencies]
fs2 = "0.4"

# config/Cargo.toml
[dependencies]
ody-shell-command = { workspace = true }
fs2 = { workspace = true }
```

如果采用方案 B（迁移到 config），则不需要 `ody-shell-command` 依赖，但仍需要 `fs2` 用于文件锁。需要在 `shell-command` 中移除 `detect_windows_bash` 并更新其测试。

## 依赖上游部分

- 本部分输出 `ShellConfigResult` 给 `core.md` 消费。
- `protocol.md` 将消费 `auto_detected_and_persisted` 来生成 `SessionConfiguredEvent.user_notification`。
