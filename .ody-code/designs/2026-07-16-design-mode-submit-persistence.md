# Design 模式 submit 持久化闭环修复 —— 详细设计

- **Date:** 2026-07-16
- **Audit level:** Deep（用户选定）[C:USER]
- **问题报告:** `.ody-code/reports/2026-07-15-design-mode-write-gate-broken.md`（方案 A 的详细化）[C:USER]
- **上游参照:** `D:\workspace\go_work\ody-code`（TS 版 ody-code，已实地取证）

## Scope

### Scope In [C:USER]

1. 新增 `SubmitDesignHandler`（工具名 `submit_design`，参数 `design: string`），仅在 Design 模式注册。 [C:USER]
2. 将 `submit_plan.rs` 的 `handle_call` 主体抽取为模式参数化的共享核心 `handle_submit_artifact`，Plan/Design 两个薄壳 handler 调用。 [C:INFERRED]
3. Design 终态提交终止回合：`core/src/session/turn.rs:373` 终止门由 `mode == Plan` 放宽为 `Plan | Design`。 [C:USER]
4. submit 时 C1–C8 机械校验（复用 `design_completeness_report`，仅终态候选调用；不完整 → 已落盘但非终态 + 缺失清单，修复重试）。 [C:USER]
5. 文案按模式参数化：工具 spec 描述、split 中间提交消息、终态消息。 [C:INFERRED]
6. `collaboration-mode-templates/templates/design.md` 改写为 submit 合同（删除 :82 虚假"已分配路径"措辞与 :155 自造文件名指示）。 [C:USER]
7. `DESIGN_MODE_WRITE_DENIED_REASON` 拒绝文案增强，指明正确合同（index 走 `submit_design`，parts 写 `<stem>/`）。 [C:INFERRED]
8. 测试 T1–T10（见 Test Plan）。 [C:INFERRED]

### Scope Out（显式推迟）

- 上游式"模型直写 + 宿主惰性改写路径"——即报告方案 B，已否决。 [C:USER]
- 定名去重（同日同 slug 两会话互相覆盖，Plan 同病的既有问题）——记入 R2，后续可移植上游 `findUniqueStemInDir`。 [C:DEFERRED]
- `TurnItem::Design` / `EventMsg::DesignDelta` 新事件类型——复用 Plan 事件通道（见假设 #2）。 [C:DEFERRED]
- `plan_artifact.rs:104` `#[allow(dead_code)]` 陈词清理等无关重构。 [C:DEFERRED]
- `split_threshold_gap` 对 Design 的语义适配——Design 分支直接跳过该检查（`Task N` 计数是 plan 语义）。 [C:INFERRED]

## Prior Art（上游 TS ody-code 取证） [C:UPSTREAM]

取证于 `D:\workspace\go_work\ody-code\packages\agent-core\`，结论：

1. **模型直写、无 submit 工具**：上游设计文件由模型 `Write` 直写；`ExitDesignMode`（`src/tools/builtin/planning/exit-design-mode.ts`）不接收内容参数，只从磁盘读。ody-rs 的 `submit_plan` 本身就是分化，本设计的 `submit_design` 延续 ody-rs 自己的"宿主代写"哲学，不以上游直写为准。
2. **惰性定名、无 tmp 文件**：上游 `Write` 工具首次写入时经 `resolveFilePathFromModelRequest`（`session-mode/index.ts:554-598`）提取 basename slug + 日期前缀 + `findUniqueStemInDir` 去重，写 WAL；每个 per-turn reminder 重新声明已分配路径。ody-rs 采用 tmp 路径 + `write_plan` 定名，机制不同但产物格式一致（`YYYY-MM-DD-<slug>.md`）。
3. **完整性门禁在"退出请求时、批准之前"**：`findMissingDesignSections`（C1–C8 正则 + 300 字符 + 3 节下限）在 `resolveExecution` 内、approval surface 之前运行；不完整 → `isError` 工具结果、回合继续、模型修复后重试（"fix and call again"）。**本设计的 A5（submit 时校验 + 落盘但非终态 + 修复重试）与该语义同构。** [C:UPSTREAM]
4. **上游无 handoff 时门禁**：ody-rs 的 `design_handoff` C1–C8 gate 是 ody-rs 独有（D6）。A5 纳入后，handoff gate 退化为 fail-safe 双保险。
5. **split 完成度判定差异**：上游按 manifest 行状态；ody-rs 按 part 文件落盘核实（`row_is_verified_done`），更严格，维持不变。
6. `design_completeness.rs` 已确认是 `findMissingDesignSections` 的忠实移植（正则逐项一致），A5 零新校验逻辑。

## Architecture

### 修复前（三重断裂，引自报告 §4，已逐项代码复核）

- artifact 路径永远停在 `tmp-<thread>-<date>.md`（`turn_context.rs:870-871` → `new_design`，定名唯一入口 `write_plan` 在 Design 下不可达）；[C:UPSTREAM(报告)]
- 模板 `design.md:82` 声称"宿主已分配路径"但从未注入；[C:UPSTREAM(报告)]
- 模板 `design.md:155` 指示模型自造文件名，必然不匹配白名单 → 写门禁 `Deny`（`safety.rs:102-119` + `plan_artifact.rs:314-339`）。[C:UPSTREAM(报告)]

### 修复后数据流 [C:USER]

```
模型 ──submit_design(design_md)──▶ SubmitDesignHandler（spec_plan.rs 仅 Design 注册）
   │                                │ 解析 SubmitDesignArgs → 共享核心 handle_submit_artifact(Design)
   │                                ├─▶ 模式校验（非 Design → Err）
   │                                ├─▶ 防丢守卫（done_count>0 且无 manifest → Err，未持久化）
   │                                ├─▶ TurnItem::Plan(started/completed) + PlanDelta 事件（复用通道）
   │                                ├─▶ PlanArtifact.write_plan ─▶ title-slug 定名 + write_atomically
   │                                │      → .ody-code/designs/YYYY-MM-DD-<slug>.md
   │                                │      → last_plan_text = Some(markdown)
   │                                ├─▶ 有 pending parts → 返回 stem_dir 绝对路径（非终态）
   │                                ├─▶ 终态候选 → design_completeness_report(markdown)
   │                                │      不完整 → 已落盘 + 缺失清单（非终态，修复重试）
   │                                └─▶ 完整 → mark_submitted() + "Design submitted"
   ▼
turn.rs:373 终止门（Plan|Design）─▶ take_submitted() → 回合干净结束
回合后 hook（turn.rs:2552-2573，is_read_only_session_mode 含 Design）
   └─▶ run_session_mode_after_turn → after_plan_turn 解析 ## Parts manifest
        → render_directive Design 分支（plan_mode_injector.rs:244-254，本就就位）
        → design_mode_directive fragment 注入下回合（turn.rs:1276-1294，含精确 part 绝对路径）
模型 ──普通写工具──▶ <stem>/<part>.md（写门禁 stem 分支放行：plan_artifact.rs:322-338）
/plan 切换 ─▶ evaluate_design_exit（design_handoff.rs:33-77）读到真实文件
   └─▶ C1–C8 通过 → Allow{reminder}（handoff reminder 注入 Plan 模式）
```

### 关键既有设施复核结论（Deep 审计逐项验证于 HEAD） [C:INFERRED]

- `turn.rs:2552` after-turn hook 门控为 `is_read_only_session_mode`（含 Design，`safety.rs:79-81`）——Design 下本就触发，只差 `last_plan_text` 为 `None`。
- `turn.rs:1276-1294` directive 注入以真实 `mode` 传参并打 `design_mode_directive` 标签——无需改动。
- `turn.rs:373-381` 终止检查 `take_submitted()` 仅限 Plan——**报告遗漏，必须放宽**（Scope In #3）。
- `submit_plan.rs:270-290` 的 `split_threshold_gap`/`rigor_structure_gap` 为 Plan 语义——Design 分支跳过（rigor 本就以 `plan_mode_tier()` 为门，Design 恒 `None`）。
- 无 `TurnItem::Design`/`EventMsg::DesignDelta` 事件类型存在（全仓 grep 为零）。

## Components & Interfaces

### ① 共享核心 —— 新文件 `core/src/tools/handlers/submit_artifact.rs` [C:INFERRED]

从 `submit_plan.rs:124-330` 纯移动式抽取，逻辑逐行不变，仅按 `expected_mode`/`wording` 参数化。

```rust
/// 模式相关文案参数；两个 handler 各持一份 `static` 实例。
pub(crate) struct SubmitWording {
    pub tool_name: &'static str,   // "submit_plan" | "submit_design"
    pub mode_name: &'static str,   // "Plan" | "Design"
    pub noun: &'static str,        // "plan" | "design"（消息与 item_id 后缀）
    pub out_dir: &'static str,     // ".ody-code/plans/" | ".ody-code/designs/"
}

/// 两模式共用的提交核心。
/// 契约：模式校验 → 防丢守卫 → 发事件 → 宿主代写落盘 →
/// split/完整性门禁 → 终态时 mark_submitted 并返回终态消息。
pub(crate) async fn handle_submit_artifact(
    invocation: ToolInvocation,
    expected_mode: ModeKind,
    wording: &SubmitWording,
    markdown: String,
) -> Result<Box<dyn ToolOutput>, FunctionCallError>
```

### ② 薄壳 handler [C:USER]

- `core/src/tools/handlers/submit_plan.rs`（改造）：`SubmitPlanHandler` 解析 `SubmitPlanArgs{ plan: String }` → `handle_submit_artifact(invocation, ModeKind::Plan, &PLAN_WORDING, args.plan)`。`PLAN_WORDING` 复刻现有全部文案常量（`PLAN_SUBMITTED_MESSAGE` 等），**Plan 行为逐字节不变**。
- `core/src/tools/handlers/submit_design.rs`（新增）：

```rust
pub struct SubmitDesignHandler;

#[derive(Deserialize)]
struct SubmitDesignArgs { design: String }

impl ToolExecutor<ToolInvocation> for SubmitDesignHandler {
    fn tool_name(&self) -> ToolName;              // SUBMIT_DESIGN_TOOL_NAME
    fn spec(&self) -> ToolSpec;                   // create_submit_design_tool()
    fn handle(&self, invocation: ToolInvocation) -> ToolExecutorFuture<'_>;
    // handle → parse args → handle_submit_artifact(inv, ModeKind::Design, &DESIGN_WORDING, args.design)
}
```

### ③ 工具 spec —— 新文件 `core/src/tools/handlers/submit_design_spec.rs` [C:INFERRED]

```rust
pub const SUBMIT_DESIGN_TOOL_NAME: &str = "submit_design";
pub fn create_submit_design_tool() -> ToolSpec
```

- 参数字段 `design: string`（描述镜像 plan 版："pass the index markdown on every call — the turn only ends once no row is `pending`"）。
- 描述写 `.ody-code/designs/`、"Design mode"、终态语义与 split checkpoint 语义，与 `submit_plan_spec.rs:18-23` 逐句对仗。

### ④ 模板改写 —— `collaboration-mode-templates/templates/design.md` [C:USER]

| 位置 | 现状（断裂） | 改为 |
|---|---|---|
| L13 mode rules | "The only file you may write is the current design file assigned to you by the host" | index 由 `submit_design` 持久化（宿主代写）；split parts 可直写 `<stem>/`；其余路径被门禁拒绝 |
| Step 4（L80-82） | "The host has assigned the exact design file path for this session. Write to that exact path" | 镜像 `plan.md:136/184`："Only `submit_design` persists the design file"；**Persistence is automatic** —— 宿主从 `# Title` 派生 slug 定名并原子写盘，无需 shell 或写工具 |
| split 段（L98-108） | 未提 submit | index 每次 checkpoint 经 `submit_design` 提交；parts 用普通写工具写到 submit 返回值 / `design_mode_directive` 给出的 `stem_dir` |
| L153-155 "Design file location" | 让模型自造 `YYYY-MM-DD-<topic>.md` | 改为**描述宿主行为**（镜像 `plan.md:182`）：定名规则 + 落盘目录 `.ody-code/designs/` |
| Step 5 / Turn discipline（L134-151） | 批准后"Design Mode closes" | 批准后以 `submit_design` 为回合唯一动作；C1–C8 在 submit 时机械校验，不完整 → 修复重交；`"Design submitted"` 后回合终止，建议用户 `/plan` |

### ⑤ 拒绝文案 —— `core/src/safety.rs:60` [C:INFERRED]

`DESIGN_MODE_WRITE_DENIED_REASON` 改为（保留 `[design-mode-blocked]` marker 与 `design_mode_write_denied_message` 的 path 后缀格式不变）：

```
Design mode is read-only. Persist the design index with the submit_design tool; write split parts only as .md files under the design's <stem>/ directory. Switch to Plan or Default mode to make other changes. [design-mode-blocked]
```

## Data Models [C:INFERRED]

**无新增持久化数据结构。** 全部状态复用 `PlanArtifact` 既有状态机（`plan_artifact.rs`）：

- `PlanArtifactState::{Temporary{temp_path} → Finalized{final_path} | InlineOnly}`：Design artifact 由 `new_design`（根目录 `designs/` 子目录）创建为 `Temporary`；`write_plan` 首次落盘时经 `slug_from_markdown_title` → `apply_finalized_name` 定名 `YYYY-MM-DD-<slug>.md` —— 对 Design 天然适用，零改动。
- `last_plan_text: Mutex<Option<String>>`：`write_plan` 首行即缓存（`plan_artifact.rs:200-202`），Design submit 后非 `None` → 激活 `turn.rs:2552` after-turn hook。
- `submitted: AtomicBool`：`mark_submitted`/`take_submitted` 语义不变；每回合 artifact 新构造（`turn_context.rs:866-886`），无跨回合残留。
- 新增仅两个编译期静态实例 `PLAN_WORDING`/`DESIGN_WORDING` 与一个反序列化结构 `SubmitDesignArgs`，均无生命周期/持久化语义。

## Algorithms

### A. `handle_submit_artifact`（共享核心主算法） [C:USER]

```
输入: invocation, expected_mode: ModeKind, wording: &SubmitWording, markdown: String
输出: Result<ToolOutput, FunctionCallError>

1.  解构 invocation → (session, turn, call_id, arguments)
2.  if turn.collaboration_mode.mode != expected_mode:
        return Err(RespondToModel("{wording.tool_name} is only available in {wording.mode_name} mode"))
3.  artifact ← turn.plan_artifact
    if artifact is None: return Err(RespondToModel("{tool_name} unavailable: no artifact"))
4.  // 防丢守卫（自 submit_plan.rs:160-181 原样迁移，两模式通用）
    previously_had_done_parts ← artifact.last_manifest_snapshot().is_some_and(done_count > 0)
    if previously_had_done_parts AND parse_parts_manifest(markdown).manifest is None:
        return Err(RespondToModel("...resubmit the full index markdown..."))   // 未持久化
5.  item_id ← "{turn.sub_id}-{wording.noun}"
    emit TurnItem::Plan(started,  id=item_id, plan_file_path=artifact.path())
    emit EventMsg::PlanDelta(thread_id, turn_id, item_id, delta=markdown)
6.  persist ← turn.config.plan_mode.persist_plan_file ∨ true
    outcome ← artifact.write_plan(markdown, persist).await
    if outcome is Failed{error}:
        emit EventMsg::Warning("Failed to persist {wording.noun}: {error}")
7.  // split 状态判定（自 :238-255 原样迁移）
    has_pending_parts ←
        if artifact.stem_dir() = Some(dir):
            manifest 存在 且 任一行 NOT row_is_verified_done(dir, row)
        else:
            manifest 存在 且 任一行 status == Pending
8.  // 终态候选门禁：按模式分派
    gap ← None
    if NOT has_pending_parts:
        if expected_mode == Plan:
            gap ← 现有逻辑原样（:270-290：无 manifest 时 split_threshold_gap，
                  否则 tier==Rigor 时 rigor_structure_gap）
        else:  // Design
            gap ← design_completeness_report(markdown)   // Option<String>；Some=缺失清单
9.  emit TurnItem::Plan(completed, id=item_id, text=markdown, plan_file_path)
10. if has_pending_parts:
        return Ok("{Noun} part saved. … stay in {mode_name} mode …
                   write each part file at exactly {stem_dir}/<part-name>.md …")
    // stem_dir 为 None 时退化为不含路径的版本（自 :310-316 原样迁移）
11. if gap = Some(g):
        return Ok("{Noun} saved, but it is {g}. This call was persisted but is NOT final —
                   stay in {mode_name} mode, add the missing section(s), and call {tool_name} again.")
12. artifact.mark_submitted()
    return Ok("{Noun} submitted")
```

**Design 分支与 Plan 的唯一逻辑差**：第 8 步——Design 跳过 `split_threshold_gap`（`Task N` 计数是 plan 语义，设计文档无此约定）与 `rigor_structure_gap`（Design 无 tier，本就恒 `None`），替换为 `design_completeness_report`。其余步骤两模式逐行共享。 [C:USER（A5）+ C:INFERRED（跳过 plan 检查）]

### B. 注册（`core/src/tools/spec_plan.rs:686-690`） [C:USER]

```
// 现状：if mode == ModeKind::Plan { add_with_exposure(SubmitPlanHandler, DirectModelOnly) }
match turn_context.collaboration_mode.mode {
    ModeKind::Plan   => planned_tools.add_with_exposure(SubmitPlanHandler,   ToolExposure::DirectModelOnly),
    ModeKind::Design => planned_tools.add_with_exposure(SubmitDesignHandler, ToolExposure::DirectModelOnly),
    _ => {}
}
// 注释更新：submit_plan / submit_design 分别是 Plan / Design 模式的显式终态动作。
```

### C. 回合终止门（`core/src/session/turn.rs:373-381`） [C:USER]

```
// 现状：if turn_context.collaboration_mode.mode == ModeKind::Plan && ...take_submitted()
if matches!(turn_context.collaboration_mode.mode, ModeKind::Plan | ModeKind::Design)
    && turn_context.plan_artifact.as_ref().is_some_and(|a| a.take_submitted()):
        last_agent_message = sampling_request_last_agent_message
        break   // 与 Plan 相同：跳过 stop hooks，submit 是有意的终态动作
// 注释更新：submit_plan / submit_design 为两模式各自的终态工具。
```

### D. C1–C8 校验接入点 [C:USER]

`design_completeness_report(&str) -> Option<String>`（`core/src/design_completeness.rs`，`design_handoff.rs:9` 已 import 同一函数）：
- 输入：submit 的 `markdown` 全文（split 时即 index——与 handoff gate 读盘所得内容一致，语义对齐）；
- 输出：`None` = 完整；`Some(缺失清单)` = 不完整 → 走主算法第 11 步（已落盘、非终态、修复重试）；
- split 中间提交（`has_pending_parts == true`）不校验——与 `rigor_gap` 的"仅终态候选"模式一致（`submit_plan.rs:257-262` 注释自述的理由同样成立）。

## Call-site Integration

| # | 文件：行 | 改动 | 周边上下文 |
|---|---|---|---|
| S1 | `core/src/tools/handlers/submit_artifact.rs`（新） | 共享核心 + `SubmitWording` + 两个静态实例 | 自 submit_plan.rs 迁入 `parse_parts_manifest`、`row_is_verified_done`、`split_threshold_gap`、`rigor_structure_gap`、`count_task_headings` 等私有辅助（随核心整体移动） |
| S2 | `core/src/tools/handlers/submit_plan.rs:105-331` | 瘦身为薄壳；保留 `SubmitPlanArgs`、测试模块 | :333-488 的现有测试继续编译通过即证明抽取等价（T10） |
| S3 | `core/src/tools/handlers/submit_design.rs`（新） | `SubmitDesignHandler` 薄壳 + `SubmitDesignArgs` | 结构镜像 submit_plan.rs |
| S4 | `core/src/tools/handlers/submit_design_spec.rs`（新） | `create_submit_design_tool` | 结构镜像 submit_plan_spec.rs:8-34 |
| S5 | `core/src/tools/handlers/mod.rs`（或等价模块清单） | 注册两个新模块 | 按现有 handlers 模块声明方式追加 |
| S6 | `core/src/tools/spec_plan.rs:686-690` | `if` → `match`（算法 B） | 前后注释同步更新 |
| S7 | `core/src/session/turn.rs:368-381` | 终止门放宽（算法 C） | 上方注释（:368-372）提及 submit_design |
| S8 | `core/src/safety.rs:60` | `DESIGN_MODE_WRITE_DENIED_REASON` 文案 | `design_mode_write_denied_message`（:70-75）不动；TUI 依赖的 marker 不动 |
| S9 | `collaboration-mode-templates/templates/design.md` L13 / L80-82 / L98-108 / L134-155 | submit 合同改写（见 Components ④） | `{{ split_threshold }}` 占位符渲染（`collaboration_mode_instructions.rs:169-189`）不受影响 |

## Error Handling（错误与降级） [C:INFERRED]

| 错误类 | 立即处理 | 降级路径 | 恢复条件 |
|---|---|---|---|
| 非 Design 模式调 `submit_design` | `Err(RespondToModel)`："only available in Design mode" | 无状态变化、未持久化 | 模式正确后重试 |
| 非 Plan 模式调 `submit_plan` | 同上（现状不变） | 同上 | 同上 |
| 无 artifact | `Err`："no artifact" | 不会发生：`turn_context.rs:856-887` 保证只读模式必有 artifact | — |
| 磁盘写失败 | `write_plan` → `InlineOnly` + `EventMsg::Warning` | 全文仍在 TurnItem(completed) 输出；`path()` 变 `None` → handoff gate fail-safe 视为不完整（与 Plan 现状一致） | 修复文件系统后重交 |
| 裸 index 覆盖已有 done parts | `Err` 拒绝，**未持久化** | 重交含 manifest 的完整 index | — |
| C1–C8 不完整（终态候选） | **已落盘**但非终态 + 返回缺失清单 | 模型补全后重交；消息显式引导，非死锁 | 校验通过 |
| split 中间提交（有 pending 行） | 非终态 + 返回 `stem_dir` 绝对路径 | 继续逐 part 写盘 | 无 pending 行 |
| 定名冲突（同日同 slug 两会话） | 后者原子覆盖前者（**既有行为**，Plan 同病） | 本设计不修（R2） | 后续独立任务 |
| `persist_plan_file=false` + Design | `InlineOnly`：`path()`=None → directive 无具体 stem 路径、handoff fail-safe Veto | 与 Plan 模式该配置下的行为一致 | 开启 persist |

## Test Plan（断言级） [C:INFERRED]

| # | 位置 | 测试 | 关键断言 |
|---|---|---|---|
| T1 | `spec_plan.rs` 测试 | 注册按模式分派 | Design 模式工具列表含 `submit_design` 且**不含** `submit_plan`；Plan 反之；Default 皆无 |
| T2 | `submit_design.rs` 测试 | 模式校验双向 | Plan 中调 `submit_design` → Err 含 `"only available in Design mode"`；Design 中调 `submit_plan` → Err 含 `"only available in Plan mode"` |
| T3 | 同上 | Design 终态落盘 | 文件存在于 `.ody-code/designs/YYYY-MM-DD-<slug>.md`、内容与提交一致；返回 `"Design submitted"`；`artifact.take_submitted() == true`；item id 以 `-design` 结尾 |
| T4 | 同上 | C1–C8 拒绝（must-reject） | 提交缺 C8 的设计 → 返回含缺失清单 + `"NOT final"`；**文件已落盘**（内容=提交）；`take_submitted() == false` |
| T5 | 同上 | split 中间提交 | 含 pending 行的 index → 返回含 `stem_dir` 绝对路径（`designs/YYYY-MM-DD-<slug>`）且**不含** `"Plan mode"` 字样；`take_submitted() == false`；C1–C8 校验**未**执行（缺 C8 也不报） |
| T6 | 同上 | done-parts 防丢 | `last_manifest_snapshot.done_count > 0` 且提交无 manifest → Err；磁盘文件内容**未被改写** |
| T7 | `safety_tests.rs` / `apply_patch_tests.rs` | 写门禁 | Design 模式写 `<finalized-stem>/part.md` → `Allow`；写他处 → `Deny` 且消息含 `submit_design` 与 `[design-mode-blocked]` |
| T8 | `turn.rs` 测试 | 终止门 | Design 模式 `mark_submitted` 后回合终止；Default 模式不受影响 |
| T9 | 端到端（core/tests/suite） | 闭环 | submit → `last_plan_text` 非空 → after-turn 注入 `design_mode_directive`（Design 文案、含解析后 part 绝对路径）→ part 直写放行 → `evaluate_design_exit` → `Allow{reminder: Some(_)}` |
| T10 | 现有套件回归 | 抽取等价 | `submit_plan.rs`、`plan_mode_injector.rs`、`design_handoff.rs`、`safety.rs` 现有测试**零修改全绿** |

**Done criteria**：`cargo nextest run -p ody-core` 全绿；`cargo test -p ody-core --tests` 全绿；`collaboration-mode-templates` 有相关 snapshot/渲染测试则同步更新并通过。

## Risk Register

| # | 风险 | 可能性 | 影响 | 缓解 |
|---|---|---|---|---|
| R1 | 共享核心抽取引入 Plan 回归 | 中 | 高 | 纯移动式抽取（逻辑逐行不变，仅参数化文案）；T10 全量回归；`PLAN_WORDING` 复刻现有常量 |
| R2 | 定名冲突：同日同 slug 两会话互相覆盖（**既有问题**，Plan 同病） | 低 | 中 | 本设计不修，记录在案；后续可移植上游 `findUniqueStemInDir`（`session-mode/index.ts`） |
| R3 | split index 不满足 C1–C8 被误拒（**实证确认**：裸 index 模板仅过 C1–C3） | 中 | 低 | 模板 split 合同增加"index 必须自带 C1–C8 全局摘要"条款（S9）；拒绝消息列缺失清单引导补全，非死锁；与 handoff gate 判定内容一致，行为可预期 |
| R4 | 模型仍尝试直写 index | 低 | 低 | 拒绝文案指明 `submit_design`（S8）；模板 Step 4 合同显式化 |
| R5 | UI 将设计显示为 "plan" 样式项（复用事件通道） | 低 | 低 | 接受；后续可加 `TurnItem::Design`（Scope Out） |
| R6 | 两份 spec/wording 长期漂移 | 低 | 低 | 文案集中于两个 `static SubmitWording` 实例；共享核心单一实现 |

## Assumptions & Unverified Items

| # | Assumption | Confidence | Impact if wrong | How to verify |
|---|---|---|---|---|
| 1 | 共享核心抽取到新模块 `submit_artifact.rs`（文件组织选择，非行为决策） | High | reviewer 偏好其他布局；零行为影响 | `cargo build` + T10 |
| 2 | Design 提交复用 `TurnItem::Plan`/`PlanDelta` 事件通道，不新增 Design 事件类型；item id 后缀按 `{noun}` 区分 | High | UI 将设计显示为 plan 样式项（R5） | T3 断言 item id；UI 走查 |
| 3 | `submit_design` 参数字段名为 `design`（不复用 `plan`） | High | 模型按 spec 传参；spec 描述已写明字段 | T2/T3 |
| 4 | `DESIGN_MODE_WRITE_DENIED_REASON` 文案增强且保留 `[design-mode-blocked]` marker 不变 | Medium | TUI 等消费者依赖 marker 而非全文 | grep marker 消费者 + T7 |
| 5 | Design 分支跳过 `split_threshold_gap` 与 `rigor_structure_gap` | High | 设计文档无 `Task N` 标题约定、Design 无 tier；跳过后两检查对 Design 恒为空操作 | 正则实证：`^#{2,4}\s+task\s+\d` 需数字后缀 |
| 6 | C1–C8 校验针对提交全文（split 时=index），与 handoff gate 读盘内容一致 | High | split index 可能误拒（实证确认）→ S9 模板条款 + R3 缓解 | V-B 实证 + T5/T9 |
| 7 | spec 放新文件 `submit_design_spec.rs`（而非参数化现有 `submit_plan_spec.rs`） | Medium | 纯布局 | `cargo build` |
| 8 | 模板仅改合同相关段落（L13 / L80-82 / L98-108 / L134-155 + split index 自给条款），其余不动 | Medium | 改多引入漂移 | diff review |
| 9 | 定名沿用 `apply_finalized_name` 现状、不引入去重 | High | 同日同 slug 覆盖（既有，R2） | 记录，后续任务 |

## Self-Review [C:INFERRED]

**最贵决策深审（3 项，各 3 输入）：**

1. **C1–C8 门禁对 split index 的判定** —— 实证（`node -e` 跑 `design_completeness.rs` 真实正则）：① 裸 index（模板 split 段描述的标准内容）→ 仅 C1–C3 PASS，**C4–C8 FAIL**（发现模板 split 段与 Step 5 C1–C8 清单的既有矛盾）；② 含 C1–C8 全局摘要节的 index → 全 PASS；③ 单文件完整设计 → 全 PASS。**结论：非本设计引入的缺陷，但 A5 使其提前暴露；已以 S9 模板"index 自给 C1–C8 摘要"条款 + R3 缓解。**
2. **写门禁 stem 分支对 `designs/` 的适用性** —— Read 验证 `plan_artifact.rs:314-339`：① finalized design 路径 → 主文件匹配 ✓；② `<stem>/core.md` → stem 分支匹配 ✓（注释自述 "Applies to both plan and design artifacts"）；③ `<stem>/x.txt` / 其他父目录 → 拒绝 ✓。
3. **`turn.rs:373` 终止门放宽的意外触发面** —— ① Design 非 submit 路径：`take_submitted()` 仅 `mark_submitted()` 后为 true，而全仓唯一调用点在共享核心终态分支 → 无误触发 ✓；② submit 后用户不切 `/plan` 继续对话：下回合 artifact 新构造（`turn_context.rs:866-886`），flag 不残留 ✓；③ split 中间提交：不调用 `mark_submitted`（`has_pending_parts` 分支先返回）→ 回合继续 ✓。

**四透镜扫描：**
- **Security**：slug 经现有 `sanitize_plan_slug`；无密钥/PII 进路径；提交全文进事件流（与 Plan 相同）；`stem_dir` 泄露面=模型可写区本身。无新增过滤器/正则 → 无假阳/假阴面。
- **Test**：每行为 must-pass + must-reject 成对（T3/T4、T5/T6、T7 Allow/Deny、T2 双向）；T5 含"不得含 Plan mode 字样"防文案串味；无与常量矛盾的断言（T4"落盘但非终态"与主算法步骤 6→8→11 顺序一致：先 `write_plan` 后 gap 判定）。
- **Ops**：submit 成本=一次原子写 + 正则扫描（亚毫秒）；artifact 每回合新构造，无跨回合竞争；重复 submit 幂等（同 slug 覆盖同路径，原子写）；split 场景 directive 注入为既有管线。
- **Integration**：全部依赖已 Read/Grep 验证存在（`design_completeness_report`、`parse_parts_manifest`、`row_is_verified_done`、`write_plan`、`stem_dir`、`is_read_only_session_mode`、`render_directive` Design 分支、`evaluate_design_exit`、`TurnItem::Plan`/`PlanDelta`）；落点即报告指定位置，无重定向。
- **Scope**：单一连贯设计（一个 bug 的修复闭环），无需拆分。

## User Final Approval

- 审计级别：**Deep** [C:USER]
- 决策记录：① 方案 A（submit 工具 + 宿主代写）[C:USER]；② 新增 `SubmitDesignHandler`/`submit_design` [C:USER]；③ 终态提交终止回合 + `turn.rs:373` 放宽 [C:USER]；④ A5 纳入（submit 时 C1–C8，语义对齐上游退出时门禁）[C:USER]
- 分段确认：第 1 段（范围+架构）✓；第 2 段（组件+算法）✓；第 3 段（模板+错误+测试+风险）✓
- 写后审计门禁（Deep 级）：**全部通过** —— 12 个章节关键论断（Scope / Prior Art / Architecture / Components / Data Models / Algorithms / Call-site / Error / Test / Risk / Self-Review / Reuse）逐节确认接受；假设 #1–#9 逐条签出，全部"接受"，无推迟、无更正
- 最终批准：**待 ExitDesignMode**

## Reuse Analysis [C:INFERRED]

| 候选（路径 + 符号） | 复用方式 |
|---|---|
| `core/src/plan_artifact.rs`: `write_plan` / `slug_from_markdown_title` / `apply_finalized_name` / `write_atomically` / `last_plan_text` / `stem_dir` / `is_plan_file_path` | **原样复用**，零改动（定名、原子写、缓存、白名单对 Design 天然适用） |
| `core/src/design_completeness.rs`: `design_completeness_report` | **原样复用**（`design_handoff.rs:9` 已 import 同款）；A5 零新校验逻辑 |
| `core/src/tools/handlers/submit_plan.rs`: `parse_parts_manifest` / `row_is_verified_done` / `split_threshold_gap` / `rigor_structure_gap` / `count_task_headings` / 防丢守卫 / split 消息 / 事件发射 | **随共享核心整体迁移**复用；Plan 薄壳继续调用 |
| `core/src/plan_mode_injector.rs`: `after_plan_turn` / `render_directive` Design 分支（:244-254） | **零改动复用**——本就写好的 Design 分支因 `last_plan_text` 修复而从生产不可达变为可达 |
| `core/src/session/turn.rs`: after-turn hook（:2552）、directive 注入（:1276-1294） | **零改动复用**（门控 `is_read_only_session_mode` 含 Design） |
| `core/src/session/design_handoff.rs`: `evaluate_design_exit` C1–C8 gate | **零改动复用**——文件真实落盘后连带恢复功能（修复报告 §5 次生 Veto） |
| `SubmitPlanHandler` 结构 / `submit_plan_spec.rs` 模式 | **镜像复用**为 Design 薄壳与 spec |
| 新代码 | 仅限：2 个薄壳 + 1 个 spec + 2 个 `static SubmitWording` + 模板文案 + 拒绝文案。**无 greenfield 组件。** |
