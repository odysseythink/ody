# PinnedTodoWidget 高度锁定修复设计

> 状态：草稿（增量构建中）— 本文件按段落逐步补全，每段提交一次 checkpoint。

## 背景

- 现象：底部固定 Todo 面板初始显示 4–5 条任务，随着任务完成高度不断收缩，违背 commit `d51b4d5` "keep PinnedTodoWidget height fixed as tasks complete" 的意图。[C:USER 报告]
- 根因（Deep 审计已逐条核实）：
  1. `PinnedTodoWidget` 只有 `explanation / plan / max_lines / expanded` 四个字段（`tui/src/bottom_pane/pinned_todo.rs:32-39`），没有持久化的目标高度；`display_lines` 每次渲染现算 `target = full.len().min(max_lines)`（`pinned_todo.rs:107`）。[C:UPSTREAM]
  2. `BottomPane::set_pinned_todo` 每次 `update_plan` 都 `PinnedTodoWidget::new()` + `update(args)` 重建 widget（`tui/src/bottom_pane/mod.rs:417-429`），任何跨帧状态都会丢失。[C:UPSTREAM]
  3. 空行 padding（`pinned_todo.rs:115-117`）补到的是"已经缩了的 target"而非初始高度。[C:UPSTREAM]
- 用户确认的锁定语义：**硬锁一次 + 复位事件**——首次渲染锁定 `min(max_lines, full_height)`，此后恒定；仅在新计划替换/线程切换（`set_pinned_todo(None)`）时复位。[C:USER]

## Scope

### Scope In
- `PinnedTodoWidget` 增加持久化锁定高度状态与复位逻辑。[C:INFERRED]
- `BottomPane::set_pinned_todo` 改为复用现有 widget 实例（保留锁定字段），无实例时才新建。[C:INFERRED]
- 复位触发点定义：新计划替换（step 集合变化的 `update`）、`set_pinned_todo(None)`（线程切换，`tui/src/chatwidget/session_flow.rs:27`）。[C:INFERRED]
- 新增/修正单元测试：同一 widget 实例跨多次 `update_plan` 且内容变缩时高度不变（覆盖报告指出的真实路径）。[C:INFERRED]

### Scope Out
- 折叠视图内容策略（优先显示 in-progress/pending、footer 汇总）不变。[C:INFERRED]
- expanded（ctrl+e）模式不锁高度，展开/收起行为不变。[C:INFERRED]
- 历史区 `PlanUpdateCell`、status 面板、composer 高度逻辑均不改动。[C:INFERRED]
- 终端变窄导致锁定高度被裁剪的问题：本期记录为已知限制，不做 viewport clamp（见错误降级表）。[C:USER 选定硬锁一次语义的附带取舍]

## C2. Architecture / Design

（待补：组件图、数据流、锁定生命周期状态机）

## C3. Data Models

（待补：`PinnedTodoWidget` 新字段、复位触发判据的类型签名）

## C4. Algorithms

（待补：`update` 复位判定、`display_lines` 锁定高度计算的伪代码）

## C5. Error Handling / Degradation

（待补：场景 → 行为表，含窄终端裁剪、空计划、plan 为空但 explanation 非空等）

## C6. Self-Review

（待补）

## C7. User Approval

（待补：最终确认记录）

## C8. Reuse Analysis

（待补：复用候选清单——已有初步结论，正式段落下一轮补全）

## Assumptions & Unverified Items

| # | Assumption | Confidence | Impact if wrong | How to verify |
|---|---|---|---|---|
| 1 | 报告三处根因定位准确（widget 无锁定字段 / set_pinned_todo 重建 / padding 锚点现算） | 高（已读源码核实） | 设计目标偏移 | 已核实：pinned_todo.rs:32-39,107,115-117；mod.rs:417-429 |
| 2 | `max_lines=8` 为唯一生产取值（Default），无外部配置注入 | 高（全仓 grep 核实） | 锁定上限语义需随配置泛化 | `max_lines: \d+` grep 仅命中 pinned_todo.rs 与无关的 status_state.rs |
| 3 | `set_pinned_todo` 仅两个生产调用点（turn_runtime.rs:421 Some / session_flow.rs:27 None） | 高（grep 核实） | 复位事件覆盖不全 | 已核实全部调用点 |
| 4 | expanded 模式不需要锁高度 | 中 | 展开时高度仍随内容变化，用户若期望展开也锁则需追加范围 | 需用户确认 |
