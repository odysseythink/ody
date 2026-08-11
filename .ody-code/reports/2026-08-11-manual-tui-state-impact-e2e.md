# 状态影响面模板：人工 TUI 端到端复测步骤

**目的：** 验证真实 Ody TUI 的 `/design`、`/plan`、popup、`submit_design` / `submit_plan` 和实际模式注入，能否让 `kimi_ranweiwei/kimi-for-coding` 遵守状态影响面规则。  
**不验证：** 代码实现本身、真实业务服务、CRUD API 的正确性。  
**模型：** `kimi_ranweiwei/kimi-for-coding`。  
**凭据：** 仅通过环境变量 `KIMI_API_KEY` 提供；不得粘贴、截图或写入实验产物。

---

## 1. 通过判据

原计划共有 12 个 artifact：4 个任务 × 3 种模式。当前 TUI 的 Plan Mode 未提供
Concise / Rigor 的可选入口，因此本轮只执行 4 个 Design artifact；8 个 Plan
artifact 记为**未执行（环境能力缺失）**，不参与通过率计算，也不能据此得出 Plan
模板有效或无效的结论。

| 类别 | 判据 | 通过门槛 |
|---|---|---|
| 有状态任务 | 每个 artifact 都写出：变更状态、读取/观察路径、一致性或副作用、失败处理、行为验证 | 完整矩阵恢复后：6 个 artifact 中至少 90% 全部覆盖；任一遗漏行为验证为关键失败 |
| 无状态任务 | 明确写 `Mutation Surface: N/A`（Design）或 `State-change impact surface: N/A`（Plan），并说明没有持久化、共享或跨边界可观察状态 | 完整矩阵恢复后：6 个 artifact 中至少 90% 正确 |
| 无状态误报 | 没有把静态文案或纯函数的输入输出误写成缓存、事件、审计、权限或 CRUD 影响面 | 完整矩阵恢复后不超过 10%；当前仅记录已执行的 Design 结果 |
| 宿主路径 | artifact 由对应 mode 的正式提交工具持久化；Design→Plan handoff 仅对完整设计通过 | 已执行的 4 次全部满足；Plan tier 选择能力补齐后再验证其余 8 次 |

## 2. 前置准备

1. 确认当前工作区包含本次模板修正，且先运行：

   ```bash
   cargo nextest run -p ody-collaboration-mode-templates
   ```
   测试结果
   ```
   ranwei@ranweideMac-mini ody % cargo nextest run -p ody-collaboration-mode-templates
       Finished `test` profile [unoptimized + debuginfo] target(s) in 0.15s
   ────────────
    Nextest run ID d2816f4e-6e50-422c-a757-fa042caf3921 with nextest profile: default
       Starting 7 tests across 1 binary
           PASS [   0.007s] ody-collaboration-mode-templates template_tests::plan_templates_require_risk_driven_verification_with_actionable_e2e_contracts
           PASS [   0.007s] ody-collaboration-mode-templates template_tests::split_templates_require_complete_detail_and_freeze_the_initial_manifest
           PASS [   0.007s] ody-collaboration-mode-templates template_tests::parts_table_examples_are_openable_as_written
           PASS [   0.008s] ody-collaboration-mode-templates template_tests::plan_and_rigor_workflow_share_the_same_understand_step
           PASS [   0.007s] ody-collaboration-mode-templates template_tests::design_template_mandates_request_user_input_for_options_and_exit_prompt
           PASS [   0.008s] ody-collaboration-mode-templates template_tests::plan_templates_only_name_tools_that_exist
           PASS [   0.008s] ody-collaboration-mode-templates template_tests::templates_require_conditional_state_change_impact_analysis
   ────────────
        Summary [   0.008s] 7 tests run: 7 passed, 0 skipped   
   ```
2. 建立隔离的 **Ody worktree**，而不是空白 Git 仓库。Design Mode 的 C8 要求复用扫描；空仓库会诱导模型在用户目录中盲目寻找源码，既不属于本实验，也会污染权限记录：

   ```bash
   eval_dir=$(mktemp -d /tmp/ody-tui-state-impact.XXXXXX)
   git worktree add --detach "$eval_dir" HEAD
   ```

   若不希望复制当前仓库内容，可保留空目录，但必须把第 4 节每个任务文本前加上固定评估前缀：`这是模板评估；当前隔离目录有意为空。不要扫描 cwd 之外的路径，不要寻找 Ody 源码；在 Reuse Analysis 中记录 greenfield / no reusable components。`

3. 确认 shell 中有凭据，但不要显示其值：

   ```bash
   test -n "${KIMI_API_KEY:-}"
   ```

4. 启动 TUI：

   ```bash
   KIMI_API_KEY="$KIMI_API_KEY" target/debug/ody \
     --model kimi_ranweiwei/kimi-for-coding \
     --cd "$eval_dir"
   ```

5. 在每个样本开始前新建会话或清空上下文。不要复用前一个样本的设计、计划或模型结论。

6. 每个样本完成后，从 `$eval_dir/.ody-code/designs/` 或 `$eval_dir/.ody-code/plans/` 复制 artifact 到单独的审阅目录，并记录：样本 ID、模式、实际 artifact 路径、时间、是否出现 popup / 手动补充说明。

## 3. 固定任务

为降低差异，逐字使用下列任务文本。不要向模型补充业务实现细节；若模型的问题属于产品偏好，使用推荐选项或在 popup 的自由文本中写“按任务文本采用最小方案”。

**扫描边界：** 不论使用 worktree 或空目录，拒绝任何以 `/Users`、`/`、`~` 为根的扫描。复用扫描只允许当前 `eval_dir` 及其版本控制内容。若模型发起越界命令，在 guardian popup 中选 `No`，并输入：`仅在当前实验工作区内探索；不要扫描 /Users 或寻找其他 Ody 仓库。本实验的复用分析限于当前 worktree。`

| ID | 类别 | 任务文本 |
|---|---|---|
| S1 | 有状态 | 新增一个可由 TUI 修改、持久化并由运行中服务热加载的工作区设置；既有客户端无需重启即可观察新值。 |
| S2 | 有状态 | 新增任务状态迁移（`queued`、`running`、`succeeded`、`failed`），并发布由 TUI 和审计流消费的 `task-updated` 事件。 |
| N1 | 无状态 | 修复一个纯字符串解析函数：它错误接受空片段；函数没有 I/O、持久化、共享可变状态，除返回值外没有可观察副作用。 |
| N2 | 无状态 | 仅修改一个静态 TUI 标签的显示文案；不得改变命令、配置、持久化、运行时状态或行为。 |



## 4. 每个任务的运行顺序

对 S1、S2、N1、N2 分别依次运行以下三个模式。原计划总计 12 次，每次均使用新会话。
本轮仅执行 A；B、C 因当前 Plan Mode 没有提供 Concise / Rigor 选择而延期。

### A. Design

1. 输入 `/design`，确认界面已显示 Design Mode。
2. 粘贴固定任务文本。
3. 对模型提出的闭合问题，使用 `request_user_input` popup 选择推荐项；不要在普通聊天文本中替代 popup。
   - 例外：若 popup 请求扫描 `eval_dir` 之外的路径，拒绝它并使用上文的扫描边界说明。这是环境探索偏航，不是产品选择；在评分表的“人工纠正”栏记为 `R-explore`，但不要把它记为状态影响面规则的失败。
4. 对无状态任务，若模型把静态文案/纯函数称为状态变更，要求它在 C3 中改为：

   ```markdown
   ### Mutation Surface
   Mutation Surface: N/A — <任务不含持久化、共享或跨边界可观察状态的原因>
   ```

   这次纠正须记录为一次模型初始误报。
5. 对有状态任务，在请求最终批准前检查 `## C3 Data Models` 是否有 `### Mutation Surface`，且每个状态变化至少包含：
   - Changed state；
   - Read/observation paths；
   - Consistency / side effects；
   - Failure handling；
   - Behavioral verification assertion。
6. 允许模型调用 `submit_design(final: true)`；若 C1–C8 或审查 gate 拒绝，记录拒绝原因，要求模型修正，然后再次提交。
7. 在 TUI 的后续菜单选择进入 Plan mode，确认 handoff 提示引用刚才的 Design artifact 路径。
8. 保存最终 Design artifact 路径和截图/文本记录。

#### S1 
  - 运行目录：<worktree /tmp/ody-tui-state-impact.u9dUtB>
  - session路径：/Users/ranwei/.ody-code/sessions/2026/08/11/rollout-2026-08-11T15-50-08-019fefcc-dd39-74e0-8dad-8af6568bd85e.jsonl
  - 落地文件路径：/tmp/ody-tui-state-impact.u9dUtB/.ody-code/designs/2026-08-11-tui.md
  - 人工纠正：`R-explore`。模型请求扫描 `/Users` 以查找 Ody 源码；已拒绝，并限定复用分析只在当前 worktree 内进行。该项不计为状态影响面失败。
  - 状态变更判定：正确。`log_level` 被持久化到工作区配置，并由服务和客户端观察。
  - 状态影响面覆盖：`✓` Changed state、`✓` observation paths、`✓` consistency / side effects、`✓` failure handling、`✓` behavioral verification。
  - C1–C8：结构上通过；C8 仅能证明空白实验 worktree 中没有可复用代码，不能证明对真实 Ody 源码的兼容性。
  - 判定：**有条件通过，暂不进入实施**。
  - 实施前必须修正：
    1. 统一重载失败语义：保留最近一次成功快照；仅首次启动且文件不存在时使用默认 `info`。
    2. 监听配置文件的父目录并处理 create / modify / rename，保证原子 `rename` 保存能被观察。
    3. 统一 `reload_from_disk` 的接口、调用方和 `SettingsError`（补齐序列化错误）。
    4. 基于真实 Ody 的日志初始化选择可重载 filter，不假设全局 max-level setter 足以热加载。
    5. 选定客户端为同进程或独立进程，并只保留一条明确的传播路径。

#### S2

  - 运行目录：`/tmp/ody-tui-state-impact.u9dUtB`
  - session路径：/Users/ranwei/.ody-code/sessions/2026/08/11/rollout-2026-08-11T16-44-56-019fefff-0a91-7302-861c-2d7a12b06662.jsonl
  - 落地文件路径：`/tmp/ody-tui-state-impact.u9dUtB/.ody-code/designs/2026-08-11-task_updated.md`
  - 状态变更判定：正确。任务状态被持久化，并由 TUI、审计流和指标观察。
  - 状态影响面覆盖：`✓` Changed state、`✓` observation paths、`✓` consistency / side effects、`✓` failure handling、`✓` behavioral verification。
  - C1–C8：结构上通过；C8 仅说明空白实验 worktree 中没有可复用代码，不能证明与真实 Ody 源码兼容。
  - 判定：**有条件通过，暂不进入实施**。
  - 实施前必须修正：
    1. 更正 JSONL 示例，使失败任务遵守 `queued -> running -> failed`，不得直接 `queued -> failed`。
    2. 按 task ID 串行化迁移，或使用版本 / CAS 校验，避免读取、持久化和内存替换之间的竞态。
    3. 统一 `TaskManager` trait 与算法：通过实现边界公开所需操作，或将伪代码放入 `TaskManagerImpl`。
    4. 明确“追加持久化成功但内存更新失败”的恢复语义；内存替换应不可失败，或提供可恢复事务 / 重放路径。
    5. 将 `EventBus` 记录为抽象依赖，不能把空 worktree 中不存在的实现当作真实 Ody 复用证据。
    6. 将任务文本未提供的决策标为 `[C:INFERRED]` 或待验证假设，不得标为用户已确认。

### B. Plan Concise（延期）

**本轮状态：未执行。** 当前 Plan Mode 没有可供人工选择 Concise tier 的入口，故不能
验证该正式宿主路径。不要通过普通聊天要求模型“写得 concise”来替代，因为这不会证明
host-selected tier、相应注入内容或 `submit_plan` artifact 的行为。

待 Plan Mode 提供 Concise 的显式选择后，按以下步骤复测：

1. 新建会话，输入 `/plan`；在 tier 选择中选择 **Concise**。
2. 粘贴同一固定任务文本。
3. 让模型完成正式计划并调用 `submit_plan`；不要接受普通聊天中的非持久化“计划”。
4. 检查 artifact：
   - S1/S2：存在状态影响面，覆盖五项；
   - N1/N2：存在精确文字 `State-change impact surface: N/A`，并有理由；
   - N2 特别检查：不得把“标签文本”本身归为 changed state。
5. 保存 Plan artifact 路径和结果。

### C. Plan Rigor（延期）

**本轮状态：未执行。** 当前 Plan Mode 没有可供人工选择 Rigor tier 的入口，故不能
将默认 Plan 行为当作一次可重复、可归因的 Rigor 测试。

待 Plan Mode 提供 Rigor 的显式选择后，按以下步骤复测：

1. 新建会话，输入 `/plan`；选择 **Rigor**。
2. 粘贴同一固定任务文本，完成并正式提交计划。
3. 除 Plan Concise 的检查项外，确认 `## Self-review` 的第 6 项覆盖：
   - 持久化、共享或跨边界可观察状态的识别；
   - 观察路径、一致性/副作用、失败处理；
   - 行为测试；
   - 无状态任务使用 `State-change impact surface: N/A`。
4. 保存 Plan artifact 路径和结果。

## 5. 评分表

每个 artifact 使用一行。`✓` 表示模型在最终 artifact 中自行满足；`R` 表示人工指出后才修正；`✗` 表示最终仍缺失。

| 样本 | 模式 | 状态 / N/A | 观察路径 | 一致性/副作用 | 失败处理 | 行为验证 | 人工纠正 | artifact 路径 | 判定 |
|---|---|---|---|---|---|---|---|---|---|
| S1 | Design | ✓ | ✓ | ✓ | ✓ | ✓ | R-explore | `/tmp/ody-tui-state-impact.u9dUtB/.ody-code/designs/2026-08-11-tui.md` | 有条件通过；实施前修正 5 项 |
| S1 | Concise | 未执行 | 未执行 | 未执行 | 未执行 | 未执行 | Plan Mode 无 Concise 选择 | — | 延期 |
| S1 | Rigor | 未执行 | 未执行 | 未执行 | 未执行 | 未执行 | Plan Mode 无 Rigor 选择 | — | 延期 |
| S2 | Design | ✓ | ✓ | ✓ | ✓ | ✓ | 未记录 | `/tmp/ody-tui-state-impact.u9dUtB/.ody-code/designs/2026-08-11-task_updated.md` | 有条件通过；实施前修正 6 项 |
| S2 | Concise | 未执行 | 未执行 | 未执行 | 未执行 | 未执行 | Plan Mode 无 Concise 选择 | — | 延期 |
| S2 | Rigor | 未执行 | 未执行 | 未执行 | 未执行 | 未执行 | Plan Mode 无 Rigor 选择 | — | 延期 |
| N1 | Design |  | N/A | N/A | N/A | N/A |  |  |  |
| N1 | Concise | 未执行 | 未执行 | 未执行 | 未执行 | 未执行 | Plan Mode 无 Concise 选择 | — | 延期 |
| N1 | Rigor | 未执行 | 未执行 | 未执行 | 未执行 | 未执行 | Plan Mode 无 Rigor 选择 | — | 延期 |
| N2 | Design |  | N/A | N/A | N/A | N/A |  |  |  |
| N2 | Concise | 未执行 | 未执行 | 未执行 | 未执行 | 未执行 | Plan Mode 无 Concise 选择 | — | 延期 |
| N2 | Rigor | 未执行 | 未执行 | 未执行 | 未执行 | 未执行 | Plan Mode 无 Rigor 选择 | — | 延期 |

## 6. 结果解释与决策

- **Plan tier 选择不可用：** 本轮 Plan Concise / Plan Rigor 的 8 项均为延期，不能套用
  12-artifact 门槛，也不能将其计作模板失败。先补齐可见且可重复选择的 Plan tier 宿主路径，
  再用第 4 节 B、C 的固定任务重跑这 8 项。

- **全部门槛通过且无人工纠正：** 接受第一阶段模板，关闭本议题；不新增 fragment 或语义 gate。
- **通过但出现 1 次 N2 类格式误报：** 保留当前修正，记录为模型偏差；下次版本/模型变动后重跑此矩阵。
- **任一有状态 artifact 缺失行为验证，或无状态误报超过 10%：** 不进入语义 gate；先修订提示词/示例并重跑完整矩阵。
- **C1–C8 / `submit_design` / `submit_plan` 的宿主逻辑和 artifact 不一致：** 这是宿主集成缺陷，单独建立修复任务；不要用新 fragment 掩盖。

## 7. 清理

实验完成且已复制必要 artifact 后，确认 `eval_dir` 是本次创建的 `/tmp/ody-tui-state-impact.*` 目录，再删除它。不要删除真实项目目录或 `$ODY_HOME`。
