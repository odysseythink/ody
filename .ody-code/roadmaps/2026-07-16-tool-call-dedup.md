# 工具重复调用检测：探索报告 + Rust 端实施 Roadmap

**Goal:** 让 ody-rs 拥有与 TS 参照实现（`D:\workspace\go_work\ody-code`，opencode 系 fork）行为一致的工具重复调用检测：同一 step 内相同调用短路复用结果；跨 step 连续重复到 3/5/8 次时在工具结果尾部注入 `<system-reminder>` 软提醒。不中断 turn、不要求用户确认、不可配置。
**补充目标（2026-07-23）：** 增加硬熔断兜底机制 M4：当跨 step 连续重复同一工具调用达到阈值（默认 8 或 10）时，强制中断当前 turn，避免模型在 reasoning 已意识到循环的情况下仍持续无意义调用并消耗大量 token。

**Architecture:** 复活在库死代码 `core/src/tools/tool_call_dedup.rs`（`ToolCallDeduplicator`），状态归属 `run_turn` 外层循环，以 `&mut` 逐层穿线到流式事件处理；streak 提醒在 spawn 工具 future 时注册、在 future resolve 后叠加到 `ResponseInputItem`；same-step 短路用 `Arc<OnceCell<ResponseInputItem>>` 共享槽复用首个调用的真实结果。全部行为决策以 TS 版 `packages/agent-core/src/agent/turn/tool-dedup.ts` 为准。

**Tech Stack:** Rust workspace（ody-core / ody-tools / ody-protocol），cargo nextest。

**Scope In:** 直接模型工具调用（`ToolCallSource::Direct`，经 `handle_output_item_done` 分发的全部调用）；streak 检测 + 提醒注入；参数规范化；same-step 短路。
**Scope Out:** 遥测事件 `tool_call_dedup_detected`（用户已确认不做）；code-mode worker 内部发起的工具调用（不经 `handle_output_item_done`，TS 版同等语义之外的差异，文档化接受）；guardian/审批拒绝路径（`RespondToModel` 不进工具分支，本就不计入 streak，无需改动）。**M4 硬熔断阈值可配置；M1-M3 的 3/5/8 提醒阈值仍硬编码以对齐 TS。**

**Last Updated:** 2026-07-16
**Revised:** 2026-07-23（新增 M4 硬熔断方案）

---

## 探索报告（2026-07-16，两仓库源码调查结论）

### Rust 端现状（E:/ody-rs）：实现存在但为死代码

- `core/src/tools/tool_call_dedup.rs`（331 行）由 commit `b313cdf`（2026-07-12）引入，但 `core/src/tools/mod.rs:1-16` 的模块声明列表中**没有 `mod tool_call_dedup;`**，全 workspace（含 BUILD.bazel）对其类型名/文件名的引用为零——孤儿文件，rustc 不编译，文件内 7 个单元测试不运行。
- 当前 turn 主循环（`core/src/session/turn.rs:266`）**没有任何生效的重复调用检测，也没有每 turn 工具步数上限**。兜底仅有：空完成重试（turn.rs:155-162，上限 3 次）、Guardian 审批熔断（`core/src/guardian/mod.rs:49`，连续拒绝 3 次断 turn）、token 上限自动压缩。
  - 注：M4 硬熔断将填补此兜底缺口，但 M4 之前仍无每 turn 工具步数上限。
- 死代码 API 已定型且引用类型全部仍有效（已逐一核实）：`ody_protocol::models::*`（`protocol/src/models.rs:1870` 起，`to_text`/`from_text` 俱在）、`ody_tools::{ToolName, ToolPayload}`（`tools/src/lib.rs:28,99` 再导出；`ToolPayload` 三变体 `tools/src/tool_payload.rs:7` 与死代码 match 臂完全一致）、`crate::tools::flat_tool_name`（`core/src/tools/mod.rs:39`）。
- 关键设计细节：`append_reminder`（tool_call_dedup.rs:99-115）**不使用 self 状态**（仅用 payload 取参数串），可平移为关联函数——这是适配 `'static` 工具 future 的前提（见 M1.2 风险 3）。阈值 3/5/8 硬编码（:106-111）；判重 key 为 `工具名 + 参数原文` 字符串拼接（:118-131），对 JSON key 顺序/空白敏感。

### TS 端参照（D:/workspace/go_work/ody-code）：已接线生效

- 实现：`packages/agent-core/src/agent/turn/tool-dedup.ts`（class `ToolCallDeduplicator`，L88-207），接线于 `turn/index.ts:546-705` 的四个 loop hook（`beforeStep` :603 / `prepareToolExecution` :645 / `finalizeToolResult` :657 / `afterStep` :609），单测 `test/agent/turn/tool-dedup.test.ts`（364 行）。
- 与 Rust 死代码的两点增强：
  1. **判重 key 规范化**（`turn/canonical-args.ts:5-8`）：参数对象递归 key 排序后 JSON 序列化，`{a:1,b:2}` 与 `{b:2,a:1}` 判为同一调用。
  2. **same-step 短路**（tool-dedup.ts:75,140-154,164-206）：同一次响应内首个调用正常执行，后续相同 key 调用**不执行工具**，先拿占位结果，`finalizeResult` 阶段用 Deferred 替换为首个调用的真实结果；副作用是同步内刷屏不触发提醒（已短路）。
- 相同点：跨 step 严格连续 streak（夹任何不同调用即重置，非滑动窗口）；阈值 3/5/8；仅注入 `<system-reminder>` 软提醒，从不中断循环；无配置项。
- 遥测（本 roadmap 不移植）：`turn/index.ts:777-806` 的 `trackDuplicateToolCall`，上报 `tool_call_dedup_detected` 事件（含 sha256 前 8 位 `args_hash`），只统计不改行为。

### 对比速览

| | TS 版（参照） | Rust 版（现状） |
| --- | --- | --- |
| 状态 | ✅ 接线生效 | ❌ 孤儿文件，零运行时效果 |
| 参数判重 | 递归排序序列化，不怕 key 顺序 | 拼原文，对 key 顺序/空白敏感 |
| 同步内重复 | 短路复用首个结果 | 无机制 |
| 阈值/行为 | 3/5/8 软提醒，不中断 | 同设计（但无运行时效果） |

---

## Execution Rubric（2026-07-16 经用户批准）

### A. 切分粒度原则

按"机制层"切分，每层独立可测试、独立可交付、独立可回滚：

- **M1 复活接线**：声明模块 + turn 循环穿线，让既有 streak 检测生效。只改文本（往工具结果尾部追加提醒），零副作用。
- **M2 参数规范化**：递归 key 排序，对齐 TS 判重精度。纯函数改动，不动控制流。
- **M3 same-step 短路**：同步内重复调用不执行、复用首个结果。**改变工具实际执行次数**（写文件/bash 等副作用工具的调用次数会真实减少），副作用风险最高，独立放最后。
- **M4 硬熔断（2026-07-23 新增）**：当跨 step 连续重复达到阈值时强制中断 turn，防止模型在软提醒无效时持续无意义循环。涉及控制流变更，独立为最后阶段，不影响 M1-M3 交付。

拆分理由：M3 与 M1 混在一起会让"提醒机制生效"的验证被"执行次数变化"污染；M2 若并入 M1 则接线 diff 难以 review。各层均 ≤4 个文件，无 >8 文件的过大项，无需再拆。

### B. 模式判定准则

| 模式 | 判定标准 | 理由 |
| --- | --- | --- |
| **normal** | 机械、低风险、唯一正确解；孤立改动，无共享签名/架构决策 | normal 模式可直接改码，无需计划开销 |
| **plan** | 多步骤实现、真实依赖、共享签名/调用方扇出，或受益于逐任务 TDD 计划 | plan 模式强制依赖图 + test-first 任务列表 |
| **design** | 架构、数据模型、公开接口/契约、迁移语义存在真未知，猜错代价大 | design 模式在批准 spec 前硬锁实现 |

本 roadmap 不使用 design：TS 参照实现已钉死全部行为决策（key 构造、阈值、文案、短路语义），Rust 接线点已源码定位（见下），无真未知。

---

## 总览

| # | 子任务 | 范围 | 模式 | Depends on | 可并行 |
| --- | --- | --- | --- | --- | --- |
| M1.1 | 复活模块编译 | `mod.rs` 声明 + 漂移修复 + 7 个内嵌单测转绿 | [normal] | none | — |
| M1.2 | turn 循环穿线接线 | `run_turn` 持有状态、`&mut` 穿三层签名、begin/end_step、register + 提醒包装 | [plan] | M1.1 | 否 |
| M2.1 | 参数规范化 | 递归 key 排序 canonical 序列化 + 非法 JSON 回退 | [normal] | M1.1 | 否（与 M1.2 同文件，串行） |
| M3.1 | same-step 短路 | OnceCell 结果复用 + 分支逻辑 + call_id 改写 + 泄漏兜底 | [plan] | M1.2, M2.1 | 否 |

## 依赖图

```
M1.1 (复活模块：mod 声明 + 编译修复)
 │
 ├──────────────┐
 ▼              ▼
M1.2 (穿线接线：streak 生效)   M2.1 (canonical 参数)
 │              │
 └──────┬───────┘
        ▼
M3.1 (same-step 短路复用)
```

**并行性说明：无可并行。** M1.2 与 M2.1 理论上只共享 M1.1，但两者都修改 `tool_call_dedup.rs`（M1.2 要把 `append_reminder` 改为关联函数，M2.1 改 `payload_arguments_string`），并行必冲突，串行。M3.1 依赖 M1.2 的 begin/end_step 生命周期与 M2.1 的 canonical key（短路判重必须用规范化后的 key，否则 `{a,b}` 顺序不同就绕过短路）。

**接线点全景（源码定位，执行时勿偏离）：**

```
core/src/session/turn.rs:266            ← run_turn 循环（dedup 状态创建点，循环之前）
core/src/session/turn.rs:331            ← run_sampling_request 唯一调用点（&mut 透传）
core/src/session/turn.rs:1411           ← run_sampling_request 签名（重试循环 :1441 内调 try_）
core/src/session/turn.rs:2057           ← try_run_sampling_request 签名
core/src/session/turn.rs:2115           ← 流式事件循环（此前 begin_step）
core/src/session/turn.rs:2199           ← HandleOutputCtx 构造点（填 dedup 字段）
core/src/stream_events_utils.rs:315-321 ← HandleOutputCtx 定义（加 &mut 字段）
core/src/stream_events_utils.rs:415-443 ← 工具调用分支（register + 包装 future / M3 短路点）
core/src/session/turn.rs:2236-2237      ← tool_future 入 in_flight（FuturesOrdered 保序）
core/src/session/turn.rs:2543           ← drain_in_flight（其后 end_step）
core/src/tools/parallel.rs:63-79        ← handle_tool_call（Fatal→Err，其余→failure_response :186）
core/src/tools/router.rs:29-33          ← ToolCall { tool_name, call_id, payload }
core/src/tools/mod.rs:1-16              ← 模块声明列表（插 mod tool_call_dedup;）
core/src/session/tests.rs:7928/7956/7985/8013/9496 ← HandleOutputCtx 测试构造点（需补字段）

TS 参照（只读对照）:
packages/agent-core/src/agent/turn/tool-dedup.ts     ← 行为 spec（L42-44 key、L198-202 阈值）
packages/agent-core/src/agent/turn/canonical-args.ts ← 规范化 spec
packages/agent-core/test/agent/turn/tool-dedup.test.ts ← 测试场景 spec
```

---

## 子任务详情

### Task M1.1: 复活模块编译 [normal]

**Depends on:** none
**模式理由:** 单行模块声明 + 编译错误最小修复，唯一正确解，无共享签名变更。
**Files:**
- Modify: `core/src/tools/mod.rs:1-16` — 按字母序在 `tool_dispatch_trace` 前插 `pub(crate) mod tool_call_dedup;`
- Modify（仅当漂移导致编译错误时，最小修复）: `core/src/tools/tool_call_dedup.rs`
- Test: 文件内既有 7 个单测（tool_call_dedup.rs:180-331），无需新增

**Steps:**
- [ ] 声明模块，运行 `cargo nextest run -p ody-core tool_call_dedup`。若编译失败（4 天漂移），以编译错误为准逐个最小修复（类型路径/签名以现状为准，**不改任何行为逻辑**）。
- [ ] 7 个内嵌单测全绿：`no_reminder_for_first_two_repeats` / `reminder_at_third_repeat` / `detailed_reminder_at_fifth_repeat` / `different_args_reset_streak` / `cross_step_streak_continues` / `cross_step_different_args_resets` / `reminder_appended_to_content_items`。
- [ ] `cargo check -p ody-core --all-targets` 绿灯。
- [ ] Commit。

**验证要点:** 本任务**只允许**让文件编译并通过其既有测试；任何"顺手优化"（含 M2 的规范化）一律禁止混入，保证 M1.2 的 diff 可独立 review。

---

### Task M1.2: turn 循环穿线接线（streak 检测生效） [plan]

**Depends on:** M1.1
**模式理由:** 跨 3 文件签名穿线（run_sampling_request / try_run_sampling_request / HandleOutputCtx）+ 并发 future 包装的正确性约束（'static 边界、FuturesOrdered 保序），符合 plan 模式"shared-signature + 多步骤 TDD"标准。
**Files:**
- Modify: `core/src/tools/tool_call_dedup.rs:99-115` — `append_reminder` 去 `&self` 改关联函数（其本不使用 self 状态）
- Modify: `core/src/session/turn.rs` — `run_turn`（:266 循环前创建状态）、`run_sampling_request`（:1411 签名 + :331 调用点透传）、`try_run_sampling_request`（:2057 签名；:2115 流循环前 `begin_step()`；:2543 drain 后 `end_step()`）、:2199 ctx 构造
- Modify: `core/src/stream_events_utils.rs:315-321`（ctx 加 `dedup: &mut ToolCallDeduplicator` 字段）、:415-443（工具分支：register + 包装 future）
- Modify: `core/src/session/tests.rs` — HandleOutputCtx 测试构造点（:7928/7956/7985/8013/9496 附近，以编译错误为准）补 `dedup: &mut ToolCallDeduplicator::new()`
- Test: `core/src/session/tests.rs` 会话级测试（仿 :7928 既有 `handle_output_item_done` harness 或会话流 harness）

**Steps:**
- [ ] 写失败测试：驱动模型连续 3 个 step 各输出 1 次相同 `(tool, args)` 调用 → 断言第 3 个 `FunctionCallOutput` 文本含 `<system-reminder>` 且含 "repeating the exact same tool call"，第 1/2 个不含；再断言第 5 次含 `repeated_times: 5` 明细。运行 `cargo nextest run -p ody-core` 确认 FAIL（未接线）。
- [ ] 实现 1：`append_reminder` 改关联函数 `fn append_reminder(response, tool_name, payload, streak)`（去 `&self`），更新文件内单测调用方式。
- [ ] 实现 2：`run_turn` 在 :266 循环前 `let mut dedup = ToolCallDeduplicator::new();`，逐层 `&mut dedup` 穿 `run_sampling_request` → `try_run_sampling_request`。
- [ ] 实现 3：`try_run_sampling_request` 流循环（:2115）之前 `dedup.begin_step()`；`drain_in_flight`（:2543）成功返回后 `dedup.end_step()`。
- [ ] 实现 4：工具分支（stream_events_utils.rs:415-443）spawn 前 `let streak = ctx.dedup.register(&call.call_id, &call.tool_name, &call.payload);`；克隆 `tool_name`/`payload` 捕获进包装 future，resolve 后 `Ok(item) => Ok(ToolCallDeduplicator::append_reminder(item, &name, &payload, streak))`，`Err` 原样透传。
- [ ] 运行新测试 PASS；`cargo nextest run -p ody-core` 全绿；`cargo check --workspace --all-targets` 绿灯。
- [ ] Commit。

**验证要点（正确性关键）:**
1. **`'static` 边界（最高风险）:** `in_flight` 是 `FuturesOrdered<BoxFuture<'static, ...>>`（turn.rs:2096），future 不得借用 `ctx.dedup`——只能按值捕获 `(tool_name, payload, streak)` + 调关联函数。若实现中出现"往 future 里塞 `&'static mut` / `Arc<Mutex>`"即走错路。
2. **提醒只对工具输出生效:** `append_reminder_to_response`（tool_call_dedup.rs:133-152）仅处理 `FunctionCallOutput`/`CustomToolCallOutput`，其余 `ResponseInputItem` 原样返回——包装层不得扩展此范围。
3. **must-survive 输入:** 审批拒绝/参数错误走 `build_tool_call` 的 `Err(RespondToModel)` 分支（stream_events_utils.rs:487），**不进**工具分支、不计入 streak——新测试需含一例"同一调用被拒绝两次后第三次正常执行，streak 从 1 起算"。
4. **保序:** FuturesOrdered 按 push 顺序 resolve，register 顺序与结果记录顺序一致，不得改用 `FuturesUnordered`。

**已查明的边界情形（接受，测试锁定即可）:** SSE 流重试会重入 `try_run_sampling_request` → `begin_step` 对同一逻辑 step 执行两次，已 register 的调用在未 `end_step` 前被清空，streak 欠计（保守方向，与 TS 版 beginStep 语义一致）；错误路径 `break Err` 跳过 `end_step`，同样欠计。两者均为"少提醒"而非"误提醒"，接受。

---

### Task M2.1: 参数规范化（canonical key） [normal]

**Depends on:** M1.1
**模式理由:** 单文件纯函数 + 单测；唯一坑点（`preserve_order`，见下）已查明并有定解，无未知。
**Files:**
- Modify: `core/src/tools/tool_call_dedup.rs:123-131`（`payload_arguments_string` 的 `Function` 分支改用 canonical 序列化）
- Test: 同文件 `#[cfg(test)]` 模块新增用例

**Steps:**
- [ ] 写失败测试：`register("c1", {"a":1,"b":2})` 后 `register("c2", {"b":2,"a":1})` → 断言返回 streak 2（当前实现返回 1）；嵌套对象 key 乱序同值判同；数组**保序**（`[1,2]` ≠ `[2,1]`）；非法 JSON 回退原文比较（不 panic）；`ToolSearch`/`Custom` 行为不变。运行确认 FAIL。
- [ ] 实现：`fn canonical_arguments(args: &str) -> String`——`serde_json::from_str::<Value>` 成功则递归重建 Value（对象：key 收集到 `BTreeMap` 排序后按序插入新 `serde_json::Map`；数组：保序递归映射），再 `to_string`；解析失败原样返回 `args.to_owned()`。`Function` 分支改调它。
- [ ] 运行测试 PASS；`cargo nextest run -p ody-core tool_call_dedup` 全绿。
- [ ] Commit。

**验证要点（本任务唯一深坑）:** `tui/Cargo.toml:91` 开启了 `serde_json/preserve_order`，Cargo 特性统一会让**整个构建**的 `serde_json::Map` 变为 IndexMap（插入序）——**禁止**依赖 `Value` 默认序列化"自动按 key 排序"（那只在没有该特性时成立，且会让测试随构建组合翻转）。必须按上述"排序后按序插入新 Map"显式构造，两种特性状态下行为一致。另一已查明分歧：`1` vs `1.0` 在 serde_json 中序列化为 `1`/`1.0`（判不同），而 TS `JSON.stringify` 均为 `1`（判相同）——接受此分歧，写进函数 doc 注释。

---

### Task M3.1: same-step 短路去重 [plan]

**Depends on:** M1.2, M2.1
**模式理由:** 侵入工具分发路径（首个/重复分支分流）、跨 spawn future 的结果复用（OnceCell 全路径填充保证）、协议正确性关键（call_id 改写），符合 plan 模式标准。
**Files:**
- Modify: `core/src/tools/tool_call_dedup.rs` — 新增 `same_step: HashMap<String, Arc<OnceCell<ResponseInputItem>>>` 状态与 `check_same_step` API；`begin_step` 清理时对未填充槽兜底填占位失败结果（对齐 TS 的 leaked deferred 处理，tool-dedup.ts 注释 L66-75）
- Modify: `core/src/stream_events_utils.rs:415-443` — 分支：首个调用正常 spawn + resolve 时全路径填槽；重复调用不 spawn `handle_tool_call`，spawn 等待槽的 future
- Test: `core/src/session/tests.rs`（计数 mock 工具断言执行次数）；仿 TS 单测场景（tool-dedup.test.ts L198-211 等）

**Steps:**
- [ ] 写失败测试：① 同一 step 两个相同调用 → mock handler 只执行 **1** 次，历史里两条 `FunctionCallOutput` 文本相同**且各自 call_id 正确**；② 原调用失败（handler 返回错误）→ 重复方也拿到失败文本且不挂起（有超时断言）；③ 相同调用分处两个 step → 执行 2 次（跨 step 不复用）；④ 同步内 3 次相同调用 → 不出现 streak 提醒（已短路，对齐 TS 语义）。运行确认 FAIL。
- [ ] 实现 1：`ToolCallDeduplicator` 加 `same_step` 槽表；`register` 改为返回枚举 `First { streak } | Duplicate { streak, slot }`（或并列方法 `check_same_step`），`begin_step` 先对残留未填槽填占位失败文本再清空。
- [ ] 实现 2：分支改造——`First`：spawn 真实执行，包装 future 在 `Ok`/`Err` **全路径**填槽（`Ok` 填结果 clone；`Err`（含 Fatal）填占位失败结果，防重复方悬挂）；`Duplicate`：spawn 等待 future，`slot.await` → clone 结果 → **改写 `call_id` 为本调用的 id** → 叠加 M1.2 的 reminder 包装（顺序：先复用结果，后叠加提醒，与 TS `finalizeResult` 顺序一致）。
- [ ] 运行测试 PASS；`cargo nextest run -p ody-core` 全绿；`cargo check --workspace --all-targets` 绿灯。
- [ ] Commit。

**验证要点:**
1. **call_id 改写（协议正确性最高风险）:** 复用的 `ResponseInputItem` 携带的是**原调用**的 `call_id`，直接入历史会与原调用的 output 冲突——必须深拷贝改写为重复调用自己的 `call_id`。TS 版在 `finalizeResult` 做了等价处理，Rust 版无现成步骤，是本任务唯一的非平凡创新点。
2. **全路径填槽:** `handle_tool_call`（parallel.rs:63-79）在 Fatal 时返回 `Err(OdyErr)`，包装层若只在 `Ok` 填槽，重复方 future 永久悬挂 → `drain_in_flight` 卡死整个 turn。`Err` 路径必须填占位失败结果（文本如 "The original tool call this duplicates failed."），测试 ② 锁定。
3. **短路范围:** 仅跳过 `handle_tool_call` 执行，会话事件（`record_completed_response_item` 等，stream_events_utils.rs:432）维持原样——重复调用的"已发起"记录与事件流不变，仅结果复用。
4. **must-survive:** 不同 key 调用不受影响（槽表 miss → First 路径一字未动）；`ToolCallRuntime` 并发锁语义（parallel.rs:36,115-119）不变。

---

### Task M4.1: 重复工具调用硬熔断 [plan]

**Depends on:** M1.2, M2.1
**模式理由:** 涉及 turn 主循环控制流修改（是否中断 turn）、错误结果注入、协议事件分发和 TUI 用户反馈，需要跨模块协调和接口设计，符合 plan 模式标准。
**背景:** 2026-07-23 实际 session（`C:\Users\hkb819\.ody-code\sessions\2026\07\23\rollout-2026-07-23T10-29-11-019f8cce-30e5-7270-8d17-1a37eacc546a.jsonl`）证明：模型在 reasoning 中已反复意识到"I keep sending no-op commands"，但工具调用层持续 150+ 次输出 `{"command":": "}`，消耗约 29M cached tokens。M1-M3 的软提醒对这类"reasoning 清醒但工具调用生成层瘫痪"的故障无自愈能力，需要硬熔断兜底。

**Files:**
- Modify: `core/src/tools/tool_call_dedup.rs` — 新增 `fuse_threshold` 与 `fuse_count` 状态；暴露 `should_fuse(tool_name, args)` / `record_fuse()` API，当跨 step 连续重复达到阈值时返回熔断决策。
- Modify: `core/src/stream_events_utils.rs` — 在 `handle_output_item_done` 分发工具调用前调用 `should_fuse`；若熔断，跳过真实工具执行，生成错误 `FunctionCallOutput`（`is_error: true`），并向外层 turn 循环发送 `ToolCallFused` 中断信号。
- Modify: `core/src/session/turn.rs` — 在 `run_turn` / `run_sampling_request` 外层循环中捕获 `ToolCallFused`，以可控方式结束当前 turn（类似 `TurnAborted`，但原因标记为 `tool_call_fused`）。
- Modify: `tui/src/chatwidget.rs` 或 `tui/src/app/event_dispatch.rs` — 接收 `tool_call_fused` 事件，在状态栏/聊天区展示熔断信息，并允许用户继续输入新指令。
- Test: `core/src/tools/tool_call_dedup_tests.rs` 或 `core/src/session/tests.rs` — 构造连续 8/10 次相同 mock 工具调用，断言第 N 次触发熔断、turn 结束、不继续消耗 token。

**Steps:**
- [ ] 写失败测试：① 连续 8 次相同 `shell_command` 调用 → 第 8 次触发 fuse，返回错误结果，turn 中断；② 中间夹 1 次不同调用 → streak 重置，不触发 fuse；③ 参数不同（如 `{"command":"git status"}` vs `{"command":"git commit"}`）→ 不判为同一调用；④ 配置 `fuse_threshold = 0` → 关闭熔断；⑤ 配置 `fuse_threshold = 3` → 第 3 次即触发。运行确认 FAIL。
- [ ] 实现 1：`ToolCallDeduplicator` 增加 `fuse_threshold: usize`（默认 8 或 10，可配置）与 `fuse_triggered: bool`；在 `end_step` 更新 streak 时判断 `consecutive_count >= fuse_threshold && fuse_threshold > 0`，触发则设置 `fuse_triggered = true` 并记录最后一次 key。
- [ ] 实现 2：`stream_events_utils.rs` 在 `handle_output_item_done` 中，对每个工具调用先 `deduper.should_fuse(tool_name, args)`；若返回 true，直接生成 `FunctionCallOutput`：`is_error: true`，output 文本包含 `"Tool call fuse triggered: repeated shell_command 8 times with identical arguments. Turn interrupted to prevent infinite loop."`，并调用 `turn_context.request_fuse()`（或等价信号）。
- [ ] 实现 3：`turn.rs` 外层循环在每次采样前检查 `turn_context.fuse_requested()`；若已触发，跳出循环并返回 `TurnOutcome::Fused { reason, repeated_call_summary }`。
- [ ] 实现 4：TUI 在 `app_server_events.rs` 或 `chatwidget.rs` 中处理 `turn/fused` 通知，展示类似 `"检测到模型连续重复调用同一工具 8 次，turn 已中断。如需继续，请明确说明下一步。"` 的提示。
- [ ] 运行测试 PASS；`cargo nextest run -p ody-core` 全绿；`cargo check --workspace --all-targets` 绿灯。
- [ ] Commit。

**验证要点:**
1. **只熔断严格连续重复：** 中间夹任何不同调用（不同工具名或不同规范化参数）即重置 streak，不误伤正常轮询/重试。
2. **参数规范化后判重：** 复用 M2.1 的递归 key 排序序列化，确保 `{command: ": "}` 与 key 顺序无关的等价写法被正确合并。
3. **错误信息可诊断：** 输出包含工具名、重复次数、参数摘要（truncated），方便用户和调试人员理解为何被中断。
4. **token 止损：** 熔断后不再向模型发起新的采样请求，不再生成新的工具调用，输出 token 消耗立即停止。
5. **用户可恢复：** TUI 不退出进程，仅结束当前 turn；用户可继续在同一 session 中发送新指令。
6. **默认启用但可关闭：** 默认阈值 8 或 10；设置为 0 时关闭硬熔断，保持与 M1-M3 软提醒行为一致（用于对齐 TS 或降级排查）。
7. **与 M1-M3 不冲突：** 软提醒（3/5/8）继续注入；fuse 阈值应高于 8（建议 8 是 reminder2 阈值，fuse 建议 10），让模型先收到两次强化提醒，仍无效才熔断。

**风险:**
1. **与 TS 行为不一致：** TS 版 `ToolCallDeduplicator` 只 nudge 不熔断。M4 是 Rust 端新增行为，需在产品文档中说明。
2. **阈值误伤合理重试：** 若用户任务本身需要连续相同调用（如轮询某个状态直到条件满足），过低阈值会中断正常任务。默认 10 且中间不同调用重置，可降低风险。
3. **错误处理路径影响：** `FunctionCallOutput` 的 `is_error: true` 可能影响模型后续行为，需确保 TUI 展示清晰，避免用户误以为工具执行失败而非系统保护。
4. **配置漂移：** 若阈值在不同客户端/版本间不一致，可能导致行为差异。建议在配置 schema 中统一。

---

## 风险与开放问题

1. **死代码漂移（低）:** tool_call_dedup.rs 自 b313cdf（2026-07-12）起未编译，引用类型已逐一核实仍有效，但 `FunctionCallOutputPayload` 等结构字段可能有细微漂移——M1.1 以编译错误为准最小修复。
2. **协议歧义（M3.1 特有）:** 部分 provider 对"一个 call 对应恰好一个 output"敏感；短路后历史中原调用与重复调用各有自己的 output（文本相同、call_id 各自正确），语义等价于"恰好返回了相同结果"，风险可控。
3. **code-mode 工具调用不纳入:** code-mode worker 内 dispatch 的调用（`ToolCallSource` 非 `Direct`）不经 `handle_output_item_done`，streak 检测对其无效。TS 版 loop hook 语义下这些调用**在**检测范围内——此为已知的版本差异，接受并文档化；若后续要对齐需另开 roadmap（在 `handle_tool_call_with_source` 层接线）。
4. **提醒效果依赖模型自觉:** 与 TS 一致，本机制只"nudge"不熔断；若模型无视提醒继续重复，唯一兜底仍是 token 上限压缩。接受（行为对齐优先于新增硬熔断）。
  - **2026-07-23 修正：** 实际 session（`C:\Users\hkb819\.ody-code\sessions\2026\07\23\rollout-2026-07-23T10-29-11-019f8cce-30e5-7270-8d17-1a37eacc546a.jsonl`）显示模型在 reasoning 中已反复意识到"I keep sending no-op commands"，但仍持续 150+ 次 `{"command":": "}` 调用、消耗约 29M cached tokens。软提醒对该类"reasoning 清醒但工具调用生成层瘫痪"的故障无效。因此新增 M4 硬熔断作为兜底，默认启用，阈值可配置（建议默认 8 或 10）。M1-M3 仍保留软提醒以继续对齐 TS 行为。
5. **`1` vs `1.0` 判重分歧:** 见 M2.1 验证要点，接受。

---

## Self-Review

- [x] 每个过大的阶段已拆分：M1/M2/M3/M4 按机制层分离，无子任务捆绑可独立交付的工作（M3 的短路语义、M4 的硬熔断均可独立推迟，不影响 M1+M2 交付价值）。
- [x] M4 作为独立机制层追加，不修改 M1-M3 已批准范围；可单独交付或推迟。
- [x] 每个 `Depends on:` 指向更早的子任务，且有真实 grep/Read 依据（符号级：`append_reminder` 的 self 使用、`run_sampling_request` 唯一调用点 turn.rs:331、`HandleOutputCtx` 构造点、`preserve_order` 特性源 tui/Cargo.toml:91），非标题猜测。
- [x] 并行性已明确说明（无并行：M1.2/M2.1 同文件冲突、M3.1 双重依赖，理由已述）。
- [x] 每个子任务有且仅有一个模式标签，理由基于代码实际要求（签名穿线 + 'static future 约束 → plan；模块声明/纯函数 → normal）。
- [x] Rubric 块位于文档顶部，已经用户批准（含范围决策：全量 M1-M3、不做遥测），后续修订按同一准则执行。
- [x] 一次性批量完成全部标注（单一 rubric 通过后一轮 sweep），未逐阶段重复裁决。
