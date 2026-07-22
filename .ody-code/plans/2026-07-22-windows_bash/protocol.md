# Part 3: `SessionConfiguredEvent.user_notification` 与 TUI 提示集成

**Scope:** 在 `protocol` crate 定义通用一次性用户通知类型并扩展 `SessionConfiguredEvent`；在 `app-server-protocol` 定义 TUI 可消费的 `InfoMessageNotification` 与 `ServerNotification::InfoMessage`；在 `app-server` 的 `thread/start` 成功后将 `SessionConfiguredEvent.user_notification` 转发为 `InfoMessage`；在 TUI 中展示该提示；最后补充各 crate 测试。

## 当前状态（来源确认）

- `protocol/src/protocol.rs:3689` 定义 `SessionConfiguredEvent`；`protocol/src/protocol.rs:3754` 手动实现 `Deserialize`，使用 `Wire` 结构体兼容旧 rollout。
- `app-server-protocol/src/protocol/v2/notification.rs` 已存在 `WarningNotification`、`ErrorNotification` 等通用通知结构。
- `app-server-protocol/src/protocol/common.rs:1454` 使用 `server_notification_definitions!` 宏定义 `ServerNotification` 变体。
- `app-server/src/request_processors/thread_processor.rs:1135-1279` 处理 `thread/start`：从 `NewThread` 解出 `session_configured`，构造 `ThreadStartResponse` 并发送 `ServerNotification::ThreadStarted`。
- `tui/src/chatwidget/protocol.rs:31-227` 的 `handle_server_notification` match 处理 `ServerNotification` 各变体；`ChatWidget::add_info_message` 位于 `tui/src/chatwidget.rs:1511`。

### Task 9: 在 `protocol` crate 新增 `UserNotification` / `NotificationLevel`，扩展 `SessionConfiguredEvent`

**Depends on:** none

**Files:**
- Modify: `protocol/src/protocol.rs`

**Implementation:**

- [ ] 在 `protocol/src/protocol.rs` 中，于 `SessionConfiguredEvent` 定义之前新增 `NotificationLevel` 枚举和 `UserNotification` 结构体（建议放在 `protocol/src/protocol.rs:3680-3688` 之间）：
  ```rust
  #[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, TS, PartialEq, Eq)]
  #[serde(rename_all = "camelCase")]
  #[ts(export_to = "protocol/")]
  pub enum NotificationLevel {
      Info,
  }

  #[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, TS, PartialEq, Eq)]
  #[serde(rename_all = "camelCase")]
  #[ts(export_to = "protocol/")]
  pub struct UserNotification {
      pub level: NotificationLevel,
      pub message: String,
  }
  ```
- [ ] 在 `SessionConfiguredEvent` 上新增字段（放在 `protocol/src/protocol.rs:3741-3747` 的 `initial_messages` 与 `network_proxy` 之间）：
  ```rust
  /// 一次性用户通知，例如 Windows bash 自动探测成功的提示。
  #[serde(default, skip_serializing_if = "Option::is_none")]
  #[ts(optional)]
  pub user_notification: Option<UserNotification>,
  ```
- [ ] 在手动 `Deserialize` 的 `Wire` 结构体（`protocol/src/protocol.rs:3760-3788`）中新增同名字段：
  ```rust
  #[serde(default)]
  user_notification: Option<UserNotification>,
  ```
- [ ] 在 `Wire` 反序列化后构造 `SessionConfiguredEvent` 时（`protocol/src/protocol.rs:3802-3820`）填充该字段：
  ```rust
  user_notification: wire.user_notification,
  ```
- [ ] 构建/类型检查：
  ```bash
  cargo check -p ody-protocol
  ```
  预期输出：无错误。
- [ ] 提交：
  ```bash
  git add protocol/src/protocol.rs
  git commit -m "feat(protocol): add UserNotification and extend SessionConfiguredEvent"
  ```

### Task 10: 在 `app-server-protocol` 中新增 `InfoMessageNotification` 和 `ServerNotification::InfoMessage`

**Depends on:** Task 9

**Files:**
- Modify: `app-server-protocol/src/protocol/v2/notification.rs`
- Modify: `app-server-protocol/src/protocol/common.rs`

**Implementation:**

- [ ] 在 `app-server-protocol/src/protocol/v2/notification.rs` 中，于 `ServerRequestResolvedNotification` 之后新增 `InfoMessageNotification`：
  ```rust
  #[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
  #[serde(rename_all = "camelCase")]
  #[ts(export_to = "v2/")]
  pub struct InfoMessageNotification {
      /// Human-readable informational message for the user.
      pub message: String,
  }
  ```
- [ ] 在 `app-server-protocol/src/protocol/common.rs` 的 `server_notification_definitions!` 宏中新增变体（建议放在 `Warning` 之前）：
  ```rust
  InfoMessage => "info" (v2::InfoMessageNotification),
  ```
- [ ] 构建/类型检查：
  ```bash
  cargo check -p ody-app-server-protocol
  ```
  预期输出：无错误。
- [ ] 提交：
  ```bash
  git add app-server-protocol/src/protocol/v2/notification.rs app-server-protocol/src/protocol/common.rs
  git commit -m "feat(app-server-protocol): add InfoMessage server notification"
  ```

### Task 11: 在 `app-server` 的 `thread_processor.rs` 中启动线程后发送 `InfoMessage`

**Depends on:** Task 9（`SessionConfiguredEvent.user_notification`），Task 10（`ServerNotification::InfoMessage`），core.md: Task 7（core 已填充 `user_notification`）

**Files:**
- Modify: `app-server/src/lib.rs`
- Modify: `app-server/src/request_processors/thread_processor.rs`

**Implementation:**

- [ ] 在 `app-server/src/lib.rs` 的 `ody_app_server_protocol` 导入区添加 `InfoMessageNotification`：
  ```rust
  use ody_app_server_protocol::InfoMessageNotification;
  ```
  建议放在 `use ody_app_server_protocol::ConfigWarningNotification;` 之后。
- [ ] 在 `app-server/src/request_processors/thread_processor.rs` 的 `thread/start` 新线程路径中，发送 `ThreadStarted` 通知之后（`app-server/src/request_processors/thread_processor.rs:1278` 之后），新增以下代码块：
  ```rust
  if let Some(user_notification) = session_configured.user_notification {
      listener_task_context
          .outgoing
          .send_server_notification(ServerNotification::InfoMessage(
              InfoMessageNotification {
                  message: user_notification.message,
              },
          ))
          .instrument(tracing::info_span!(
              "app_server.thread_start.notify_info",
              otel.name = "app_server.thread_start.notify_info",
          ))
          .await;
  }
  ```
  注意：该代码块必须位于 `thread/start` 新线程路径（`app-server/src/request_processors/thread_processor.rs:1135-1279`），不能位于恢复线程路径（`app-server/src/request_processors/thread_processor.rs:2600-2720`），以保证恢复线程时不重复发送。
- [ ] 构建/类型检查：
  ```bash
  cargo check -p ody-app-server
  ```
  预期输出：无错误。
- [ ] 提交：
  ```bash
  git add app-server/src/lib.rs app-server/src/request_processors/thread_processor.rs
  git commit -m "feat(app-server): forward SessionConfiguredEvent user_notification as InfoMessage"
  ```

### Task 12: 在 `tui/src/chatwidget/protocol.rs` 中处理 `ServerNotification::InfoMessage`

**Depends on:** Task 10（`ServerNotification::InfoMessage` 变体已定义）

**Files:**
- Modify: `tui/src/chatwidget.rs`
- Modify: `tui/src/chatwidget/protocol.rs`

**Implementation:**

- [ ] 在 `tui/src/chatwidget.rs` 的 `ody_app_server_protocol` 导入区添加 `InfoMessageNotification`：
  ```rust
  use ody_app_server_protocol::InfoMessageNotification;
  ```
  建议放在 `use ody_app_server_protocol::ErrorNotification;` 之后。
- [ ] 在 `tui/src/chatwidget/protocol.rs` 的 `handle_server_notification` match 中，为 `InfoMessage` 添加处理分支（建议放在 `Warning` 分支之前）：
  ```rust
  ServerNotification::InfoMessage(notification) => {
      self.add_info_message(notification.message, None);
  }
  ```
- [ ] 构建/类型检查：
  ```bash
  cargo check -p ody-tui
  ```
  预期输出：无错误。
- [ ] 提交：
  ```bash
  git add tui/src/chatwidget.rs tui/src/chatwidget/protocol.rs
  git commit -m "feat(tui): display InfoMessage server notifications"
  ```

### Task 13: protocol / app-server-protocol / app-server / tui 测试

**Depends on:** Tasks 9-12

**Files:**
- Modify: `protocol/src/protocol.rs`（测试模块）
- Modify: `app-server-protocol/src/protocol/common.rs`（测试模块）
- Modify: `app-server/src/request_processors/thread_processor.rs`（或新增测试文件）
- Modify: `tui/src/chatwidget/tests.rs` 或 `tui/src/chatwidget/tests/app_server.rs`

**Implementation:**

- [ ] 在 `protocol/src/protocol.rs` 的 `#[cfg(test)] mod tests` 中新增测试：
  ```rust
  #[test]
  fn session_configured_event_round_trips_user_notification() -> Result<()> {
      let mut event = SessionConfiguredEvent {
          session_id: SessionId::generate(),
          thread_id: ThreadId::generate(),
          forked_from_id: None,
          parent_thread_id: None,
          thread_source: None,
          thread_name: None,
          model: "kimi".to_string(),
          model_provider_id: "kimi".to_string(),
          service_tier: None,
          approval_policy: AskForApproval::OnRequest,
          approvals_reviewer: ApprovalsReviewer::default(),
          permission_profile: PermissionProfile::read_only(),
          active_permission_profile: None,
          cwd: AbsolutePathBuf::current_dir()?,
          reasoning_effort: None,
          initial_messages: None,
          user_notification: Some(UserNotification {
              level: NotificationLevel::Info,
              message: "已自动检测到 Windows bash".to_string(),
          }),
          network_proxy: None,
          rollout_path: None,
      };
      let value = serde_json::to_value(&event)?;
      let deserialized: SessionConfiguredEvent = serde_json::from_value(value)?;
      assert_eq!(deserialized.user_notification, event.user_notification);
      Ok(())
  }

  #[test]
  fn session_configured_event_deserializes_without_user_notification() -> Result<()> {
      let value = json!({
          "sessionId": SessionId::generate().to_string(),
          "threadId": ThreadId::generate().to_string(),
          "model": "kimi",
          "modelProviderId": "kimi",
          "approvalPolicy": "onRequest",
          "approvalsReviewer": "human",
          "permissionProfile": "readOnly",
          "cwd": AbsolutePathBuf::current_dir()?.to_string(),
      });
      let event: SessionConfiguredEvent = serde_json::from_value(value)?;
      assert!(event.user_notification.is_none());
      Ok(())
  }
  ```
  注意：如果 `SessionConfiguredEvent` 的字段构造复杂，可使用 `Default::default()` 或最小必填字段；上述代码假设已导入 `AskForApproval`、`PermissionProfile` 等类型。测试模块顶部已有 `use super::*;`，因此 `UserNotification` 和 `NotificationLevel` 可用。
- [ ] 在 `app-server-protocol/src/protocol/common.rs` 的 `#[cfg(test)] mod tests` 中新增测试：
  ```rust
  #[test]
  fn info_message_notification_serializes() {
      let notification = ServerNotification::InfoMessage(v2::InfoMessageNotification {
          message: "已自动检测到 Windows bash".to_string(),
      });
      let value = serde_json::to_value(&notification).expect("serialize");
      let method = value["method"].as_str().expect("method");
      assert_eq!(method, "info");
      let params = value["params"].as_object().expect("params");
      assert_eq!(
          params["message"].as_str(),
          Some("已自动检测到 Windows bash")
      );
  }
  ```
  注意：该测试依赖 `ServerNotification` 的序列化格式；如果 `server_notification_definitions!` 生成的结构不是 `{ method, params }`，请根据实际输出调整断言。可通过 `cargo test -p ody-app-server-protocol info_message_notification_serializes` 查看实际输出并修正。
- [ ] 在 `app-server` 中新增集成测试或单元测试，验证 `thread/start` 成功后转发 `InfoMessage`。由于 `thread_processor.rs` 的函数不易直接单元测试，创建 `app-server/tests/thread_start_info_notification.rs`：
  ```rust
  use ody_app_server_protocol::ServerNotification;
  use ody_protocol::protocol::{NotificationLevel, SessionConfiguredEvent, UserNotification};

  #[test]
  fn info_message_notification_can_be_constructed_from_user_notification() {
      let user_notification = UserNotification {
          level: NotificationLevel::Info,
          message: "已自动检测到 Windows bash".to_string(),
      };
      let server_notification = ServerNotification::InfoMessage(
          ody_app_server_protocol::InfoMessageNotification {
              message: user_notification.message,
          },
      );
      match server_notification {
          ServerNotification::InfoMessage(info) => {
              assert_eq!(info.message, "已自动检测到 Windows bash");
          }
          _ => panic!("expected InfoMessage variant"),
      }
  }
  ```
- [ ] 在 TUI 中新增测试，验证 `handle_server_notification` 对 `InfoMessage` 调用 `add_info_message`。在 `tui/src/chatwidget/tests.rs` 或 `tui/src/chatwidget/tests/app_server.rs` 中添加：
  ```rust
  #[test]
  fn handle_server_notification_info_message_adds_info_message() {
      let mut chat = ChatWidget::default();
      chat.handle_server_notification(
          ServerNotification::InfoMessage(InfoMessageNotification {
              message: "已自动检测到 Windows bash".to_string(),
          }),
          None,
      );
      // 验证 history 中最后一条为 info 消息。
      let last = chat.history().last().expect("history not empty");
      assert!(last.render_text().contains("已自动检测到 Windows bash"));
  }
  ```
  注意：需根据 `ChatWidget` 实际的 history 访问方法调整；若 `history()` 不存在，可改用 `assert!(chat.transcript().last_message().contains(...))` 或类似 API。执行测试时根据编译错误修正。
- [ ] 运行测试：
  ```bash
  cargo nextest run -p ody-protocol session_configured_event
  cargo nextest run -p ody-app-server-protocol info_message_notification
  cargo nextest run -p ody-app-server info_message
  cargo nextest run -p ody-tui handle_server_notification_info_message
  ```
  预期输出：所有测试通过。
- [ ] 提交：
  ```bash
  git add protocol/src/protocol.rs app-server-protocol/src/protocol/common.rs app-server/tests/thread_start_info_notification.rs tui/src/chatwidget/tests.rs
  git commit -m "test(protocol/app-server/tui): add InfoMessage notification tests"
  ```

## Self-review (Part 3)

- [ ] 1. Spec-coverage: 本 part 覆盖索引中"通过 `SessionConfiguredEvent` 向客户端发送一次用户通知"（Task 9, 11）、"恢复线程时不重复发送通知"（Task 11 仅在新线程路径发送）、"旧 rollout 反序列化兼容"（Task 9 `Wire` 结构体 `#[serde(default)]`）。
- [ ] 2. Placeholder scan: 无 TODO/TBD/deferred placeholders；所有测试均给出具体代码。
- [ ] 3. No phantom tasks: 每个 task 都产生代码或测试变更。
- [ ] 4. Dependency soundness: Task 9 无依赖；Task 10 依赖 Task 9；Task 11 依赖 Task 9、Task 10、core.md: Task 7；Task 12 依赖 Task 10；Task 13 依赖 Tasks 9-12。
- [ ] 5. Caller & build soundness: 本 part 未修改共享函数签名；新增枚举变体由 match 穷尽性检查覆盖，新增结构体字段对现有构造者无影响（通过 `#[serde(default)]` 和 `#[ts(optional)]`）。
- [ ] 6. Test-the-risk: Task 13 对序列化兼容性、通知变体构造、TUI 展示路径进行行为测试。
- [ ] 7. Type一致性: `UserNotification` / `NotificationLevel` 与 core.md: Task 7 的使用一致；`InfoMessageNotification` 的字段名 `message` 与 Task 11/12 的消费一致；`ServerNotification::InfoMessage` 方法名 `"info"` 与测试断言一致。
