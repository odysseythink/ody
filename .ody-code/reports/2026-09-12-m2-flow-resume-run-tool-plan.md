# M2 执行计划：checkpoint/replay 恢复 + flow__run 模型工具 + 运行前审批

日期：2026-09-12
上位文档：`2026-09-11-flow-skill-multi-runtime-report.md`（§6 M2 范围）
状态：待评审
Last Updated: 2026-09-12

---

## Execution Rubric

复用 M1 执行计划（`2026-09-11-m1-flow-yaml-execution-plan.md`）已批准的同一套 rubric（拆分粒度三触发器、~256k 执行上下文预算、normal/plan/design 三档判定与平票规则），不重新立法。M1 全程按此执行无偏差。

### 已锁定的 M2 架构决策（缩小未知域，避免过度 design）

1. **checkpoint 内容**：只缓存 `agent()` 结果对（插值后的 prompt → 最终输出文本）。phase/log 进度事件不缓存（重跑会重发，TUI 可容忍重复进度行）。pipeline/parallel 的 fan-out 逐个 agent 缓存，恢复粒度 = 单个 agent 调用。
2. **checkpoint 键**：`(plan_fingerprint, step_path, item_index)`。`plan_fingerprint` = plan 源文本的 SHA-256 短哈希（flow.yaml 原文），`step_path` = `phase_index/step_index` 的稳定位置路径，pipeline/parallel 内再加 `item_index`。args 不同导致插值 prompt 不同 → 天然不同键，无需单独存 args。重跑时源文件被改 → fingerprint 变 → 全量重跑（正确语义：plan 变了不应用旧缓存）。
3. **run 标识与 resume 入口**：run_id 沿用 M1.4 的 run_call_id。slash 重触发同一 flow 技能名时，若存在未完成/已中断的同 fingerprint checkpoint 则 resume，否则新 run。M2 不做跨进程自动恢复（仍是回合内执行）；checkpoint 落盘的收益 = 同会话重触发/同 turn 内失败重试不重跑已完成 agent。
4. **checkpoint 存储位置**：跟随既有会话状态持久化。执行 M2.1 时做一次性勘察在 `thread-store` artifacts 与 session 状态目录之间锁定（倾向：session 状态目录下 `flow-checkpoints/<run_id>.json`，单文件原子写、零新依赖、随会话清理）；**不允许**为此引入新 crate 或新数据库表。
5. **flow__run 执行位置**：host 侧 core 执行（与 M1 决策 1 一致）。extension 侧工具只做：参数校验 → guardian 审批 → 转交 core flow runtime。模型工具不直接持有 session spawn 能力。
6. **审批粒度**：一次 flow run 一次 guardian 审批，展示 plan 摘要（技能名、phase 数、每 phase step 数与类型、prompt 模板预览截断）。slash 路径与 flow__run 路径共用同一审批组件与摘要渲染。
7. **schema 校验重试**：纳入 M2（M1.5 走查发现的真实模型几乎不输出纯 JSON）。flow.yaml agent step 增加可选 `schema:` 字段（JSON Schema 子集，先支持 `type/object/required/properties`），runtime 在带 schema 时把要求注入 prompt、解析+校验失败自动重试（上限 3 次，指数退避不需要——本地调用直接连续重试）。父报告原列 M4，提前到 M2.4（小叶子，不阻塞主线）。

---

## 子阶段拆分

### M2.1 [plan] 内核 checkpoint 钩子 + 恢复执行 — `core/src/flow/`

- 内容：`FlowAgentHost` 增加 `checkpoint_read/checkpoint_write`（RPITIT 默认空实现，与 `report_progress` 同款模式）；runtime 在 agent step 执行前后查/写缓存，命中即跳过 spawn；`FlowContext` 增加 `plan_fingerprint`（validate 时算）与 `resume_run_id: Option<String>`；slash 重触发路径（`trigger.rs`）检测同 fingerprint 未完成 checkpoint 时传 resume_run_id。存储落点按决策 4 勘察锁定。
- 源码依据：`core/src/flow/mod.rs:185-225`（FlowAgentHost trait，新增 provided 方法零破坏）、`core/src/flow/runtime.rs:267`（run_agent_batch，cache 查询点）、`core/src/flow/trigger.rs`（run_flow_skills_in_turn，resume 检测点）、`core/src/flow/host.rs:218`（SessionFlowAgentHost 覆写写盘）。
- 验收：`cargo nextest run -p ody-core` flow 模块（新增：中断-恢复 mock 测试——第二批 agent 失败短路后重跑，第一批全部命中缓存零 spawn；fingerprint 变化全量重跑）。

### M2.2 [plan] 运行前 guardian 审批（slash 路径）— `core/src/flow/` + `core/src/guardian/`

- 内容：`GuardianApprovalRequest` 新增 `FlowRun` variant（plan 摘要 JSON：技能名/phases/steps/prompt 预览，per approval_request.rs:17 既有枚举模式）；slash 触发分流后、`run_flow_skills_in_turn` 前挂审批；`approval_policy = never` 直过（extension_tools.rs:70-77 既有语义）；TUI/审批渲染端补臂（编译器穷尽匹配强制列出扇出）。
- 源码依据：`core/src/guardian/approval_request.rs:17`（枚举+serialize 模式）、`core/src/tools/handlers/extension_tools.rs:70-88`（review_approval_request 调用形态）、`core/src/session/turn.rs:1023-1033`（挂接点）、`core/src/flow/trigger.rs`。
- 与 M2.1、M2.3 **可并行**（互不触碰：M2.2 改 guardian+trigger 壳，M2.1 改 runtime 内核与 host trait）。
- 验收：`cargo nextest run -p ody-core`（审批通过/拒绝/never 三路径测试）；guardian snapshots 若涉及则更新。

### M2.3 [design] flow__run 模型工具 + catalog 可见性拆分 — `ext/skills/`

- 内容：① catalog 可见性拆成 `prompt_visible`（slash 列表）与 `is_model_invocable`（模型工具）两个独立维度：`is_model_invocable` 放行 `SkillType::Flow`（catalog.rs:214-219 去掉 Inline|Prompt 硬编码门，Flow 走 flow__run 而非裸 read）；`prompt_visible = false` 保持（Flow 不进 slash 补全以外的 prompt 目录）。② 新 `flow__run` 工具（tools/ 下参照 read.rs 模式：ReadTool 结构 + skill_function_tool spec + parse_args），参数 `name + args`，行为：查 catalog → 取 flow_artifact plan → 走决策 5 的 host 通道执行 → FlowOutcome 作为工具结果返回。**真实未知**：extension 工具进程驱动 core flow runtime 的集成缝（M1.3 slash 路径在 core 内，反向通道是否存在需勘察：ExtensionContext 的 session 服务可达性）。动手前先冻结集成缝 spec，猜错代价大（工具签名 + 审批链路 + catalog 语义三处扇出）。
- 源码依据：`ext/skills/src/catalog.rs:124,174-219`（两个可见性维度）、`ext/skills/src/tools/{mod.rs:56-59, read.rs}`（工具注册模式）、`ext/skills/src/extension.rs`（ExtensionContext 能力面）。
- 依赖：与 M2.1/M2.2 并行；执行期若集成缝勘察结果改变决策 5，回报后修订。
- 验收：`cargo nextest run -p ody-skills-extension`（catalog 维度拆分单测 + flow__run 参数/拒绝路径）；审批语义与 M2.2 共用。

### M2.4 [normal] schema 校验重试 — `core/src/flow/` + `ody-core-skills/`

- 内容：flow.yaml agent step 可选 `schema:` 字段（FlowPlan step 解析）；runtime 带 schema 时：schema 要求注入 prompt 尾部 → 子代理输出 JSON 解析 + 校验 → 失败重试至 3 次，仍失败按该 agent 失败短路（既有语义）。校验器用 workspace 已有 jsonschema 依赖（Cargo.toml 全 workspace 搜确认，无则 serde_json 手写最小子集）。
- 源码依据：`core/src/flow/runtime.rs`（agent step 执行点）、`ody-core-skills/src/flow.rs:12-49`（FlowPlan schema）。
- 依赖：M2.1（避免并发改 runtime.rs 冲突）。
- 验收：`cargo nextest run -p ody-core-skills -p ody-core`（schema 正例/坏 JSON/坏 schema/超限四路径）。

### M2.5 [normal] e2e + 手动走查 — `core/src/flow/flow_tests.rs` + TUI

- 内容：e2e 覆盖"中断-重触发 resume"全链（drive 部分子代理 → 模拟中断 → 重跑 → 断言已完成零 spawn、失败处继续）；flow__run 工具 e2e（模型侧触发 → 审批 → 执行 → 结果回工具）；TUI 手动走查（参照 M1.5 test-tui 流程，重点看审批弹窗与 resume 提示）。
- 依赖：M2.1–M2.4 全部。
- 验收：`cargo nextest run -p ody-core --tests` + 手动走查记录回填本报告。

---

## 依赖图

```
M2.1 [plan]   内核 checkpoint 钩子 + 恢复 (core/src/flow/)
M2.2 [plan]   运行前 guardian 审批 (guardian + trigger 壳) ── 并行，互不触碰
M2.3 [design] flow__run 工具 + catalog 可见性拆分 (ext/skills) ── 并行（design 先行冻结集成缝 spec）
   │（M2.4 依赖 M2.1 落地，避免并发改 runtime.rs）
M2.4 [normal] schema 校验重试
   │
M2.5 [normal] e2e + 手动走查（依赖 M2.1–M2.4）
```

显式并行声明：M2.1 / M2.2 / M2.3 三路可同时开工（M2.3 的 design 勘察先行）；M2.4 等 M2.1 合并；M2.5 收尾。

## 测试命令汇总

- `cargo nextest run -p ody-core`（M2.1、M2.2、M2.4、M2.5）
- `cargo nextest run -p ody-skills-extension`（M2.3）
- 不做全 workspace 测试（CI 负责）。

## 风险

- M2.3 的集成缝勘察若发现 extension→core 无现成反向通道，需要在 extension API 加 host callback（扇出到 extension_tools.rs 与所有 ExtensionContext 实现），工作量超 1 天则回报用户重排（可能拆 M2.3a API 通道 / M2.3b 工具本体）。
- checkpoint 文件的会话生命周期（何时清理）M2.1 锁定一个最小策略（run 完成即归档/随会话保留最近 N 个），不做自动 GC 守护。
- M2.2 的 TUI 审批渲染若无现成 guardian 弹窗组件复用，范围会膨胀——执行时先找既有渲染点，没有则降级为文本确认（y/N prompt），并在报告中记录。


---

## 执行记录（回填）

### M2.4 ✅（commit `e83cb8d9`）

- `ody-core-skills` flow.rs：`Agent` step 增加可选 `schema:` 字段（`serde_json::Map`，scalar 视为解析错误）。
- core 新增 `core/src/flow/schema.rs`：手写最小 JSON Schema 子集校验器（`type/properties/required/items/enum`，未知关键字忽略），9 个内联单测。
- runtime.rs：带 schema 时 prompt 尾部注入 "Reply with JSON only ..."；`run_one_agent` 加 schema 参数 + 重试循环（`SCHEMA_MAX_ATTEMPTS=3`，失败原因反馈进重试 prompt，成功才 checkpoint_write；host 失败直接短路不重试）；pipeline batch 传 None。
- checkpoint 键 = 注入 schema 指令后的完整 prompt 的 SHA-256。
- flow_tests.rs M2.4 段 8 个测试全过；`cargo test -p ody-core-skills --lib` 140/140。

### M2.5 ✅（e2e 测试 commit 见 git log；走查记录如下）

**自动化 e2e（`core/src/flow/flow_tests.rs` M2.5 段，2 个测试）**

1. `flow_resume_e2e_replays_completed_agents_and_continues_after_interrupt`：3 步串行 flow；run1 drive A → B 在飞时 `abort()`；断言磁盘 checkpoint 恰好 1 条（step A）；run2 零 spawn 复用 A、续跑 B/C；成功后 checkpoint 文件删除。
2. `flow_run_model_tool_end_to_end_with_guardian_approval`：wiremock reviewer 返回 allow；`FlowRunner::run_flow` 全链（审批门 → spawn → drive → 结果回传）；断言 guardian 请求体含 flow 名、事件流上 `[InProgress, Approved]`。
   - 关键坑：`make_session_and_context_and_config_and_rx` harness 强制 kimi provider（需 `KIMI_API_KEY`），guardian review 会炸；必须换成 `make_session_and_context_with_rx` 并在 config clone 上重置 `model_provider_id/model_provider/model_providers` 为 `TEST_PROVIDER_ID/test_provider()`（mirrors `guardian_test_session_turn_and_rx`）。该 harness 的 `make_session_and_context_with_rx` 也是 kimi forcing（别被名字骗）。
   - 事件流断言注意：`next_spawn_prompt` 会顺手丢弃途中的 `GuardianAssessment` 事件，需在 helper 里收集。

**TUI 手动走查（2026-09-12，真实凭证，隔离 `ODY_HOME=/tmp/m25-walk.ehW3EW`，`/walkflow` 2 步 flow，approval_policy=on-request + auto_review）**

环境：`guardian_approval = true` + `approvals_reviewer = "auto_review"`，模型 kimi-for-coding（真实 API）。

1. **审批链真实工作 ✅**：4 次 flow run 的 guardian review 全部命中真实模型并 allow（增量 review conversation 复用，后续审批仅 ~3s）；TUI footer 持续渲染 `Reviewing approval request └ flow run 'walkflow'`（M2.2 的 TUI 渲染）；日志确认审批请求体含 flow_name + plan summary（FlowPlanSummary）。low risk 自动放行，无交互弹窗（符合设计）。
2. **成功路径 ✅**：run 1 三步全部完成，checkpoint 成功路径正确丢弃（目录空），最终答案正确。
3. **中断-保留 ✅**：step A 完成后 Esc 中断 → `Conversation interrupted` → checkpoint 文件保留（1 条 step A 结果）。
4. **resume 生效 ✅**：重触发后 step A 零 spawn 复用（checkpoint 中 step A 的值未被覆盖），直接续跑 step B。
5. **走查发现的真实缺陷（记录，不在 M2.5 修）**：
   - **D1**：被 Esc 中断的子代理线程槽位释放滞后，紧接的 resume run spawn step B 撞 `agent thread limit reached` → flow 失败（checkpoint 保留，容错正确，但用户体验差）。建议后续 M：interrupt 时同步等待子代理清理，或 resume 前回收 interrupted 槽位。
   - **D2**：被中断的 step B 子代理滞后完成，其结果迟到写入了 checkpoint（entries 出现 step B 键），与 resume 重新 spawn 的 step B 形成覆盖竞争。建议后续 M：中断后迟到的 agent 完成通知不再写 checkpoint（或写前校验 flow run 代际）。
6. resume 提示 UI：M2.1 未加显式 "resuming from checkpoint" 提示，resume 在 UI 上表现为"审批后直接开始后续步骤"（静默恢复）。如需显性提示，记入后续 M。

结论：M2.1–M2.5 全部落地，e2e 自动化 + 真实 TUI 走查通过；D1/D2 为走查新发现，建议排入后续里程碑。
