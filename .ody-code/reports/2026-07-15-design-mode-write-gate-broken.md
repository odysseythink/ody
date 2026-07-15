# Design 模式设计文件写门禁必然拦截 —— 问题分析与解决方案报告

- **Date:** 2026-07-15
- **触发会话:** `C:\Users\Administrator\.ody-code\sessions\2026\07\15\rollout-2026-07-15T21-20-14-019f65ef-5f52-7ee1-8476-1a8363a8a18c.jsonl`（cwd `D:\go_work\hermind`，Design 模式，Deep 审计级）
- **分析方式:** 只读源码取证 + 会话 JSONL 交叉验证；未做任何代码改动
- **Verdict:** 确认为确定性 bug（deterministic，与模型行为无关）——Design 模式下设计文件**永远不可能写入成功**，且 Design→Plan 切换门禁会连带 Veto。推荐方案 A（为 Design 增加 submit 工具，平移 Plan 的宿主代写闭环）。

---

## 1. 问题概述

用户在 Design 模式完成设计并批准后，模型反复尝试将设计落盘到 `.ody-code/designs/2026-07-15-desktop-webview-llm-preference.md`，全部被写门禁拒绝：

```
Design mode is read-only. Finish designing and switch to Plan or Default mode to make changes.
[design-mode-blocked] (file: D:\go_work\hermind\.ody-code\designs\2026-07-15-desktop-webview-llm-preference.md)
```

被拦的恰恰是**设计文件本身**——按 AGENTS.md 与 design.md 模板的约定，它是 Design 模式下唯一允许写入的文件。模型最终只能把设计全文贴在对话里，无法归档。

## 2. 会话取证

该 session 中共有 **4 次 `apply_patch` 被 `[design-mode-blocked]` 拒绝**：

| JSONL 行 | 时间 (UTC) | 目标文件 | 拦截是否合理 |
|---|---|---|---|
| 163 | 13:28:12 | `.ody-code\designs\2026-07-15-desktop-webview-llm-preference.md`（Add File，完整设计初稿） | **不合理**（设计文件本身） |
| 256 | 13:33:34 | `D:\tmp\check_webview.ps1`（工作区外脚本） | 合理 |
| 303 | 13:36:34 | 同上设计文件（`# test` 写权限探测） | **不合理** |
| 316 | 13:40:24 | 同上设计文件（用户批准后的最终落盘） | **不合理** |

关键交叉验证：会话全文 grep 不到任何 `tmp-*.md` 字符串——**模型自始至终从未被告知白名单所认的"已分配路径"**。

## 3. 写门禁机制（工作正常的部分）

- `core/src/apply_patch.rs:102-138`：Plan/Design 模式下 `apply_patch` 先过 `plan_mode_gate_for_patch`，传入 `turn_context.plan_artifact`。
- `core/src/safety.rs:102-110`：白名单 = 补丁中**所有**路径都满足 `artifact.is_plan_file_path(path)`，否则按 enforcement 处理。
- 默认 `PlanEnforcement::Strict`（`config/src/config_toml.rs:108-111`）→ `PlanGateDecision::Deny`，产出会话中那条拒绝消息（文案见 `safety.rs:60,70-75`）。
- `is_plan_file_path`（`core/src/plan_artifact.rs:314-339`）只认两类目标：① artifact 当前注册路径本身；② 其 `<stem>/` 目录下的 `.md` 文件（split parts）。

## 4. 根本原因：三重断裂

### 断裂 1 —— 白名单里的"已分配路径"永远是临时名 `tmp-<thread_id>-<date>.md`，Design 模式下无任何生产代码将其定名

- artifact 创建：`core/src/session/turn_context.rs:870-871` → `PlanArtifact::new_design(...)` → 初始状态 `Temporary { temp_path }`（`core/src/plan_artifact.rs:119-121`）。本会话对应：
  `D:\go_work\hermind\.ody-code\designs\tmp-019f65ef-5f52-7ee1-8476-1a8363a8a18c-2026-07-15.md`
- 定名（`Temporary → Finalized`）仅两个入口：
  - `write_plan` 内部调 `apply_finalized_name`（`plan_artifact.rs:214`）；
  - `finalize_name`（`plan_artifact.rs:172`）——注释自述 "No production call site left"，全仓库核实确无生产调用。
- `write_plan` 的**唯一**生产调用点是 `submit_plan` 工具（`core/src/tools/handlers/submit_plan.rs:215`），而 `submit_plan.rs:146-150` 硬性拒绝非 Plan 模式：

  ```rust
  if turn.collaboration_mode.mode != ModeKind::Plan {
      return Err(... "submit_plan is only available in Plan mode" ...);
  }
  ```

- 且 `submit_plan` 工具仅当 `mode == ModeKind::Plan` 才注册（`core/src/tools/spec_plan.rs:688-690`）——Design 模式下该工具甚至不在工具列表中。
- **结论：Design 模式下 artifact 路径永远停在 `tmp-...md`，白名单永远只认这个 tmp 路径及其 `<tmp-stem>/` 目录。**

### 断裂 2 —— 模板声称"宿主已分配确切路径"，但该路径从未注入给模型

- `collaboration-mode-templates/templates/design.md:82`："The host has assigned the exact design file path for this session. Write to **that exact path**"。
- 渲染管线只做 `{{ split_threshold }}` 一个占位符替换（`core/src/context/collaboration_mode_instructions.rs:169-189`），**没有设计文件路径占位符**。
- `plan_mode_injector.rs` 的提醒注入同样不含 `artifact.path()`（:508 的唯一出现是测试代码）。

### 断裂 3 —— 模板同时指示模型自造文件名，而该文件名必然不匹配白名单

- `design.md:155`："Use the filename format `YYYY-MM-DD-<topic>.md`"。模型严格照做（`2026-07-15-desktop-webview-llm-preference.md`），与白名单的 `tmp-...md` 不匹配 → `is_plan_file_path` 两个分支均失败 → 每次必被 `Deny`。
- 三重断裂叠加 ⇒ **确定性 bug**：即使模型写 `.ody-code/designs/` 下任何合法文件名都会被拦；唯一能通过的路径（tmp 名）模型又无从得知。会话中模型改用 `Test-Path` 探测、写 `# test` 试探、批准后重试，全部徒劳，与现象完全吻合。

## 5. 次生问题：Design→Plan 切换门禁连带 Veto（"写不进"之后"出不来"）

- `evaluate_design_exit`（`core/src/session/design_handoff.rs:79-85`）用 `read_artifact_content` 从 `artifact.path()`（仍是 tmp 路径）`tokio::fs::read_to_string`；注释自述 "Fail-safe: any unreadable/missing artifact is treated as incomplete"。
- 设计文件从未写入 ⇒ 读到空串 ⇒ C1–C8 判定不完整 ⇒ Strict enforcement 下 `/plan` 切换被 **Veto**（`design_handoff.rs:40-44`）。

## 6. Plan 模式对照：没有此问题，且是"三件套"全闭环

Plan 模式同样以 tmp 路径起步（`turn_context.rs:868` `PlanArtifact::new_temp`），但靠三件机制闭环：

**① 主文件保存不让模型碰路径 —— `submit_plan` 宿主代写**
- 模板合同：`plan.md:136` "Only `submit_plan` persists the plan file"；`plan.md:184` "**Persistence is automatic** ... you do not need shell commands or a write tool"。
- 落盘：`submit_plan.rs:215` → `write_plan` → `plan_artifact.rs:224` `write_atomically` —— **宿主代码直接写盘，完全不经过 apply_patch 写门禁**。
- 命名：`write_plan` 从 plan 的 `# Title` 自动派生 slug（`slug_from_markdown_title`，`plan_artifact.rs:458-471` → `apply_finalized_name` :186-197），产物恰好是 `YYYY-MM-DD-<slug>.md`。故 `plan.md:182` 的命名约定是**描述宿主行为**，与 design.md:155 让模型直写形成关键反差。

**② split part 需模型直写时，宿主把解析后的绝对路径喂给它**
- part 用普通写工具直写（`plan.md:163`），经白名单 stem 分支（`plan_artifact.rs:322-338`）。
- `submit_plan` 返回值显式携带 stem 绝对路径（`submit_plan.rs:303-316`），注释明言设计意图：
  > "The model must know the exact resolved stem directory ... the model cannot reliably reconstruct it on its own. **Without this, a guessed part-file path can miss the plan-mode write whitelist entirely**"

**③ after-turn hook 驱动 manifest 推进，directive 注入精确 part 路径**
- `turn.rs:2552-2554`：回合结束读 `artifact.last_plan_text()`（由 `write_plan` 设置）→ `after_plan_turn` 解析 `## Parts` manifest → `render_directive` 用 `resolved_part_path(...)`（`plan_mode_injector.rs:227-262`）把解析后的绝对 part 路径注入下一回合。

**Design 侧现状：基础设施全部就绪，唯独差最后一环**
- `render_directive` 已写好 Design 分支（`plan_mode_injector.rs:244-254`），但生产不可达：after-turn hook 依赖 `last_plan_text`，而它在 Design 下永远为 `None`。
- `new_design`（`plan_artifact.rs:105`）、`design_handoff` 的 C1–C8 gate 均已就位，被同一断点架空。

## 7. 解决方案（方案 A：把 Plan 的闭环平移到 Design）

### 7.1 改动清单

| # | 位置 | 改动 |
|---|---|---|
| A1 | `core/src/tools/spec_plan.rs:688-690` | `SubmitPlanHandler` 注册条件由 `mode == Plan` 放宽为 `Plan \| Design`（或新增 `SubmitDesignHandler`，结构对称）。 |
| A2 | `core/src/tools/handlers/submit_plan.rs:146-150` | ModeKind 校验放宽为 `Plan \| Design`；split 返回消息（:303-316，携带 `stem_dir` 绝对路径）对 Design 同样生效。 |
| A3 | `core/src/tools/handlers/submit_plan_spec.rs:18-23` | 工具描述按模式参数化（`.ody-code/plans/` vs `.ody-code/designs/`）。 |
| A4 | `collaboration-mode-templates/templates/design.md` | Step 4 / "Design file location" 改为 submit 合同（镜像 `plan.md:136/184` 的 "persistence is automatic"）；删除让模型直写 `YYYY-MM-DD-<topic>.md` 的指示；删除 "host has assigned the exact design file path" 的虚假措辞。 |
| A5 | （可选增强）submit handler 内复用 `core/src/design_completeness.rs` 的 `design_completeness_report` 对 Design 提交做 C1–C8 机械校验（对应 `rigor_structure_gap` 的做法），提前于 handoff 拦截不完整设计。 |

### 7.2 为何零改动即可复用的现成设施

- `PlanArtifact::new_design` 已将 artifact 根目录指向 `designs/` 子目录；`write_plan` 的 title-slug 定名、原子写盘、`last_plan_text` 缓存对 Design 天然适用。
- after-turn hook（`turn.rs:2552`）一旦拿到 `last_plan_text`，`render_directive` 的 Design 分支（`plan_mode_injector.rs:244-254`）自动激活。
- `design_handoff` 的 `read_artifact_content` 因文件真实落盘而恢复功能，**连带修复第 5 节的切换 Veto**。

### 7.3 被否决的备选

- **方案 B（模型直写 + 注入 assigned 路径）**：进入 Design 模式时立即 `finalize_name` 并在模板加 `{{ design_file_path }}` 占位符。缺点：进入模式时 topic 未知，定名时机尴尬（需改名机制或接受泛名）；直写合同与 Plan 的"宿主代写"哲学继续分叉，两份模板需长期同步维护。
- **方案 C（A+B 混合）**：submit 工具为主、路径注入兜底。可作为 A 的后续增强，非必需。

### 7.4 推荐理由

方案 A 改动集中（一个注册条件 + 一个 ModeKind 校验 + 模板文案），却复用全部现成基础设施，并把 Design/Plan 两份合同重新对齐——代码库的演化方向本就如此（Design 侧半成品均已就位），方案 A 只是补上最后一块拼图。

## 8. 测试建议

1. `safety_tests.rs` / `apply_patch_tests.rs`：Design 模式下 `apply_patch` 写 `<finalized-stem>/part.md` 放行、写其他路径仍 `Deny`。
2. `submit_plan.rs` 测试：Design 模式调用成功落盘到 `designs/YYYY-MM-DD-<title-slug>.md`；split 返回消息含 `stem_dir`。
3. 端到端：Design 模式 submit → `last_plan_text` 非空 → after-turn directive 注入 → part 直写放行 → `/plan` 切换时 `evaluate_design_exit` 读到完整设计、C1–C8 通过、handoff reminder 注入。
4. 回归：Plan 模式行为不变（注册条件放宽不影晌 Plan 原有路径）。

---

*证据获取日期 2026-07-15；所有 file:line 基于工作区 HEAD（a87b582 附近）。*
