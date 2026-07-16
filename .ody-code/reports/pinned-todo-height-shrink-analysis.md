# PinnedTodoWidget 高度随任务完成收缩 — 根因分析报告

日期：2026-07-16
范围：TUI `PinnedTodoWidget`（底部 Todo 面板）
现象：Todo 初始显示 4-5 条，随着任务完成，面板高度不断收缩，不符合"达到最大高度后不再缩减"的预期。

## 现象示例

```
• Todo  (6/8 done)
  └ ☐ Task 7: Update safety.rs write-denied reason
    ☐ Task 8: Rewrite design.md template
    ✓ 6 completed
```

## 根本原因

提交 `d51b4d5`（fix(tui): keep PinnedTodoWidget height fixed as tasks complete）试图实现"锁定高度"，但实现中**缺少持久化的目标高度状态**，导致锁定逻辑失效。三处代码共同导致高度收缩：

### 1. Widget 没有记住历史高度

`PinnedTodoWidget` 仅有 `explanation / plan / max_lines / expanded` 四个字段（`tui/src/bottom_pane/pinned_todo.rs:32-39`），**没有任何字段记录"首次显示时的高度"**。

`display_lines` 的注释（`pinned_todo.rs:92-94`）声称 "lock to `min(max_lines, full_content_height)`"，但实际代码（`pinned_todo.rs:107`）是：

```rust
let target = full.len().min(self.max_lines);
```

`full.len()` 是**当前这一次**渲染的内容高度，每次渲染都重新计算，并非"首次显示时锁定"的快照。任务完成越多，`render_plan_steps` 产出的 `full` 越短，`target` 随之变小。

### 2. 每次 update 都重建 widget，状态无法跨帧保留

`on_plan_update`（`tui/src/chatwidget/turn_runtime.rs:421`）在每次 `update_plan` 时调用 `set_pinned_todo(Some(update))`；而 `set_pinned_todo`（`tui/src/bottom_pane/mod.rs:417-422`）的实现是 `PinnedTodoWidget::new()` + `update(args)`，**新建一个 widget 替换旧的**。

即便给 widget 加上"锁定高度"字段，也会在这一步被丢弃。这是第二层断点。

### 3. 空行 padding 补的是"已经缩了的 target"，不是初始高度

`pinned_todo.rs:115-117`：

```rust
// Keep the height fixed while tasks are being completed.
while lines.len() < target {
    lines.push(Line::from(""));
}
```

padding 确实存在，但补到的是**当前这次**算出的 `target`（已随任务完成而变小），而非最初的最大高度。padding 机制存在，锚点本身却在缩。

## 为什么单测全绿却漏了

`fixed_height_for_short_plan_when_all_completed`（`pinned_todo.rs:375`）与 `fixed_height_for_long_plan_when_all_completed`（`pinned_todo.rs:420`）用 `desired_height` 断言，但：

- short-plan 用例中，`render_plan_steps` 对 3 步恒产出 1 header + 3 行，`full.len()` 在"初始"和"全完成"两种状态下都是 4，`target` 没变，测试恒过。
- 两个测试都各自**新建** widget 比较两个独立实例的高度，没有覆盖"同一 widget 实例跨多次 `update_plan` 且 `full` 变缩"的真实路径。

换言之，测试验证的是"内容行数不变"，而非"锁定机制跨 update 生效"。

## 调用链

```
update_plan (model tool call)
  └─ turn_runtime.rs:409  on_plan_update(update)
       └─ turn_runtime.rs:421  bottom_pane.set_pinned_todo(Some(update))
            └─ mod.rs:420-422  PinnedTodoWidget::new() + update(args)  // 旧 widget 被丢弃
                 └─ pinned_todo.rs:95  display_lines(width)
                      └─ pinned_todo.rs:107  target = full.len().min(max_lines)  // 每次现算
```

## 修复方向（未实施）

1. 让目标高度成为 widget 的持久状态：首次渲染时记录 `locked_height = min(max_lines, full_height)`，后续 padding 到该值而非现算的 `target`。
2. `set_pinned_todo` 改为复用现有 widget（保留锁定字段），仅在无 widget 时新建，而非每次重建。

> 本报告仅为根因分析，未改动任何代码。
