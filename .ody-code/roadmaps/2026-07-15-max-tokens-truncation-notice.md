# max_tokens 截断检测 + TUI 提示 Roadmap

**Goal:** 当 Chat Completions 供应商以 `finish_reason: "length"`（或变体 `"max_tokens"`）结束响应时，ody-rs 检测该截断并在 TUI 历史区显示一条警告提示，让"输出被服务端上限截断"从静默故障变为明确诊断。

**Architecture:** 信号沿现有事件管线透传：`ody-api` SSE 解析（检测点）→ `ResponseEvent::Completed` 新增 `finish_reason` 字段（信号载体）→ `model-provider` 映射为 `FinishReason::MaxTokens`（复活现有死变体）→ `core` client 回转 `ResponseEvent`（保持 `end_turn` 语义不变）→ `core` session 在 Completed 分支发 `EventMsg::Warning` → app-server/TUI 走**已存在**的 Warning 通知链路（零改动）。

**Tech Stack:** Rust workspace（ody-api / model-provider / core / tui），cargo test。

**Scope In:** Chat Completions 路径（kimi 等自定义 chat 供应商）的截断检测与提示。
**Scope Out:** 自动续写（`needs_follow_up`）、`max_tokens` 请求字段接线、Responses 路径（已有 `response.incomplete` → 硬错误）、截断 tool call 的 JSON 修复。

**Last Updated:** 2026-07-15

---

## Execution Rubric

### A. 切分粒度原则

一个 roadmap 项在以下情况必须拆分：触及 >~8 个不同文件/模块；把共享基础设施变更与叶子工作混在一起；把可独立交付的部分捆在一个标题下。每个拆分出的子任务必须**独立可测试**。

本 roadmap 的具体应用：按 **crate 边界**切分（ody-api → model-provider → core client → core session → E2E），因为信号逐 crate 透传，每个边界都是自然的编译/测试检查点。共享类型变更（`ResponseEvent::Completed` 加字段）集中在 1.1 一个子任务内完成全部调用点更新，不跨任务分摊。

### B. 模式判定准则

| 模式 | 判定标准 | 理由 |
| --- | --- | --- |
| **normal** | 机械、低风险、唯一正确解；孤立改动，无共享签名/架构决策 | normal 模式可直接改码，无需计划开销 |
| **plan** | 多步骤实现、真实依赖、共享签名/调用方扇出，或受益于逐任务 TDD 计划 | plan 模式强制依赖图 + test-first 任务列表 |
| **design** | 架构、数据模型、公开接口/契约、迁移语义存在真未知，猜错代价大 | design 模式在批准 spec 前硬锁实现 |

平局裁决：仅当存在真未知时才选更谨慎的模式（design > plan > normal），否则选更便宜的。常规工作不升级为 design。

---

## 总览

| # | 子任务 | 范围 | 模式 | Depends on | 可并行 |
| --- | --- | --- | --- | --- | --- |
| 1.1 | ody-api 检测与字段透传 | `ResponseEvent::Completed` 加 `finish_reason` + chat SSE 保留原始值 + 全仓库构造点机械补齐 | [plan] | none | — |
| 1.2 | model-provider 映射 | `finish_reason` → `FinishReason::MaxTokens` | [normal] | 1.1 | 否 |
| 1.3 | core client 回转 | `ChatEvent::Finish` → `ResponseEvent::Completed`，`end_turn` 语义保护 | [normal] | 1.2 | 否 |
| 1.4 | core session 发警告 | Completed 分支检测截断 → `EventMsg::Warning` | [normal] | 1.3 | 否 |
| 1.5 | E2E 验证 | app-server → TUI 链路验证（零代码改动） | [normal] | 1.4 | 否 |

## 依赖图

```
1.1 (ody-api: finish_reason 字段 + 全仓库构造点)
 │
 ▼
1.2 (model-provider: FinishReason::MaxTokens 映射)
 │
 ▼
1.3 (core client: end_turn 语义保护 + 字段透传)
 │
 ▼
1.4 (core session: EventMsg::Warning)
 │
 ▼
1.5 (E2E 验证)
```

**并行性说明：无并行。** 这是一条逐 crate 的信号管线：1.2 消费 1.1 创建的 `finish_reason` 字段，1.3 消费 1.2 复活的 `FinishReason::MaxTokens`，1.4 消费 1.3 透传的字段。每个 `Depends on:` 均为源码级符号依赖，非标题推断。

**事件链路全景（源码定位，执行时勿偏离）：**

```
ody-api/src/sse/chat.rs:440          ← 检测点（当前 Some(_) => Some(true) 吞掉 "length"）
ody-api/src/common.rs:88-94          ← ResponseEvent::Completed（信号载体，加字段）
model-provider/src/adapters/common.rs:121-135  ← Completed → ChatEvent::Finish 映射
core/src/client.rs:1860-1884         ← ChatEvent::Finish → Completed 回转（end_turn 保护）
core/src/session/turn.rs:2361-2388   ← Completed 分支 → sess.send_event(EventMsg::Warning)
app-server/src/bespoke_event_handling.rs:226-233  ← Warning → ServerNotification::Warning（已存在）
tui/src/chatwidget/protocol.rs:150   ← ServerNotification::Warning → on_warning（已存在）
tui/src/chatwidget/turn_runtime.rs:384           ← on_warning → 历史区 warning cell（已存在）
```

---

## 子任务详情

### Task 1.1: ody-api 检测与 `finish_reason` 字段透传 [plan]

**Depends on:** none
**模式理由:** 共享签名变更（`ResponseEvent::Completed` 加字段）扇出到全仓库 ~10 个构造点（含测试），符合 plan 模式"shared-signature/caller fan-out"标准。
**Files:**
- Modify: `ody-api/src/common.rs:88-94` — `Completed` 变体加字段
- Modify: `ody-api/src/sse/chat.rs:425-452` — 检测点保留原始 finish_reason
- Modify: `ody-api/src/sse/responses.rs:410` — Responses 构造点补字段
- Modify（机械补 `finish_reason: None`，保编译）:
  - `core/src/client.rs:1848`（Usage 事件衍生的 Completed）
  - `core/src/client.rs:1880`（ChatEvent::Finish 回转，1.3 会改为真实透传）
  - `core/src/client.rs:1888`（ChatEvent::Error 分支）
  - `core/src/client.rs:2011-2043`（websocket/relay 透传，解构 + 重发都要带字段）
  - `core/src/compact_remote_v2.rs:830`
  - `memories/write/src/runtime.rs:256`
  - 全部测试构造点（`ody-api/src/sse/responses.rs` 1224/1261/1296/1331 附近、`core/src/client_tests.rs:443` 等，以编译错误为准逐个补齐）
- Test: `ody-api/src/sse/chat_sse_tests.rs`（仿 71 行 `"stop"` 与 181 行 `"tool_calls"` 现有用例）

**Steps:**
- [ ] 写失败测试（`chat_sse_tests.rs`）：SSE 流含 `{"choices":[{"delta":{"content":"hi"},"finish_reason":"length"}]}` → 断言 `ResponseEvent::Completed { end_turn: Some(true), finish_reason: Some("length"), .. }`。另加 `"max_tokens"` 变体用例断言相同行为。运行 `cargo test -p ody-api chat_sse` 确认 FAIL（字段不存在，编译错误）。
- [ ] 实现 1：`common.rs` `Completed` 加 `pub finish_reason: Option<String>`（doc 注释说明：线格式原始结束原因，目前仅 Chat Completions 路径填充；`"length"`/`"max_tokens"` 表示输出被服务端上限截断）。
- [ ] 实现 2：`chat.rs` 在 440-444 的 match 之后构造 Completed 时填 `finish_reason: finish_reason.clone()`（`end_turn` 计算逻辑**一字不改**——`Some(_) => Some(true)` 保持不变，这是本 roadmap 的硬约束）。
- [ ] 实现 3：其余全部构造点补 `finish_reason: None`（client.rs:2011 透传点填解构出的字段值）。
- [ ] 运行 `cargo test -p ody-api chat_sse` PASS；运行 `cargo check --workspace --all-targets` 全树绿灯（含测试 target，确保无遗漏构造点）。
- [ ] Commit。

**验证要点:** `must-survive` 输入枚举——`finish_reason: "stop"`（→ `end_turn: Some(true)`，现有 71 行测试）、`"tool_calls"`（→ `Some(false)`，现有 181 行测试）、无 `finish_reason` 且有 tool calls（→ `Some(false)`）。三个现有用例断言不得被本改动破坏。

---

### Task 1.2: model-provider 映射 `FinishReason::MaxTokens` [normal]

**Depends on:** 1.1
**模式理由:** 单文件、唯一正确解的机械映射 + 单测，无共享签名变更。
**Files:**
- Modify: `model-provider/src/adapters/common.rs:121-135`（`ResponseEvent::Completed` → `ChatEvent::Finish` 的映射）
- Test: 同文件内 `#[cfg(test)]` 模块（450/483/504 行附近有现成的 `ChatEvent::Finish` 断言模式可仿）

**Steps:**
- [ ] 写失败测试：构造 `ResponseEvent::Completed { end_turn: Some(true), finish_reason: Some("length".into()), .. }` 输入映射函数 → 断言输出 `ChatEvent::Finish { reason: FinishReason::MaxTokens, raw_reason: Some("length") }`。运行 `cargo test -p ody-model-provider` 确认 FAIL（当前映射只产出 Stop/Other）。
- [ ] 实现：121-125 行的 `reason` 计算改为——`finish_reason` 为 `Some("length")` 或 `Some("max_tokens")` 时 `FinishReason::MaxTokens`；否则维持原逻辑（`Some(true) => Stop`，`Some(false) => Other("incomplete")`，`None => Stop`）。`raw_reason` 填 `finish_reason` 原始值。
- [ ] 运行 `cargo test -p ody-model-provider` PASS。
- [ ] Commit。

**验证要点:** `must-survive` 输入——`end_turn: Some(false)`（pause/incomplete，现有语义）仍映射 `Other("incomplete")`；`finish_reason: Some("stop")` 仍映射 `Stop` 而非 MaxTokens（匹配只认 `"length"`/`"max_tokens"` 两个字面值，不做子串/大小写模糊匹配）。

---

### Task 1.3: core client 回转与 `end_turn` 语义保护 [normal]

**Depends on:** 1.2
**模式理由:** 单文件 2 处改动，但含一处正确性关键的单行守卫（`end_turn` 映射），强制行为测试覆盖；无共享签名变更。
**Files:**
- Modify: `core/src/client.rs:1879-1884`（`ChatEvent::Finish` → `ResponseEvent::Completed`）
- Test: `core/src/client_tests.rs`（676 行附近有 ChatProvider 流式测试 harness，`test_model_info_for_chat_provider` 于 730 行）

**Steps:**
- [ ] 写失败测试：mock ChatProvider 产出 `ChatEvent::Finish { reason: FinishReason::MaxTokens, raw_reason: Some("length") }` → 断言核心侧输出 `ResponseEvent::Completed { end_turn: Some(true), finish_reason: Some("length"), .. }`。**两个断言缺一不可**：`end_turn` 为 `Some(true)`（防自动续写回归）、`finish_reason` 被保留。运行 `cargo test -p ody-core client` 确认 FAIL（当前 `matches!(reason, FinishReason::Stop)` 会给出 `end_turn: Some(false)`）。
- [ ] 实现：1879 行改为 `let end_turn = matches!(reason, FinishReason::Stop | FinishReason::MaxTokens);`，Completed 构造填 `finish_reason: raw_reason`（透传 1.2 填的原始值）。
- [ ] 运行测试 PASS。
- [ ] Commit。

**验证要点（本 roadmap 最高风险行）:** `end_turn == Some(false)` 是 `turn.rs:2381` 触发 `needs_follow_up = true`（自动续写）的唯一条件。本任务必须保证 `MaxTokens` **不**落入 `Some(false)`——这正是用户划定的 scope 红线（只检测提示，不自动续写）。

---

### Task 1.4: core session 发 `EventMsg::Warning` [normal]

**Depends on:** 1.3
**模式理由:** 单分支插入 + 现成 `EventMsg::Warning` 模式（core 内已有 ~26 处发射点可仿，如 `core/src/compact.rs:349`）。
**Files:**
- Modify: `core/src/session/turn.rs:2361-2388`（`ResponseEvent::Completed` 分支）
- Test: `core/src/session/tests.rs`（会话测试 harness）

**Steps:**
- [ ] 写失败测试：驱动会话收到 `Completed { end_turn: Some(true), finish_reason: Some("length"), .. }` → 断言会话事件队列中出现 `EventMsg::Warning`，且 `message` 含 `"length"`；同时断言回合正常结束（无 follow-up 请求）。运行 `cargo test -p ody-core session` 确认 FAIL。
- [ ] 实现：Completed 分支内、`break Ok(...)` 之前，若 `finish_reason.as_deref()` 为 `Some("length") | Some("max_tokens")`，执行：
  ```rust
  sess.send_event(
      &turn_context,
      EventMsg::Warning(ody_protocol::protocol::WarningEvent {
          message: format!(
              "Model hit the provider's max output token limit (finish_reason={reason}); \
               the response may be incomplete.",
              reason = reason
          ),
      }),
  )
  .await;
  ```
- [ ] 运行测试 PASS；`cargo check --workspace --all-targets` 绿灯。
- [ ] Commit。

**验证要点:** 文案**不得**引导用户修改 `max_output_size` 配置——ody-rs 该字段当前无运行时效果（已调查确认），引导会造成误导。文案只陈述事实（被截断 + 原始 reason 值）。

---

### Task 1.5: E2E 验证（app-server → TUI） [normal]

**Depends on:** 1.4
**模式理由:** 纯验证任务——下游链路三个环节已存在（grep 确认），零代码改动。
**Files:**
- 验证用（只读确认）:
  - `app-server/src/bespoke_event_handling.rs:226-233` — `EventMsg::Warning` → `ServerNotification::Warning`
  - `tui/src/chatwidget/protocol.rs:150` — `ServerNotification::Warning` → `self.on_warning(...)`
  - `tui/src/chatwidget/turn_runtime.rs:384` — `on_warning` → 历史区 warning cell
- 可选 Test: `app-server/tests/` 下加集成测试（mock core 发 Warning → 断言客户端收到 WarningNotification）

**Steps:**
- [ ] 手工验证：构造返回 `finish_reason: "length"` 的 mock chat 端点（或拦截代理改写响应），用 kimi chat 供应商跑一轮会话 → TUI 历史区出现黄色 warning cell，回合正常结束，无额外请求发出。
- [ ] 确认 `tui/src/app/replay_filter.rs:28` 的行为符合预期：Warning 通知**不进回放**——会话恢复/回放时看不到该提示（直播-only 提示，本 roadmap 接受此语义，文档化即可）。
- [ ] `cargo test --workspace` 全量回归绿灯。
- [ ] Commit（若只加集成测试则提交测试，否则以验证记录收尾，不允许 `--allow-empty`）。

---

## 风险与开放问题

1. **`needs_follow_up` 红线（最高风险）:** `end_turn: Some(false)` 是自动续写的唯一触发器（`turn.rs:2381`）。1.1 不改 `end_turn` 计算、1.3 显式把 `MaxTokens` 归入 `Some(true)`，双重防护。
2. **匹配范围:** 仅字面值 `"length"` 与 `"max_tokens"`（用户已确认）。GLM/Kimi/DeepSeek 的 OpenAI 兼容端点均遵循 `"length"` 标准；遇到新变体时在 1.1/1.2 两处同步扩表。
3. **回放不可见:** Warning 通知被 `replay_filter.rs:28` 过滤，提示仅直播可见。若未来需要持久化提示，需另开 roadmap（改走 history item 而非通知）。
4. **Responses 路径:** `response.incomplete` 已转硬错误（`ody-api/src/sse/responses.rs:395-405`），不在本范围；截断 tool call 的 JSON 修复亦不在范围。

---

## Self-Review

- [x] 每个过大的阶段已拆分；无子任务捆绑可独立交付的工作。
- [x] 每个 `Depends on:` 指向更早的子任务，且有真实 grep/Read 依据（符号级：`finish_reason` 字段、`FinishReason::MaxTokens`、`end_turn`），非标题猜测。
- [x] 并行性已明确说明（无并行，管线式串行，理由已述）。
- [x] 每个子任务有且仅有一个模式标签，理由基于代码实际要求（共享签名扇出 → plan；机械映射 → normal）。
- [x] Rubric 块位于文档顶部，后续修订按同一准则执行。
- [x] 一次性批量完成全部标注（单一 rubric 通过后一轮 sweep），未逐阶段重复裁决。
