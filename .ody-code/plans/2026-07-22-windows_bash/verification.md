# Part 4: 验证与全工作区类型检查

**Scope:** 在 Part 1–3 的全部代码与测试落地后，按依赖顺序运行各 crate 的测试，并执行 `cargo check --workspace --all-targets` 确保全工作区类型一致。

## 当前状态（来源确认）

- `config/src/shell_auto_detect.rs`（Part 1 Task 2）提供 `ShellConfigResult` / `resolve_windows_shell` / `persist_shell_blocking`，并自带测试。
- `core/src/config/mod.rs`（Part 2 Task 5-6）已修改 `ConfigBuilder::build` 与 `load_config_with_layer_stack` 签名，`core/src/session/session.rs`（Part 2 Task 7）根据 `shell_config_result` 填充 `SessionConfiguredEvent.user_notification`。
- `protocol/src/protocol.rs`（Part 3 Task 9）定义 `UserNotification` / `NotificationLevel`，`app-server-protocol/src/protocol/v2/notification.rs` 与 `common.rs`（Part 3 Task 10）定义 `InfoMessageNotification` / `ServerNotification::InfoMessage`。
- `app-server/src/request_processors/thread_processor.rs`（Part 3 Task 11）在 `thread/start` 成功后转发 info 通知，`tui/src/chatwidget/protocol.rs`（Part 3 Task 12）处理 `InfoMessage`。
- 各 part 已分别包含单元/集成测试；本 part 负责统一验证，不做代码修改。

### Task 14: 运行相关 crate 测试

**Depends on:** config.md: Tasks 1-4, core.md: Tasks 5-8, protocol.md: Tasks 9-13

**Files:** 无修改（仅验证）

**Implementation:**

- [ ] 运行 `ody-config` 测试，验证 `shell_auto_detect.rs` 的探测、文件锁与持久化逻辑：
  ```bash
  cargo nextest run -p ody-config
  ```
  预期输出：所有测试通过，末尾显示 `PASS` 计数，无失败。
- [ ] 运行 `ody-core` 相关测试，验证 `ConfigBuilder` 的 shell 覆盖、`Config` 的 `shell_config_result` 字段，以及 session 通知生成：
  ```bash
  cargo nextest run -p ody-core shell_auto_detect
  cargo nextest run -p ody-core shell_notification
  cargo nextest run -p ody-core --test windows_shell_config
  ```
  预期输出：
  - `shell_auto_detect` 测试命中探测与持久化路径。
  - `shell_notification` 的 3 个测试（`auto_detected_shell_produces_info_notification`、`non_auto_detected_shell_produces_no_notification`、`user_shell_override_suppresses_notification`）全部通过。
  - `windows_shell_config` 的 `config_builder_non_windows_does_not_auto_detect` 在非 Windows 平台通过；Windows 平台无断言失败。
- [ ] 运行 `ody-protocol` 测试，验证 `SessionConfiguredEvent` 新增 `user_notification` 字段及旧 rollout 反序列化兼容：
  ```bash
  cargo nextest run -p ody-protocol session_configured_event
  ```
  预期输出：序列化/反序列化测试通过，`user_notification` 在旧数据上反序列化为 `None`。
- [ ] 运行 `ody-app-server-protocol` 测试，验证 `ServerNotification::InfoMessage` 序列化格式：
  ```bash
  cargo nextest run -p ody-app-server-protocol info_message_notification
  ```
  预期输出：序列化后的 `method` 为 `"info"`，`params.message` 为预期内容。
- [ ] 运行 `ody-app-server` 测试，验证 `InfoMessage` 通知可从 `UserNotification` 构造：
  ```bash
  cargo nextest run -p ody-app-server info_message
  ```
  预期输出：集成测试 `info_message_notification_can_be_constructed_from_user_notification` 通过。
- [ ] 运行 `ody-tui` 测试，验证 `handle_server_notification` 对 `InfoMessage` 调用 `add_info_message`：
  ```bash
  cargo nextest run -p ody-tui handle_server_notification_info_message
  ```
  预期输出：TUI 测试通过，history 中最后一条消息包含 info 文本。
- [ ] 若任一命令失败，先回退到对应 part 修复实现或测试，再重新运行本任务；不得通过 `--ignored` 或注释测试绕过失败。
- [ ] 提交（仅在所有测试通过后）：
  ```bash
  git commit --allow-empty -m "chore(verification): run targeted tests for windows bash auto-detect"
  ```

### Task 15: 全工作区类型检查

**Depends on:** Task 14（所有相关测试通过），config.md: Task 6, core.md: Task 6（共享签名已更新所有调用者）

**Files:** 无修改（仅验证）

**Implementation:**

- [ ] 执行全工作区类型检查，确认 Part 2 的 `load_config_with_layer_stack` 签名变更没有遗漏调用者：
  ```bash
  cargo check --workspace --all-targets
  ```
  预期输出：无编译错误，无 `unresolved import` / `mismatched types` / `wrong number of arguments` 等错误；末尾显示 `Finished dev [unoptimized + debuginfo] target(s)` 或 `Finished check [unoptimized + debuginfo] target(s)`。
- [ ] 若类型检查失败，回到对应任务修复所有调用者（含测试文件），然后重新运行本命令；不得跳过 `--all-targets`。
- [ ] 提交（仅在类型检查通过后）：
  ```bash
  git commit --allow-empty -m "chore(verification): workspace-wide typecheck passes"
  ```

## Self-review (Part 4)

- [ ] 1. Spec-coverage: 本 part 不对新需求做实现，仅验证索引中所有需求已在 Part 1-3 覆盖；无新增 GAP。
- [ ] 2. Placeholder scan: 无 TODO/TBD/deferred placeholders；验证步骤均给出具体命令和预期输出。
- [ ] 3. No phantom tasks: Task 14 产生测试运行结果，Task 15 产生全工作区类型检查结果；每个任务都有可验证产出。
- [ ] 4. Dependency soundness: Task 14 依赖 config.md/core.md/protocol.md 全部任务；Task 15 依赖 Task 14 以及共享签名变更任务（config.md: Task 6 / core.md: Task 6）。
- [ ] 5. Caller & build soundness: Task 15 以 `cargo check --workspace --all-targets` 作为最终全工作区类型检查，确保 Part 2 的共享签名变更没有遗漏调用者。
- [ ] 6. Test-the-risk: 本 part 为验证阶段，通过运行 Part 1-3 的行为测试确认状态变更（持久化、通知生成）已被断言；未新增状态变更，因此无需新增测试。
- [ ] 7. Type consistency: 全工作区类型检查确保 Part 1-3 定义的 `ShellConfigResult`、`UserNotification`、`NotificationLevel`、`InfoMessageNotification`、`ServerNotification::InfoMessage` 在所有 crate 中类型一致。
