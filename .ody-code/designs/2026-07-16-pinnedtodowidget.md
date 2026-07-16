# PinnedTodoWidget 高度锁定与渲染路径收敛设计

日期：2026-07-16
关联报告：`.ody-code/reports/pinned-todo-height-shrink-analysis.md`
审计级别：Deep [C:USER]
选定方案：方案 A — PlanLayout 纯函数 + widget 内水位线 + BottomPane 复用实例 [C:USER]
锁定语义：单调水位线（高度只增不减，封顶 max_lines，换全新计划时不重置）[C:USER]
修复范围：高度锁定 + 顺带重构渲染路径（folded/full 收敛到一处）[C:USER]
推断项签字：7 项 [C:INFERRED] 全部接受 [C:USER]
最终批准：已批准 [C:USER]

## 背景与问题陈述

`PinnedTodoWidget`（底部 Todo 面板）随任务完成高度不断收缩。根因（报告已证）：
1. widget 无持久目标高度状态，`target = full.len().min(max_lines)` 每次现算；
2. `BottomPane::set_pinned_todo` 每次 `PinnedTodoWidget::new()` 重建实例，丢弃任何状态；
3. padding 补到的是"已收缩的 target"而非历史最大高度。

注：收缩仅发生在 `full_height > max_lines` 的长计划 folded 路径（短计划 `render_plan_steps` 输出高度与完成状态无关，恒为 `1 + N + expl`）[C:INFERRED: 经 plans.rs:53-103 源码核实]。

## C1 Scope

### In
- `PinnedTodoWidget` 新增单调水位线状态，folded 高度只增不减、封顶 `max_lines` [C:USER]
- `BottomPane::set_pinned_todo(Some)` 改为复用现有实例（调 `update`），仅在无实例时新建 [C:INFERRED ✓]
- 渲染路径收敛：`full` / `folded` 两条路径收进 `pinned_todo.rs` 内一个私有 `plan_layout` 子模块 [C:USER]
- 同一 widget 实例跨多次 `update_plan` 的锁定测试 [C:INFERRED ✓]

### Out
- expanded (ctrl+e) 行为改动（保持现状：展开显示全部，收起回 folded+水位线；expanded 期间不更新水位线）[C:USER]
- `render_plan_steps`（`history_cell/plans.rs`）对外契约不变 [C:INFERRED ✓]
- BottomPane 其它 widget / 布局逻辑 [C:INFERRED ✓]
- 持久化到磁盘 / 跨 session 恢复锁定高度 [C:INFERRED ✓]
- 换全新计划时重置水位线（经用户确认：不重置，单调语义涵盖）[C:USER]

## C2 Architecture / Design

### 组件与数据流

```
update_plan (model tool call)
  └─ turn_runtime.rs:409 on_plan_update(update)
       └─ turn_runtime.rs:421 bottom_pane.set_pinned_todo(Some(update))
            └─ bottom_pane/mod.rs:417 set_pinned_todo
                 │  Some → match self.pinned_todo
                 │           Some(w) => w.update(args)          // 复用, 保留 watermark
                 │           None    => PinnedTodoWidget::new() + update(args)
                 │  None  → self.pinned_todo = None             // watermark 随实例销毁
                 ▼
            PinnedTodoWidget
            ├─ explanation, plan, max_lines, expanded   (数据)
            ├─ watermark: Option<usize>                  (folded 单调水位线)
            └─ display_lines(width)                      // Renderable 入口
                 │  plan.is_empty → []                   // 隐藏
                 │  expanded      → plan_layout::full(width, expl, plan)
                 ▼  folded
              plan_layout::folded(width, expl, plan, max_lines, &mut watermark)
                 ├─ full_lines  = full(width, expl, plan)      // 复用 render_plan_steps
                 ├─ candidate   = min(full_lines.len(), max_lines)
                 ├─ *watermark  = max(watermark.unwrap_or(0), candidate)
                 ├─ body        = full_lines.len() ≤ max_lines
                 │                 ? full_lines
                 │                 : compact_fold(width, expl, plan, max_lines)
                 └─ pad blank to *watermark

渲染管线（每次 frame）:
  BottomPane::as_renderable (mod.rs:1757)
    └─ flex2.push(Borrowed(pinned))            // desired_height → display_lines(width).len()
         └─ FlexRenderable::desired_height(width) → sum(children.desired_height)
```

### 模块划分

`tui/src/bottom_pane/pinned_todo.rs`（单文件内收敛，不新增顶层文件）：
- `PinnedTodoWidget` — 持有数据 + `watermark`；实现 `Renderable`；对外 API 不变（`new` / `update` / `toggle_expanded` / `to_update_args`）。
- `mod plan_layout`（私有子模块）— 纯函数，不持有状态：
  - `full(width, explanation, plan) -> Vec<Line>` — 直接委托 `render_plan_steps`（复用，见 C8）。
  - `compact_fold(width, explanation, plan, max_lines) -> Vec<Line>` — 现有 `folded_lines` 的实现平移，参数从 `&self` 拆为显式入参。
  - `folded(width, explanation, plan, max_lines, watermark: &mut Option<usize>) -> Vec<Line>` — 唯一知道"水位线"的地方；内部调 `full` / `compact_fold` 并做 padding。

这样 widget 只负责"何时销毁实例"（生命周期事件决定 watermark 何时归零），`plan_layout` 只负责"给定状态如何渲染+更新水位线"。

## C3 Data Models

```rust
// tui/src/bottom_pane/pinned_todo.rs
pub(crate) struct PinnedTodoWidget {
    explanation: Option<String>,
    plan: Vec<PlanItemArg>,
    max_lines: usize,
    expanded: bool,
    /// Folded-mode monotone watermark: the largest folded height (in lines)
    /// this widget instance has ever rendered, capped at `max_lines`.
    /// `None` until the first folded render. Drops with the widget instance
    /// (set_pinned_todo(None) / new thread); never shrinks while alive.
    watermark: Option<usize>,          // [C:USER 单调水位线]
}

impl Default for PinnedTodoWidget {   // watermark: None
    ...
}

// plan_layout 子模块（私有）
mod plan_layout {
    pub(super) fn full(
        width: u16,
        explanation: Option<&str>,
        plan: &[PlanItemArg],
    ) -> Vec<Line<'static>>;

    pub(super) fn compact_fold(
        width: u16,
        explanation: Option<&str>,
        plan: &[PlanItemArg],
        max_lines: usize,
    ) -> Vec<Line<'static>>;

    pub(super) fn folded(
        width: u16,
        explanation: Option<&str>,
        plan: &[PlanItemArg],
        max_lines: usize,
        watermark: &mut Option<usize>,
    ) -> Vec<Line<'static>>;
}
```

字段可见性与 crate 内测试构造保持一致（沿用现有 `pub(crate)` 结构体字面量构造的测试风格）。

## C4 Algorithms

### folded 渲染 + 水位线更新（核心）

```
fn folded(width, expl, plan, max_lines, watermark) -> Vec<Line>:
    full_lines = full(width, expl, plan)
    if full_lines.is_empty():            # plan.is_empty, 由调用方先判
        return []

    candidate  = min(full_lines.len(), max_lines)
    *watermark = Some(max(watermark.unwrap_or(0), candidate))

    body = if full_lines.len() <= max_lines
               then full_lines
               else compact_fold(width, expl, plan, max_lines)

    while body.len() < watermark.unwrap():
        body.push(Line::from(""))
    return body
```

性质（已经 python 内存求值验证，见 C6）：
- 高度单调不降：每次渲染后 `body.len() >= watermark >= 历史任何 candidate`。
- 封顶：`watermark ≤ max_lines`，不会无限涨。
- 宽度变化：宽 → 折行少 → candidate 小，但 watermark 已高，pad 保持；窄 → 折行多 → candidate 大，watermark 跟涨。宽度不回撤，符合"只增不减"（C1 接受的语义）。
- `max_lines = 0` → watermark = 0，输出 0 行，无 panic。

### BottomPane 实例复用

```
fn set_pinned_todo(&mut self, update: Option<UpdatePlanArgs>):
    match update:
        Some(args) => match &mut self.pinned_todo:
            Some(w) => w.update(args),                 // 保留 watermark / expanded
            None    => {
                let mut w = PinnedTodoWidget::new();   // watermark = None
                w.update(args);
                self.pinned_todo = Some(w);
            }
        None => self.pinned_todo = None,               // watermark 随实例销毁
    self.request_redraw()
```

`update(args)` 现有行为（重置 `expanded = false`）保留；**不**重置 watermark（经用户确认，换全新计划也不重置，单调语义涵盖）。

## C5 Error Handling / Degradation

| 场景 | 行为 | 依据 |
|---|---|---|
| 空计划（plan.is_empty） | 返回空行，widget 隐藏（高度 0） | 保持 pinned_todo.rs:97-99 现状 [C:INFERRED ✓] |
| `max_lines = 0` | `watermark = 0`，输出 0 行 | saturating 语义，无 panic（已验证） |
| 宽度 = 0 / 极窄 | `adaptive_wrap_line` 已有 `.max(1)` 兜底；folded 仍至少给 header | plans.rs:59,74 |
| expanded 期间高度 > watermark | expanded 路径不经过 watermark，原样全量返回 | C1 Out，保持现状 [C:USER] |
| expanded → 收起 | 回 folded，watermark 仍是展开前的 folded 水位线（展开期间不更新） | 语义简化 [C:INFERRED ✓] |
| 同一 turn 内连续多次 update_plan | 复用实例，watermark 只增不减 | 本次修复目标 |
| 换 thread / `set_pinned_todo(None)` | 实例销毁，下次 Some 时 watermark 重新从 None 累积 | 新会话不应继承旧面板高度 [C:INFERRED ✓] |
| explanation 变长导致 full 变高 | candidate 变大，watermark 跟涨（封顶 max_lines） | 单调语义自然涵盖 |
| 换全新计划（step 集合剧变） | 不重置 watermark（用户确认），面板可能比新计划需要的高（留白） | [C:USER] |

## C6 Self-Review

### 最昂贵决策审计（1–3 个）

1. **水位线单调不降 vs 首次锁定**（最贵，选错则语义返工）：已验证单调算法在 4 个序列（恒高 / 递减 / max_lines=0 / 先涨）下正确（python 内存求值，见下）。选单调是因为用户明确"达到最大高度后不再缩减"，而首次锁定在"首帧就偏矮"（如首帧只有 2 步）时会永久锁矮。
2. **BottomPane 复用实例 vs widget 自锁**（次贵，选错则状态丢失）：若不修 `set_pinned_todo` 的 `new()` 重建，widget 内任何 watermark 都会被丢弃——这是报告指出的第二层断点，必须一起修。已确认 BottomPane 仅 mod.rs:417 一处新建实例（session_flow.rs:27 为 None 清除），无其它绕过路径。
3. **plan_layout 私有化 vs 提升为公共模块**（便宜但影响面）：选私有子模块，因为 folded 渲染目前只被 PinnedTodoWidget 使用；`render_plan_steps`（full 路径）保持公共以维持与历史 cell 的契约测试。

### 四视角扫描

- **Security**：无新输入面；`update_plan` 参数来自 model tool call，沿用现有解析。`Line::from("")` 为空行，无注入风险。
- **Test/Verification**：现有 `pinned_todo_renders_identically_to_history_cell` 锁定 full 路径契约；新增同实例跨 update 测试覆盖第二层断点（widget 内 watermark）。`renders_pinned_todo_with_explanation_and_steps.snap` 是 full 路径快照，full 路径逻辑不变，该快照不应变；但建议跑 `cargo insta review` 确认（见假设 #5）。
- **Operations**：无配置项 / 无迁移 / 无持久化变更；行为变化仅限 TUI 渲染。
- **Integration**：`set_pinned_todo` 签名不变（`Option<UpdatePlanArgs>`），turn_runtime.rs:421 与 session_flow.rs:27 调用点无需改。`Renderable` 契约（`desired_height` 纯查询）未被破坏——watermark 更新发生在 `display_lines`（被 `desired_height` 与 `render` 共同调用），两者结果一致。

### 纯函数验证记录

watermark 算法经 python 内存求值（不落盘、不改 repo）：
- 恒高序列（full=9, max=8）→ `[8;8]` 单调 ✓
- 递减序列（full 9→3, max=8）→ 锁 8 ✓
- `max_lines=0` → 0，无 panic ✓
- 增长序列（full 3→9, max=8）→ `[3,5,8]` 单调涨且封顶 ✓

### 自审修正记录

- 初稿 C5 曾把"expanded 期间更新 watermark"列为开放项 → 经考量简化为"expanded 不更新 watermark"（路径不经过 folded），已写入 C5 第 5 行与 C1 Out。
- 初稿假设 #5 未确认快照路径 → 已核实 `renders_pinned_todo_with_explanation_and_steps.snap` 为 full 路径（5 行无 padding），full 路径逻辑不变故快照不应变；保留假设但下调风险。

## C7 User Approval

分段确认记录：
- C1 Scope：锁定语义 = 单调水位线 [C:USER]，修复范围 = 高度锁定 + 顺带重构渲染路径 [C:USER]
- 方案选型：方案 A [C:USER]
- C2 架构 + C3 数据模型：正确，继续 [C:USER]
- 换计划是否重置 watermark：不重置 [C:USER]
- 审计门禁：7 项 [C:INFERRED] 全部接受 [C:USER]
- 最终批准：已批准 [C:USER]

## C8 Reuse Analysis

复用清单（已核实符号存在）：
- `crate::history_cell::render_plan_steps`（`tui/src/history_cell/plans.rs:53`）— `plan_layout::full` 直接委托，保证 pinned 与历史 cell 渲染一致（现有测试 `pinned_todo_renders_identically_to_history_cell` 锁定此契约）。
- `crate::render::line_utils::prefix_lines`（`render/line_utils.rs:41`）— `compact_fold` 缩进树形前缀沿用。
- `crate::render::line_utils::push_owned_lines`（`render/line_utils.rs:21`）— 折行行收集沿用。
- `crate::wrapping::adaptive_wrap_line` / `RtOptions`（`wrapping.rs:508`）— explanation / step 折行沿用。
- `crate::style::accent_style` — header "Todo" 强调样式沿用。

不新造：折行、树形前缀、样式、full 渲染。仅把现有 `folded_lines`（pinned_todo.rs:123-239）平移为 `plan_layout::compact_fold` 并参数化。

## Requirements → Test Assertions 映射

| 需求 | 测试断言 |
|---|---|
| 同一实例跨 update 高度不缩 | 新增：同一 widget 连续 `update`（8 步计划，逐步 Completed），每次 `desired_height` 相等 |
| 高度封顶 max_lines | 现有 `folded_mode_respects_max_lines` + 新增：长计划完成后 `desired_height == max_lines` |
| 短计划高度不变 | 现有 `fixed_height_for_short_plan_when_all_completed`（改为同一实例两次 update，而非两个独立实例） |
| 长计划高度不变 | 现有 `fixed_height_for_long_plan_when_all_completed`（同上改造） |
| BottomPane 复用实例 | 新增：BottomPane 两次 `set_pinned_todo(Some)` 后，widget 的 watermark 保留（通过高度断言间接验证） |
| full 路径契约不变 | 现有 `pinned_todo_renders_identically_to_history_cell` |
| 空计划隐藏 | 现有 `renders_empty_pinned_todo` / `update_replaces_content` |
| expanded 不受影响 | 现有 expanded 相关行为 + 新增：expanded → 收起后高度仍等于展开前 folded 高度 |

## Assumptions & Unverified Items

| # | Assumption | Confidence | Impact if wrong | How to verify |
|---|---|---|---|---|
| 1 | update_plan 每次整体替换 plan 列表（非增量 diff） | 高（已核实 protocol.rs:101-117 TurnPlanUpdated 为整包快照） | 换计划重置水位线的触发条件失效 | core 侧 update_plan handler 复核 |
| 2 | 短计划（full ≤ max_lines）渲染高度与完成状态无关 | 高（已核实 plans.rs:53-103，步骤行不随状态增删） | 短计划也可能收缩，需另行处理 | 已证 |
| 3 | desired_height 是 bottom pane 布局唯一高度来源 | 高（已核实 FlexRenderable::desired_height → sum(children)，renderable.rs:187-192） | 锁定失效或双重计算 | 已证 |
| 4 | BottomPane 无其它新建 PinnedTodoWidget 的调用点 | 高（已 grep，仅 mod.rs:420 一处；session_flow.rs:27 为 None 清除） | 水位线被意外重置 | 已证 |
| 5 | 现有 insta snapshot 不含折叠态空行 padding | 中（已核实 `renders_pinned_todo_with_explanation_and_steps.snap` 为 full 路径，5 行无 padding） | 若 full 路径逻辑被误改，快照会 diff | `cargo insta review` |
| 6 | expanded 期间不更新 watermark | 中（设计决策，非事实假设） | 展开看到 >8 行、收起回到 ≤8 行会造成"跳变" | 用户已在 C1 Out 确认 expanded 不动 |

## Risk Register

| 风险 | 等级 | 缓解 |
|---|---|---|
| 宽度显著收窄后 watermark 涨到 max_lines，之后即使宽度恢复也保持 max_lines（可能留白） | 中 | 单调语义本身就是"宁高勿缩"；C1 已接受 |
| 换全新计划沿用旧水位线，面板可能比新计划实际需要的高 | 中 | 用户已确认不重置；接口预留：`update` 内一行可改重置 |
| expanded 与 folded 高度不一致造成视觉跳变 | 低 | C1 已声明 expanded 不动 |
| BottomPane 实例复用后，`expanded` 不再因新计划被重置为 false | 低 | `update` 保留 `expanded = false`，行为不变 |
| 测试改造（同实例跨 update）暴露现有 `folded_lines` 其它高度不一致 | 中 | 这正是修复目标；测试先行，失败即定位 |

## Parts

（单文件设计，不拆分）
