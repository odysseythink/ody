# Flow script 载体可观测性补齐 实施计划（2026-09-21）

锚定报告：`.ody-code/reports/2026-09-21-game-full-pipeline-research.md`（合并讨论段）+ 2026-09-21 会话内对照评估（候选 A 胜出，用户已确认方向 + 含 guardian 预览增强）。

## 目标

让 `flow.star`（及 `workflow.js`）的 `phase()` 调用产生**结构化阶段进度**，使 script 载体在 TUI `/flows` 获得与 yaml 一致的阶段行显示与完成态推导；guardian run-before 预览从"40 行源码"升级为"阶段标题清单 + 源码预览"。

## 非目标

- 不给 yaml 加 `when:` 条件分支（否决，见评估：侵蚀静态 plan 设计支柱）
- 不加 pause/stop/restart 控制、token 统计、per-agent 钻取（M4 原定 out of scope，仍需协议扩展）
- 不动 `app-server-protocol` / `app-server` / `tui`（复用现有 `FlowPhase` 事件与 TUI 自动完成推导）
- 不改 `/game` 技能本身（它已在每个阶段调用 `phase()`，自动受益）

## 现状锚点（已查证）

- `core/src/flow/star.rs`：`phase()`/`log()` 同走 `BridgeRequest::Log`（:300），泵循环在 :377–:410 转 `FlowProgress::Log`；run 结束在泵循环退出、drain in-flight 之后
- `core/src/flow/v8.rs:88,111`：phase/log 同样合并映射到 `FlowProgress::Log`
- `core/src/flow/host.rs:295+`：`SessionFlowAgentHost::report_progress` 已能把 `PhaseBegin/PhaseEnd` 映射为 `FlowPhaseBeginEvent/FlowPhaseEndEvent`——**无需改动**
- `core/src/flow/plan_summary.rs`：`for_script` 只产 40 行 `source_preview`，`phases` 为空；`PhaseSummary { id, steps }`
- 测试已有 `RecordingHost`（flow_tests.rs:847+）记录 progress 事件，直接复用
- TUI 完成判定：`FlowRuns::status` 在所有 phase finished 时自动 "complete"——**无需改动**

## 语义设计（写进 `core/src/flow/mod.rs` single source of truth）

- `phase(title)`：**阶段边界**。每次调用关闭上一个打开阶段并开启新阶段；run 正常结束时关闭最后阶段（completed==total），run 失败/中断时以 completed<total 关闭（与 yaml 失败路径语义一致，TUI 停留非完成态）
- script 阶段无静态 step 概念：`total_steps = 1`，成功关闭 `completed_steps = 1`，失败 `0`
- `log(msg)`：保持自由文本追加行（`FlowLogEvent`），不变
- `phase_id` 取 title 原文；重复 title 时关闭并重开同名阶段（TUI 按 id 更新同一行，可接受，写入文档）
- 三载体语义一致性：star/v8 行为相同；conformance 测试断言的 prompt/outputs 不受影响（progress 事件不参与 outputs）

## 任务拆分

### T01 star 结构化 phase 事件

- `core/src/flow/star.rs`：`BridgeRequest` 增加 `Phase { title: String }` 变体；`flow_builtins::phase` 改发 `Phase`，`log` 保持 `Log`
- 泵循环维护 `open_phase: Option<String>`：收 `Phase` → 有打开则 `PhaseEnd(completed=1,total=1)` 后 `PhaseBegin{phase_id: title, total_steps: 1}`；收 `Log` 行为不变
- run 收尾（泵退出、drain 完成、eval join 之后）：成功 → 关闭打开阶段（1/1）；失败（Err 路径）→ 以 (0/1) 关闭。注意所有 `?` 提前返回点都要覆盖（建议收尾逻辑集中在一个闭包/守卫里）
- `mod.rs` 语义文档更新（上文"语义设计"段）
- 验收测试（flow_tests.rs，复用 RecordingHost）：
  1. `phase("one") → agent → phase("two") → agent` 的事件序为 `PhaseBegin(one) → PhaseEnd(one,1/1) → PhaseBegin(two) → PhaseEnd(two,1/1)`
  2. `log()` 仍只产 `FlowProgress::Log`
  3. agent 失败中断：最后一个阶段以 (0/1) 关闭，且后续 phase 不再开启
  4. 无 phase 调用的脚本：零 PhaseBegin/End（行为回退兼容）

### T02 v8 对齐（`flow-v8` feature）

- `core/src/flow/v8.rs:88,111`：phase/log 分离，同 T01 语义（v8 的 `phase()` 全局函数与 `log()` 目前合并上报，需在调用点区分）
- 本地不强制编译 V8（prebuilt 下载大）；保证方式：`cargo check -p ody-core --no-default-features --features flow-starlark` 确认默认/无 v8 构建干净 + CI 的 flow-v8 shard 兜底
- 验收：现有 `conformance_v8_matches_yaml_and_starlark_calls_and_outputs` 在 CI 通过（本地可选：`cargo nextest run -p ody-core --features flow-v8 -E 'test(flow_tests::)'`，由用户决定是否本地跑）

### T03 guardian 预览 phase 字面量提取

- `core/src/flow/plan_summary.rs::for_script`：逐行扫描 `phase("...")` / `phase('...')` 字面量（手写扫描器，不引 regex；f-string/变量插值的调用点跳过——只取静态可知的）
- 产出：`phases` 填 `PhaseSummary { id: 字面量, steps: vec![] }`（按出现顺序，去重保留首次）；`source_preview` **保留**（40 行），审批界面两者皆有
- star/v8 共用同一提取器（`phase("x")` 语法相同）
- 验收测试：含 3 个 phase 字面量的脚本 → summary.phases 顺序一致；无字面量 → phases 空、preview 仍在；`/game` 的 flow.star → 能提取出 triage/A1..A5/B1..B3/C 全部标题

### T04 全量验证 + 报告回写

- `cargo nextest run -p ody-core -E 'test(flow_tests::)'`（默认 features，含 starlark）
- `cargo check -p ody-core --no-default-features --features flow-starlark`
- `cargo nextest run -p ody-skills`（确认嵌入技能不受影响）
- `cargo check -p ody-core --all-targets`（共享类型改动纪律）
- 结论回写本计划文件（按用户惯例：核对结论写回原文档）

## 风险与对策

| 风险 | 对策 |
|---|---|
| run 失败的 `?` 早退点漏关阶段 | 收尾关闭逻辑集中在单点（守卫闭包），测试 3 覆盖 |
| v8 本地无法编译验证 | 改动镜像 T01 结构、最小化；CI flow-v8 shard 兜底；计划里显式标注 |
| 重复 phase title 在 TUI 合行 | 文档化；`/game` 的 title 全唯一，无实际影响 |
| guardian 预览对动态 phase（f-string）漏提 | 只承诺静态字面量；动态调用点在 preview 里仍可见源码 |

## 工作量估计

约 250–400 行含测试，全部在 `core/src/flow/` + `core/src/flow/flow_tests.rs`；单 crate（ody-core），预计一次会话内完成。

## 执行结论（2026-09-21，全部完成）

| 任务 | 状态 | 说明 |
|---|---|---|
| T01 star 结构化 phase | ✅ | `BridgeRequest::Phase` 变体 + 泵循环 open_phase 状态机 + `close_open_phase` 收尾（成功 1/1 / 失败 0/1）；`mod.rs` 语义文档更新；4 条验收测试全过 |
| T02 v8 对齐 | ✅ | 实际方案微调：code-mode 引擎侧加 `WorkflowRequest::Phase` + `WorkflowHost::report_phase`（薄传输，默认空实现），open/close 状态机放在 core 的 `FlowWorkflowHost`（与 star 语义单点一致）；v8.rs run 收尾 1/1 / 0/1；**本地 rusty_v8 缓存可用，已实际编译并跑通 v8 测试**（非仅 CI 兜底）；更新 1 条旧断言（`js_parallel_and_phase_reports_progress` 的 phase 从 Log 升级为结构化事件） |
| T03 guardian 预览 | ✅ | `extract_phase_literals` 手写扫描器（标识符边界 + `.` 成员访问排除 + 转义处理 + 去重保序）；`for_script` 产出 `PhaseSummary{id, steps:[]}` 清单 + 保留 40 行 preview；新增 4 条测试含 `/game` flow.star 全部 12 个标题提取；更新 1 条旧断言（`for_plan_marks_starlark_carrier`） |

验证结果（全部通过）：
- `cargo nextest run -p ody-core -E 'test(flow_tests::)'` → 84/84（默认 features）
- `cargo nextest run -p ody-core --features flow-v8 -E 'test(flow_tests::)'` → 94/94（含 conformance 与 js 模块）
- `cargo check -p ody-core --no-default-features --features flow-starlark` → 干净
- `cargo check -p ody-core --features flow-v8` → 干净（本地 v8 缓存）
- `cargo nextest run -p ody-skills` → 15/15
- `cargo check -p ody-core --all-targets` → 干净
- `cargo nextest run -p ody-core -E 'test(flow::plan_summary)'` → 9/9

改动文件：`core/src/flow/star.rs`、`core/src/flow/v8.rs`、`core/src/flow/mod.rs`、`core/src/flow/plan_summary.rs`、`core/src/flow/flow_tests.rs`、`code-mode/src/runtime/workflow.rs`。

影响面：`/game`（flow.star）与所有 script 载体 flow 的 TUI `/flows` 现在显示结构化阶段行并自动推导完成态；guardian run-before 审批 JSON 的 `phases` 字段从空数组变为阶段标题清单（`sourcePreview` 保留）。协议/TUI/guardian 代码零改动（复用现有序列化）。
