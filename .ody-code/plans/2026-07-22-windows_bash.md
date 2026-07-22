# Windows 平台 bash 自动探测与持久化 — 实施计划

**目标：** 在 Windows 平台且 `~/.ody-code/config.toml` 的 `shell` 为空时，自动探测原生 bash（Git Bash/MSYS2/Cygwin/PATH），将绝对路径持久化到 `config.toml`，并通过 `SessionConfiguredEvent.user_notification` + `ServerNotification::InfoMessage` 向 TUI 发送一次非阻塞提示。

**架构：** 探测与持久化下沉到 `ody-config` crate（新增 `shell_auto_detect.rs`），使用 `spawn_blocking` 和 `fs2` 文件锁；`ody-core` 在 `ConfigBuilder::build` 中调用该探测、覆盖最终 `shell` 值，并把 `ShellConfigResult` 存到 `Config`；`ody-core` 的 session 启动路径根据 `shell_config_result.auto_detected_and_persisted` 填充 `SessionConfiguredEvent.user_notification`；`ody-app-server` 在 `thread/start` 成功后转发为 `ServerNotification::InfoMessage`；TUI 的 `chatwidget/protocol.rs` 处理该变体并调用 `add_info_message`。

**技术栈：** Rust（cargo, tokio, fs2, toml_edit, ts-rs, schemars, serde），涉及 crates：`config`、`shell-command`、`core`、`protocol`、`app-server-protocol`、`app-server`、`tui`。

> 对于执行工作者：请按任务逐条实现（建议每个任务使用新的子代理/Task，避免单会话上下文退化）。步骤使用 - [ ] 复选框跟踪。

## 文件结构

| 职责 | 文件 |
|---|---|
| 探测结果类型、文件锁、持久化入口 | `config/src/shell_auto_detect.rs` |
| config crate 模块导出 | `config/src/lib.rs` |
| 配置依赖声明 | `Cargo.toml`、`config/Cargo.toml` |
| 触发探测并保存结果到 `Config` | `core/src/config/mod.rs` |
| `Config` 的 shell 探测元数据字段 | `core/src/config/mod.rs` |
| session 启动路径生成通知 | `core/src/session/session.rs` |
| 通用用户通知类型、扩展 `SessionConfiguredEvent` | `protocol/src/protocol.rs` |
| TUI 可消费的 info 通知类型 | `app-server-protocol/src/protocol/v2/notification.rs` |
| 服务器通知枚举扩展 | `app-server-protocol/src/protocol/common.rs` |
| 启动线程后转发 info 通知 | `app-server/src/request_processors/thread_processor.rs` |
| TUI 接收并展示 info 通知 | `tui/src/chatwidget/protocol.rs` |
| 各 crate 测试 | `config/src/shell_auto_detect.rs`、`core/tests/...`、`protocol/src/...`、`app-server-protocol/src/...`、`app-server/tests/...`、`tui/src/...` |

## 依赖概述

```
Part 1: Config crate 探测、持久化与写入
  ├─ Task 1: 依赖变更（fs2 workspace + config 的 ody-shell-command/fs2）
  ├─ Task 2: 实现 `config/src/shell_auto_detect.rs`（ShellConfigResult、ConfigFileLock、resolve_windows_shell）
  ├─ Task 3: `config/src/lib.rs` 导出 `shell_auto_detect`
  └─ Task 4: config crate 测试

Part 2: Core session 启动路径中的 shell 选择流程变更
  ├─ depends on Part 1: Task 2（使用 ShellConfigResult / resolve_windows_shell）
  ├─ Task 5: 在 `core/src/config/mod.rs` 的 `ConfigBuilder::build` 中调用 `resolve_windows_shell` 并覆盖 `config_toml.shell`（depends on config.md: Task 2）
  ├─ Task 6: 在 `Config` 结构体上新增 `shell_config_result` 字段并修改 `load_config_with_layer_stack` 签名（depends on config.md: Task 2, Task 5）
  ├─ Task 7: 在 `core/src/session/session.rs` 中读取 `config.shell_config_result` 并生成 `SessionConfiguredEvent.user_notification`（depends on Task 6, protocol.md: Task 9）
  └─ Task 8: core 测试（depends on Task 6, Task 7）

Part 3: `SessionConfiguredEvent.user_notification` 与 TUI 提示集成
  ├─ Task 9: 在 `protocol` crate 中新增 `UserNotification` / `NotificationLevel`，扩展 `SessionConfiguredEvent`（depends on nothing）
  ├─ Task 10: 在 `app-server-protocol` 中新增 `InfoMessageNotification` 和 `ServerNotification::InfoMessage`（depends on Task 9）
  ├─ Task 11: 在 `app-server` 的 `thread_processor.rs` 中启动线程后发送 `InfoMessage`（depends on Task 9, Task 10, core.md: Task 7）
  ├─ Task 12: 在 `tui/src/chatwidget/protocol.rs` 中处理 `ServerNotification::InfoMessage`（depends on Task 10）
  └─ Task 13: protocol / app-server-protocol / app-server / tui 测试（depends on Tasks 9-12）

Part 4: 验证
  ├─ depends on Part 1–3
  ├─ Task 14: 运行相关 crate 测试
  └─ Task 15: 全工作区类型检查
```

## 设计说明与实现调整

原设计文件假设 `ConfigLayerStack` 持有 `toml` 字段并可在 `config` crate 中直接完成 `shell` 覆盖。经代码核对，`ConfigLayerStack` 实际仅持有 `layers`（`Vec<ConfigLayerEntry>`），合并后的 `ConfigToml` 在 `core/src/config/mod.rs` 中反序列化。因此本计划将 `shell` 覆盖和 `ShellConfigResult` 保存放在 `core/src/config/mod.rs` 的 `ConfigBuilder::build` 中完成，并通过 `Config` 的新字段 `shell_config_result` 暴露给 `core/src/session/session.rs`。这与原设计的数据流和接口语义一致，仅调整字段宿主位置。

## 风险与开放问题

| # | 风险 | 缓解 |
|---|---|---|
| R1 | `config` crate 新增 `ody-shell-command` 依赖后，未来若 `shell-command` 反向依赖 `config` 会产生循环依赖。 | 实施前先运行 `cargo tree -p ody-shell-command --invert -e normal` 确认无反向依赖；Part 1 完成后再次运行 `cargo check -p ody-config` 验证。 |
| R2 | `resolve_windows_shell` 使用 `tokio::task::spawn_blocking`，但 `config` crate 的 `tokio` 仅启用 `fs` 特性。 | Task 1 在 `config/Cargo.toml` 中将 `tokio` 特性改为 `features = ["fs", "rt"]`。 |
| R3 | 现有 `detect_windows_bash` 对 PATH 命中设置 `is_fixed_path = false` 且原注释表示 PATH 命中不持久化，但本设计 require 持久化任意 bash。 | Task 2 的 `resolve_windows_shell` 不检查 `is_fixed_path`，只要 `detect_windows_bash` 返回候选即写入其绝对路径。 |
| R4 | `load_config_with_layer_stack` 签名变更会波及 `core/src/config/mod.rs` 内部调用点以及测试文件（约 15 处），漏改会导致编译失败。 | Task 6 中一次性更新所有调用点（含 `core/src/agent/role.rs`、`core/src/config/config_tests.rs`、`core/src/guardian/tests.rs`），并以 `cargo check --workspace --all-targets` 作为该任务结束条件。 |
| R5 | 旧 rollout 反序列化 `SessionConfiguredEvent` 时缺少 `user_notification` 字段。 | Task 9 在手动 `Deserialize` 的 `Wire` 结构体中为 `user_notification` 提供 `#[serde(default)]`。 |
| R6 | `mcp_edit.rs` / `plugin_edit.rs` 与 shell 写入使用不同锁，理论上仍可能并发修改 `config.toml`。 | 本计划按设计文件范围仅对 shell 写入加锁；在 `config/src/shell_auto_detect.rs` 中新增 `ConfigFileLock`，并在 Task 2 测试中验证锁能阻止并发 shell 写入。 |
| R7 | 旧 TUI 客户端不认识 `ServerNotification::InfoMessage`，可能 panic 或忽略。 | `ServerNotification` 是枚举，`InfoMessage` 作为新增变体；旧客户端若未升级绑定会忽略未知 method，不会 panic。 |
| R8 | Part 2 Task 7 使用 `UserNotification` / `NotificationLevel`，而它们由 Part 3 Task 9 定义；若顺序错误会导致编译失败。 | 实施时必须先完成 protocol.md: Task 9，再执行 core.md: Task 7；本计划依赖图已明确标注该交叉依赖。 |

## Spec coverage

| 需求 | 任务 | 状态 |
|---|---|---|
| Windows 且 `shell` 为空时触发 bash 探测 | Task 2, Task 5 | covered |
| 探测候选：固定安装目录 + PATH 中的 `bash.exe` | Task 2（复用 `detect_windows_bash`） | covered |
| 探测成功后写入 `~/.ody-code/config.toml` 的绝对路径 | Task 2（`persist_shell_blocking`） | covered |
| 通过 `SessionConfiguredEvent` 向客户端发送一次用户通知 | Task 7, Task 9 | covered |
| 使用 `spawn_blocking` 避免探测阻塞 tokio | Task 2, Task 5 | covered |
| shell 写入路径使用进程级文件锁 | Task 2（`ConfigFileLock`） | covered |
| `shell` 已配置时不探测、不修改 | Task 2（`resolve_windows_shell` 开头判断） | covered |
| 非 Windows 平台行为不变 | Task 2（`cfg!(not(windows))` 直接返回） | covered |
| 不探测 zsh、PowerShell、WSL | Task 2（复用 `detect_windows_bash`，其已排除 WSL） | covered |
| 不阻塞会话启动、不弹窗确认 | Task 2（spawn_blocking 异步）, Task 11（异步发送通知） | covered |
| 恢复线程时不重复发送通知 | Task 11（仅在 `thread/start` 成功后发送） | covered |
| 旧 rollout 反序列化兼容 | Task 9（`#[serde(default)]`） | covered |

## Out-of-scope

| 符号/路径 | 原因 | 动作 |
|---|---|---|
| `mcp_edit.rs` / `plugin_edit.rs` 的并发写入 | 设计文件明确将其列为 out-of-scope，建议后续统一 ConfigEditor | 不修改，仅新增 `ConfigFileLock` 供 shell 写入使用 |
| `detect_windows_bash` 本身 | 复用已有逻辑，不修改其算法或固定路径列表 | 不修改 |
| zsh / PowerShell / WSL 探测 | 设计文件 scope out | 不新增 |
| 国际化消息 | 设计文件 `[C:DEFERRED]` | 使用硬编码中文提示，不新增 i18n 机制 |
| `ThreadResumeResponse` 新增通知字段 | 设计文件选择方案 A，恢复时不发送 | 不修改 `ThreadResumeResponse` |

## Parts

| # | File | Scope | Status |
|---|---|---|---|
| 1 | `2026-07-22-windows_bash/config.md` | Config crate 探测、持久化与写入 | done |
| 2 | `2026-07-22-windows_bash/core.md` | Core session 启动路径中的 shell 选择流程变更 | done |
| 3 | `2026-07-22-windows_bash/protocol.md` | `SessionConfiguredEvent.user_notification` 与 TUI 提示集成 | done |
| 4 | `2026-07-22-windows_bash/verification.md` | 验证与全工作区类型检查 | done |

## 跨文件一致性审查

- 交叉依赖确认：`core.md: Task 7` 消费的 `UserNotification` / `NotificationLevel` 由 `protocol.md: Task 9` 定义；`protocol.md: Task 11` 消费的 `SessionConfiguredEvent.user_notification` 由 `core.md: Task 7` 填充。依赖图中两条交叉边均已列出，实施顺序满足 DAG。
- 共享签名确认：`core.md: Task 6` 修改的 `load_config_with_layer_stack` 签名在 Task 6 内一次性更新所有调用者（含测试文件），并在 Task 15 以 `cargo check --workspace --all-targets` 最终验证。
- 类型一致性确认：`ShellConfigResult`（config.md: Task 2）、`UserNotification` / `NotificationLevel`（protocol.md: Task 9）、`InfoMessageNotification` / `ServerNotification::InfoMessage`（protocol.md: Task 10）的字段名、枚举变体名、方法名在所有 part 中保持一致。
- 文件冲突检查：没有两个 part 任务同时修改同一文件的不兼容区域；`core/src/config/mod.rs` 的修改集中在 Part 2，`core/src/session/session.rs` 的修改集中在 Part 2，`app-server/src/request_processors/thread_processor.rs` 与 `tui/src/chatwidget/protocol.rs` 的修改集中在 Part 3。

## Self-review

- [x] 1. Spec-coverage table: 已映射所有 12 条需求到具体 Task（`## Spec coverage` 表），无 GAP 或 no-op。
- [x] 2. Placeholder scan: 索引与所有 part 文件均无 TODO/TBD/deferred placeholders；每个任务均给出具体代码或具体验证命令。
- [x] 3. No phantom tasks: 15 个 Task 均产生代码、测试或验证产出；无 `--allow-empty` 或 "already done in Task N" 情况。
- [x] 4. Dependency soundness: 依赖图从 Earlier 指向 Later；`core.md: Task 7` 引用 `protocol.md: Task 9` 定义的符号，`protocol.md: Task 11` 引用 `core.md: Task 7` 产生的字段，两条交叉边均合法。
- [x] 5. Caller & build soundness: `core.md: Task 6` 一次性更新 `load_config_with_layer_stack` 的所有调用者（含测试文件），并以 `cargo check --workspace --all-targets` 结束；`verification.md: Task 15` 再次执行全工作区类型检查。
- [x] 6. Test-the-risk: 状态变更任务（`config.md: Task 2` 持久化、`core.md: Task 7` 通知生成）均有行为测试断言；验证阶段 Task 14 重新运行这些测试。
- [x] 7. Type consistency: 跨 part 引用的类型名称与字段（`ShellConfigResult.shell`、`UserNotification.level` / `message`、`NotificationLevel::Info`、`InfoMessageNotification.message`、`ServerNotification::InfoMessage`）在所有 part 中保持一致。
