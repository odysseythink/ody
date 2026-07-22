# Part 3: `SessionConfiguredEvent.user_notification` 与 TUI 提示集成

## 目标

将 config 层自动探测到 bash 并持久化的消息，通过 core 的 `SessionConfiguredEvent` 传递给 app-server，最终让 TUI 在会话启动时展示一次非阻塞的信息提示。

## 当前状态（来源确认）

- `ody_protocol::protocol::SessionConfiguredEvent` 当前没有 `user_notification` 字段（`protocol/src/protocol.rs:3688-3752`）。
- app-server 从 core 拿到 `NewThread.session_configured` 后，构造 `ThreadStartResponse` / `ThreadResumeResponse` 返回给客户端（`app-server-protocol/src/protocol/v2/thread.rs:160-191`、`393-428`）。
- app-server 的 `ServerNotification` 是一个宏生成的枚举，包含 `Warning`、`ConfigWarning`、`DeprecationNotice` 等，但没有通用的 info 通知（`app-server-protocol/src/protocol/common.rs:1454-1521`）。
- TUI 在 `chatwidget` 上已有 `add_info_message(message, hint)` 方法，被广泛用于本地和来自 app-server 的事件处理（如 `tui/src/app/app_server_events.rs:89`、`

app/config_persistence.rs:617`）。

## 设计选择

### 方案 A：SessionConfiguredEvent 新增字段 + app-server 转发为 ServerNotification（推荐）

1. 在 `ody_protocol::protocol::SessionConfiguredEvent` 中新增 `user_notification: Option<UserNotification>`。
2. core 在构造 `SessionConfiguredEvent` 时，如果 `config.shell_config_result.auto_detected_and_persisted` 为 true，则填充该字段。
3. app-server 在收到 `NewThread` 或从 rollout 恢复会话时，读取 `session_configured.user_notification`。
4. app-server 在启动响应后，向同一连接发送一个一次性的 `ServerNotification::InfoMessage`（新增）通知。
5. TUI 的 `handle_server_notification_event` 处理 `InfoMessage` 并调用 `chat_widget.add_info_message`。

**优点** [C:INFERRED]：
- 与现有事件流一致：core 产生事件，app-server 转发，TUI 消费。
- `SessionConfiguredEvent` 被持久化到 rollout 历史，通知内容也随之保留。

**缺点**：
- 需要新增 `UserNotification` 类型和 `ServerNotification::InfoMessage` 变体。
- 旧客户端（没有 `InfoMessage` 处理）会忽略该通知，但不会影响功能。

### 方案 B：直接在 ThreadStartResponse/ThreadResumeResponse 中添加字段

在 `ThreadStartResponse` 和 `ThreadResumeResponse` 中新增 `user_notification: Option<String>`，TUI 收到响应后立即调用 `add_info_message`。

**优点**：
- 无需新增 ServerNotification 变体，改动范围小。

**缺点**：
- 通知只出现在启动/恢复响应时，后续无法通过事件流重新发送。
- 与 `SessionConfiguredEvent` 设计意图不一致。

### 推荐

**方案 A** [C:USER]：在 `SessionConfiguredEvent` 中新增 `user_notification`，并通过 app-server 的 `ServerNotification` 转发到 TUI。这与原设计文件意图一致，也更通用。

## 新增数据类型

### 1. protocol crate：UserNotification

```rust
// ody_protocol/src/protocol.rs

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "")]
pub enum NotificationLevel {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "")]
pub struct UserNotification {
    pub level: NotificationLevel,
    pub message: String,
    /// Optional hint shown below the message (e.g., action to take).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub hint: Option<String>,
}
```

### 2. protocol crate：SessionConfiguredEvent 扩展

```rust
// 在 SessionConfiguredEvent 字段末尾添加

/// Optional user-facing notification emitted once when the session is configured.
/// Currently used to inform Windows users that bash was auto-detected and persisted.
#[serde(default, skip_serializing_if = "Option::is_none")]
#[ts(optional)]
pub user_notification: Option<UserNotification>,
```

反序列化时同样提供默认值 `None`，保证旧 rollout 兼容。

### 3. app-server-protocol crate：InfoMessageNotification

```rust
// app-server-protocol/src/protocol/v2/thread.rs 或新增 v2/info_message.rs

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct InfoMessageNotification {
    pub thread_id: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub hint: Option<String>,
}
```

在 `ServerNotification` 定义中新增：

```rust
server_notification_definitions! {
    ...
    InfoMessage => "infoMessage" (v2::InfoMessageNotification),
    ...
}
```

## 数据流

```
SessionConfiguredEvent.user_notification
              │
              ▼
app-server 构造 ThreadStartResponse/ThreadResumeResponse
              │
              ├─ 向客户端发送响应
              │
              ├─ 若 user_notification.is_some():
              │     发送 ServerNotification::InfoMessage
              │
              ▼
TUI 收到 ServerNotification::InfoMessage
              │
              ▼
        chat_widget.add_info_message(message, hint)
```

## 伪代码：app-server 处理

### 启动线程

```rust
// app-server/src/request_processors/thread_lifecycle.rs

let session_configured = &new_thread.session_configured;

let user_notification = session_configured.user_notification.clone();

// ... 构造 ThreadStartResponse ...
outgoing.send_response(request_id, response).await;

// 在响应之后发送一次性通知
if let Some(notification) = user_notification {
    outgoing
        .send_notification(ServerNotification::InfoMessage(InfoMessageNotification {
            thread_id: thread_id.to_string(),
            message: notification.message,
            hint: notification.hint,
        }))
        .await;
}
```

### 恢复线程

恢复线程时，`session_configured` 来自 core 从 rollout 读取的 `SessionConfiguredEvent`。如果旧 rollout 中没有 `user_notification`，则反序列化后自然为 `None`。恢复时**不重新发送**历史通知，除非该字段被显式持久化且历史非空。根据设计，该通知只在首次创建会话时产生，因此恢复时即使有也不会重复发送（可以在 app-server 中根据来源判断）。

```rust
// 恢复时：仅在 cold resume 或用户重新连接时，若 user_notification 存在且来自当前会话创建，则发送。
// 更安全的做法：恢复时不发送，因为通知是一次性的。
```

**推荐** [C:INFERRED]：恢复线程时不发送 `InfoMessage`，避免每次重连都重复提示。只有 `thread/start` 成功返回后才发送。

## 伪代码：TUI 处理

```rust
// tui/src/app/app_server_events.rs

async fn handle_server_notification_event(
    &mut self,
    app_server_client: &AppServerSession,
    notification: ServerNotification,
) {
    match &notification {
        ...
        ServerNotification::InfoMessage(info) => {
            self.chat_widget.add_info_message(info.message.clone(), info.hint.clone());
        }
        ...
    }
}
```

## 兼容性

| 方向 | 兼容性处理 |
|---|---|
| 旧 rollout 反序列化 | `SessionConfiguredEvent.user_notification` 使用 `#[serde(default, skip_serializing_if = "Option::is_none")]`，缺失时反序列化为 `None`。 |
| 旧客户端接收 `InfoMessage` ServerNotification | 旧客户端不认识该 method，会忽略。 |
| 新客户端连接旧 app-server | 旧 app-server 不会发送 `InfoMessage`，无影响。 |

## 测试断言

- 协议层：验证 `SessionConfiguredEvent` 在 `user_notification` 缺失时能正确反序列化。
- 协议层：验证 `UserNotification` 和 `InfoMessageNotification` 的 JSON 序列化/反序列化。
- app-server 层：验证当 `SessionConfiguredEvent.user_notification` 非空时，启动线程后发送 `ServerNotification::InfoMessage`。
- app-server 层：验证恢复线程时不发送 `InfoMessage`。
- TUI 层：验证收到 `ServerNotification::InfoMessage` 时调用 `add_info_message`。
- 端到端：Windows 测试环境中自动探测到 bash 后，TUI 中展示包含 bash 路径的信息消息。

## 错误处理

| 场景 | 行为 |
|---|---|
| `SessionConfiguredEvent.user_notification` 为 `None` | 不发送任何通知。 |
| app-server 发送通知失败 | 记录 warn，不影响会话启动响应。 |
| TUI 不认识 `InfoMessage` | 旧客户端忽略，新客户端正常展示。 |
| 消息包含特殊字符 | 通过 JSON 转义正常传输。 |

## 依赖上游部分

- `config.md` 提供 `ShellConfigResult.auto_detected_and_persisted`。
- `core.md` 在构造 `SessionConfiguredEvent` 时填充 `user_notification`。

## 国际化（可选，Deferred）

[C:DEFERRED] 消息字符串目前使用硬编码中文或英文。后续可考虑通过 TUI 的本地化机制根据客户端语言切换，但本次设计不在此范围。
