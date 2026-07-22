# Windows 平台 bash 自动探测与持久化设计

## 背景与目标

Windows 平台下，当 `~/.ody-code/config.toml` 中的 `shell` 未配置或为空时，Ody 当前直接回退到 `cmd.exe`。本设计引入自动探测可用的原生 Windows `bash.exe`（Git Bash、MSYS2、Cygwin 等），在探测成功时将其绝对路径持久化到 `config.toml`，并给 TUI 一次非阻塞提示。非 Windows 平台行为不变；已有 `shell` 配置的用户不受影响。

## Scope In

- Windows 平台下，且 `config.toml` 中 `shell` 为空时触发 bash 探测。
- 探测候选路径：固定安装目录（Git Bash、MSYS2、Cygwin） + `PATH` 中的 `bash.exe`。
- 探测到任意可用 bash 后，将其绝对路径写入 `~/.ody-code/config.toml`。
- 通过 `SessionConfiguredEvent` 向客户端发送一次用户通知，告知自动切换到了 bash。
- 对 `detect_windows_bash` 的调用方式与生产集成做最小调整。
- 使用 `spawn_blocking` 避免探测阻塞 tokio 运行时。
- 在 shell 写入路径使用进程级文件锁保护 `read-modify-write`。

## Scope Out

- 不自动探测 zsh、PowerShell 或其他 shell。
- 不支持 WSL bash（`WindowsApps`、`wsl`、`bash.exe` 在 System32 等路径被显式排除）。
- 不修改非 Windows 平台的 shell 选择逻辑。
- 当 `config.toml` 中 `shell` 已配置时，无论是否有效，都不探测、不修改配置。
- 不阻塞会话启动，不弹窗要求用户确认。
- 不解决 `mcp_edit.rs` / `plugin_edit.rs` 与 shell 写入之外的并发冲突（建议后续统一 ConfigEditor）。

## Parts

| # | File | Scope | Status |
|---|---|---|---|
| 1 | `2026-07-22-windows_bash/core.md` | Session 启动路径中的 shell 选择流程变更 | done |
| 2 | `2026-07-22-windows_bash/config.md` | Config crate 的探测、持久化与写入 | done |
| 3 | `2026-07-22-windows_bash/protocol.md` | `SessionConfiguredEvent.user_notification` 与 TUI 提示集成 | done |

## Architecture / Design

高层数据流：

```
Session 启动
    │
    ▼
ConfigLayerStack 加载 (config crate)
    │
    ├─ 若 cfg!(windows) 且 shell 为空 ──► spawn_blocking(detect_windows_bash)
    │                                    │
    │   命中（固定路径或 PATH）──► 加锁写入 config.toml ──┤
    │   未命中                 ──► 回退 cmd.exe           │
    │
    ▼
SessionConfiguredEvent 生成 (protocol/app-server)
    │
    ├─ 若 config 层触发持久化 ──► user_notification = Some(...)
    │
    ▼
TUI 接收 ServerNotification::InfoMessage，展示一次性通知
```

### 关键决策

- **只要 config.toml 中 `shell` 不为空就不探测** [C:USER]：无论配置是否有效，config 层都不探测、不修改；由 core 层处理无效配置时的回退。
- **探测到任意 bash 都持久化** [C:USER]：不仅固定路径，PATH 中命中的 bash 也找到其绝对路径并写入 `config.toml`，避免每次启动重复探测。
- **探测下沉到 config crate** [C:USER]：由 `config` 层在加载配置时完成探测和持久化，core session 只读取最终结果和通知标记。
- **成功才提示** [C:USER]：探测失败或回退时静默，不提示用户。
- **非阻塞探测** [C:INFERRED]：`detect_windows_bash` 在 `spawn_blocking` 中执行，避免阻塞 tokio 运行时；`bash --version` 仅对已存在文件路径调用。
- **进程级文件锁** [C:INFERRED]：在 shell 写入路径使用 `fs2` 独占锁，保护 shell 持久化。`mcp_edit.rs` / `plugin_edit.rs` 尚未共享该锁，建议后续统一 ConfigEditor。

## Data Models

新增数据类型：

- `ShellConfigResult`（`config` crate）：包含 `shell: Option<String>` 和 `auto_detected_and_persisted: bool`。
- `UserNotification` / `NotificationLevel`（`protocol` crate）：通用的一次性用户通知结构。
- `InfoMessageNotification`（`app-server-protocol` crate）：TUI 可消费的 info 通知。
- `ConfigFileLock`（`config` crate）：基于 `fs2` 的进程级文件锁封装。

扩展的数据类型：

- `ConfigLayerStack`：可选字段 `shell_config_result: Option<ShellConfigResult>`，提供 `shell_config_result()` 方法。
- `SessionConfiguredEvent`：新增 `user_notification: Option<UserNotification>`，兼容旧 rollout。
- `ServerNotification`：新增 `InfoMessage` 变体，用于向 TUI 转发 info 提示。

详见各 part 文件。

## Algorithms / Implementation Notes

核心算法分布在三个 part 文件中：

1. **config 层探测与持久化**（`config.md`）：
   - `resolve_windows_shell(ody_home, shell)`：
     - 非 Windows 直接返回。
     - `shell` 已配置直接返回，不探测、不修改配置。
     - 在 `spawn_blocking` 中调用 `detect_windows_bash`，避免阻塞 tokio。
   - 命中（固定路径或 PATH）：加锁写入 `config.toml`，返回 `auto_detected_and_persisted = true`。
   - 未命中：返回 `shell = None`。
   - `persist_shell_blocking`：获取 `fs2` 文件锁 → 读取 `DocumentMut` → 插入 `shell` 项 → `write_atomically`。

2. **core 层 shell 选择**（`core.md`）：
   - 从 `config.shell_config_result()` 读取最终 `shell` 值。
   - 保持 `user_shell_override` → `config.shell` → `zsh fork` → `default_user_shell()` 顺序。
   - 若 `auto_detected_and_persisted`，构造 `UserNotification` 并填入 `SessionConfiguredEvent`。
   - 若 `config.shell` 已配置但无效，回退到 `default_user_shell()` 并记录 warn，不修改配置。

3. **protocol 与 TUI 转发**（`protocol.md`）：
   - core 构造 `SessionConfiguredEvent` 时填充 `user_notification`。
   - app-server 在 `thread/start` 响应后，读取 `session_configured.user_notification`，发送 `ServerNotification::InfoMessage`。
   - TUI 的 `handle_server_notification_event` 匹配 `InfoMessage` 并调用 `chat_widget.add_info_message`。

## Error Handling / Degradation

| 场景 | 行为 |
|---|---|
| 非 Windows 平台 | 不探测，直接返回原 `shell` 值。 |
| `shell` 已配置 | 不探测，不修改配置。core 层负责处理无效配置时的回退。 |
| 固定路径或 PATH 探测成功 | 加锁写入 config.toml，使用 bash，发送提示。 |
| 探测成功但写入失败 | 使用 bash，不发送提示，记录 warn。 |
| 未命中任何 bash | 回退 `cmd.exe`（Windows 默认），不提示。 |
| `bash --version` 超时 | 忽略该候选，继续探测或回退。 |
| 探测到的 shell 在 core 中无法识别 | 回退 `default_user_shell()` 并记录。 |
| 用户配置的 shell 无效 | core 回退到 `default_user_shell()`，记录 warn。config 层不验证、不修改。 |
| `user_shell_override` 存在 | 完全跳过探测和提示。 |
| 恢复线程 | 不重新发送 `InfoMessage`，避免重复提示。 |
| 旧 rollout 反序列化 | `user_notification` 缺失时默认 `None`，不影响回放。 |
| 旧客户端接收 `InfoMessage` | 忽略未知 method，无影响。 |
| 文件锁获取失败 | 仍尝试写入，但记录 warn；若写入失败则回退。 |

## Assumptions & Unverified Items

| # | Assumption | Confidence | Impact if wrong | How to verify |
|---|---|---|---|---|
| 1 | `SessionConfiguredEvent` 添加 `user_notification` 字段不会破坏旧 rollout 反序列化 | medium | 历史会话回放失败 | 使用 `#[serde(default, skip_serializing_if = "Option::is_none")]` 并在测试中覆盖 |
| 2 | TUI 已存在一次性通知的展示机制，或可以轻松接入 `SessionConfiguredEvent` 中的新字段 | low | 需要额外 UI 工作量 | 检查 app-server / TUI 代码中的通知渲染路径 |
| 3 | `detect_windows_bash` 的 `&dyn FsChecker` 签名可以在 config crate 中直接使用，无需改为无参数 | medium | 需要修改函数签名或包装 | 确认 `shell-command` 对 `config` 的依赖关系与 `FsChecker` trait 可见性 |
| 4 | `config` crate 新增 `ody-shell-command` 依赖不会引入循环依赖 | high | 编译失败，需改方案 B | 已通过 `cargo tree -p ody-shell-command --invert -e normal` 验证无反向依赖 |
| 5 | 恢复线程时不应重复发送 `InfoMessage` 通知 | medium | 用户每次重连都看到重复提示 | 在 app-server 层设计为仅在 `thread/start` 成功后发送 |
| 6 | `fs2` crate 的跨平台文件锁在 Windows 上可用 | high | 文件锁失效，并发写入冲突 | `fs2` 明确支持 Windows；实现后加并发测试 |
| 7 | 只要 `shell` 已配置就不探测，不验证，不修改 | high | 违反用户原则 | 已在 `config.md` 的 `resolve_windows_shell` 中明确实现 |
| 8 | PATH 命中的 bash 路径写入 config.toml 不会导致用户困惑 | medium | 用户看到 config.toml 中 shell 来自 PATH | 提示中说明路径来源；持久化后用户可手动修改 |

## Risk Register

| # | Risk | 可能性 | 影响 | 缓解措施 |
|---|---|---|---|---|
| 1 | 探测 `bash.exe` 时 `bash --version` 1 秒超时阻塞 tokio 运行时 | 低 | 启动延迟/卡顿 | 使用 `spawn_blocking` 执行探测；`detect_windows_bash` 会先检查文件存在再调用 `bash --version` |
| 2 | 写入 config.toml 失败（权限、磁盘）导致每次启动都重复探测 | 低 | 性能与噪音 | 写失败时记录 warn，不再提示；后续启动会再次探测 |
| 3 | 持久化后 bash 被卸载导致 core 回退 | 中 | 用户不知情地回退到 cmd | 每次启动时 core 会验证并记录 warn；config 层不修改用户配置 |
| 4 | 新增 `ServerNotification::InfoMessage` 破坏旧客户端或生成 schema 失败 | 低 | 兼容性/CI 失败 | 使用标准枚举扩展方式，更新 schema 生成，测试反序列化兼容性 |
| 5 | `fs2` 文件锁保护范围不完整，与 mcp_edit/plugin_edit 并发冲突 | 中 | 配置丢失 | 文件锁仅保护 shell 写入；建议后续统一 ConfigEditor；实现并发测试 |
| 6 | 固定路径列表硬编码，无法覆盖非标准安装位置 | 中 | 非标准安装无法持久化 | PATH 探测会覆盖这些场景；探测成功后写入绝对路径 |

## Reuse Analysis

| 组件 | 可复用候选 | 说明 |
|---|---|---|
| Bash 探测 | `shell-command/src/shell_detect.rs:407` 的 `detect_windows_bash` 和 `FsChecker` / `RealFsChecker` | 生产逻辑与测试抽象已存在，仅需在 config crate 中接入。 |
| Config 原子写 | `config/src/plugin_edit.rs:74` 的 `write_atomically` 和 `read_or_create_document` | 可直接复用或包装为 `persist_shell_blocking`。 |
| 配置编辑模式 | `config/src/mcp_edit.rs` 的 `ConfigEditsBuilder` 和 `plugin_edit.rs` 的 `apply_user_plugin_config_edits` | 可作为新建 `shell_auto_detect.rs` 的参考。 |
| TUI 信息提示 | `tui/src/chatwidget.rs` 的 `add_info_message` 和 `tui/src/app/app_server_events.rs` 的事件处理 | 直接复用，无需新增 UI 组件。 |
| ServerNotification 扩展 | `app-server-protocol/src/protocol/common.rs:1454` 的宏定义模式 | 新增 `InfoMessage` 变体遵循现有模式。 |
| 通用 info 通知 | 无现成通用 info ServerNotification，最接近的是 `Warning`、`ConfigWarning`、`DeprecationNotice` | 需要新增 `InfoMessageNotification` 和对应变体。 |
| 文件锁 | 无现成文件锁封装 | 新增 `fs2` workspace 依赖并包装 `ConfigFileLock`。 |

## Self-Review

### 最昂贵的三个决策

1. **探测下沉到 config crate 是否合适？**
   - 审计：已通过 `cargo tree` 验证无循环依赖。config 依赖 shell-command 改变了职责方向，但当前是最小改动。如果未来需要更干净的架构，可迁移探测逻辑到 config 或新 crate。
   - 缓解：设计保留了方案 B 作为备选。

2. **是否应该在恢复线程时重复发送通知？**
   - 审计：选择恢复时不发送，避免重复打扰。用户可通过 config.toml 查看已持久化的 shell。
   - 缓解：若反馈需要，可在后续迭代中增加 TUI 设置开关。

3. **只要 shell 已配置就不探测、不验证、不修改是否合适？**
   - 审计：这是用户明确提出的原则。优势是尊重用户显式配置；劣势是用户配置了无效路径后，每次启动都会回退并记录 warn，但 config 不会自动修复。
   - 缓解：core 层会记录清晰 warn，让用户知道配置无效；用户可手动修改或删除 config.toml 中的 `shell` 以触发重新探测。

### 四镜审查

- **Security**：探测只使用固定路径和 PATH，拒绝 WSL shim；写入前路径已由 `detect_windows_bash` 通过 `bash --version` 验证；持久化只写入用户主目录；文件锁保护 shell 写入。无新增 secrets 或远程调用。
- **Test/Verification**：每个 part 都包含测试断言；需要覆盖 Windows 和非 Windows、已配置/未配置、固定路径/PATH/未命中、写入成功/失败、旧 rollout 兼容性、文件锁并发、无效配置回退。
- **Operations**：新行为默认对 Windows 用户生效；已有配置不受影响；无迁移脚本；写失败时不影响当前会话；无效配置由 core 回退并记录。
- **Integration**：需要新增 workspace 和 config 的 `fs2` 依赖、config 对 shell-command 的依赖、扩展 protocol 和 app-server-protocol 类型、修改 app-server 和 TUI 处理。跨 crate 接口已明确。

### 重新检查 Scope

Scope 符合用户选择：Windows 下 `shell` 为空时探测 bash，命中后持久化并提示。不验证/修改已配置 shell，不扩展其他 shell 或 WSL，不修改非 Windows 行为。无遗漏。

### 跨文件一致性检查

- `core.md` 消费的 `ShellConfigResult` 与 `config.md` 定义一致，字段名 `shell` 和 `auto_detected_and_persisted` 完全匹配。
- `core.md` 填充的 `SessionConfiguredEvent.user_notification` 与 `protocol.md` 中 `UserNotification` 结构一致。
- `protocol.md` 的 `InfoMessageNotification` 通过 `ServerNotification::InfoMessage` 转发到 TUI 的 `add_info_message`，与 TUI 现有接口一致。
- `config.md` 的 `resolve_windows_shell` 明确实现：只要 `shell` 已配置就不探测、不修改；PATH 命中也持久化。
- `config.md` 的 `fs2` 文件锁和 `core.md` 的无效配置回退逻辑在索引中已反映。
- 所有 `[C:INFERRED]` 假设均已在 `## Assumptions & Unverified Items` 表中列出。

## User Approval

本设计已根据用户原则和 adversarial review 修正：
- 只要 `config.toml` 中 `shell` 已配置，就不探测、不验证、不修改。
- 探测到任意 bash（固定路径或 PATH）都写入其绝对路径到 `config.toml`。
- 使用 `spawn_blocking` 避免探测阻塞 tokio 运行时。
- 在 shell 写入路径使用 `fs2` 文件锁；`mcp_edit.rs` / `plugin_edit.rs` 的并发问题建议后续统一 ConfigEditor。
- 已验证 `ody-shell-command` 无反向依赖，config 添加依赖不会循环。

请审批：
- 是否同意在 `config` crate 中探测并持久化 Windows bash（包括 PATH 命中）？
- 是否同意新增 `SessionConfiguredEvent.user_notification` 和 `ServerNotification::InfoMessage`？
- 是否同意引入 `fs2` 文件锁保护 shell 写入路径？
- 是否同意只要 `shell` 已配置就不探测、不验证、不修改？
