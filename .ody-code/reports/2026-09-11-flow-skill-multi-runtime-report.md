# Flow 技能（狭义多步工作流）多运行时实现方案

日期：2026-09-11
状态：设计提案（待评审）
范围：`ody-core-skills` / `ody-skills-extension`（ext/skills）/ `ody-code-mode` / `ody-cli`

---

## 1. 背景

### 1.1 概念界定

“技能的多步工作流”有两层含义：

- **广义**：把可复用的多步骤流程写成 SKILL.md 指令，由模型按需加载并逐步编排工具执行。ody 已通过 Inline/Prompt 技能支持。
- **狭义**：`SkillType::Flow` —— 一个独立的技能类型，由确定性运行时（而非模型的逐轮判断）执行编排计划。本报告只讨论狭义。

### 1.2 现状：Flow 是“死技能”

代码证据：

| 位置 | 事实 |
|---|---|
| `core-skills/src/model.rs:23` | `SkillType::Flow` 注释为 “Multi-step workflow skill. **Schema-only** in T3.1.0.” |
| `core-skills/src/loader.rs:1246` | frontmatter `type: flow` 可解析，但仅登记元数据 |
| `ext/skills/src/catalog.rs:174` | Flow 被设为 `prompt_visible = false` |
| `ext/skills/src/catalog.rs:213-219` | `is_model_invocable` 仅放行 `Inline \| Prompt`；注释：“Knowledge and Flow skills are triggered through other mechanisms”——而这些机制尚不存在 |

结论：注册为 Flow 的技能没有任何激活路径，必须保持 Inline。

### 1.3 关键约束：V8 不是必选项

`ody-code-mode` 的 V8 运行时位于默认关闭的 `v8` Cargo feature 之后（`ody-code-mode/v8 ← ody-core/v8 ← ody-cli/v8`，发布构建需显式 `cargo build -p ody-cli --release --features v8`）。**若 Flow 唯一运行时建立在 V8 上，默认构建中 Flow 依然无法执行**，等于把“死技能”问题从触发机制挪到了编译参数。

### 1.4 已有资产

- 子代理原语：`core/src/tools/handlers/multi_agents/`（spawn / wait / send_input / close_agent）。
- 无 V8 的嵌入式脚本先例：workspace 已依赖 `starlark = "0.14.2"`（`Cargo.toml:378`），`execpolicy` / `execpolicy-legacy` 用它做沙箱策略求值。
- serde / YAML 解析链全线已有。
- 持久化相关 crates：`thread-store` / `rollout` / `state`。

---

## 2. 总体设计

### 2.1 三种 Flow 载体，三个运行时，全部实现

| 方案 | 载体文件 | 运行时 | Cargo feature | 默认 | 表达力 |
|---|---|---|---|---|---|
| **A** | `flow.yaml` | 纯 Rust 声明式状态机（serde 解析） | `flow-yaml` | 开 | 中：步骤、依赖序、pipeline/parallel fan-out、prompt 模板插值、结构化输出 |
| **B** | `flow.star` | Starlark 求值器（复用 execpolicy 依赖） | `flow-starlark` | 开 | 中高：循环、条件、步骤间数据变换 |
| **C** | `workflow.js` | `ody-code-mode` 的 V8 | `flow-v8`（蕴含 `v8`） | 关 | 最高；兼容 Claude Code dynamic workflows 脚本形态 |

### 2.2 统一抽象

```rust
/// Flow 执行引擎的统一接口；每种载体一个实现。
pub trait FlowRuntime: Send + Sync {
    /// 该运行时支持的载体扩展名（flow.yaml / flow.star / workflow.js）
    fn supported_artifact(&self) -> &'static str;
    /// 静态校验（schema/语法），错误在进入运行时前暴露
    fn validate(&self, source: &str) -> Result<FlowPlan, FlowError>;
    /// 执行：宿主函数（agent/pipeline/parallel/phase/log）由引擎注入，
    /// 底层统一映射到 multi_agents 原语
    async fn run(&self, plan: FlowPlan, ctx: FlowContext) -> Result<FlowOutcome, FlowError>;
}
```

宿主函数语义三种实现保持一致：

- `agent(prompt, { schema?, label? })` → `multi_agents::spawn` + `wait`；带 `schema` 时做 JSON 校验并重试；
- `pipeline(items, fn)` / `parallel(tasks)` → 并发调度，单批上限 4096（与 Claude Code 对齐）；
- `phase(title)` / `log(msg)` → 进度事件上报；
- `args` → 触发时传入的输入。

### 2.3 编译期选择与运行时降级

- feature 决定**注册哪些 runtime**；一个构建可带 1~3 个。
- 解析 Flow 技能时按载体扩展名选择 runtime；请求的运行时未编译时，返回与 `CodeModeService` 一致风格的明确错误：“compiled without the `flow-v8` feature”。
- 默认构建（A+B）覆盖全部核心场景；C 只服务于需要兼容 Claude Code 脚本生态的用户。

---

## 3. 各实现要点

### A. `flow.yaml` 声明式 DSL（`flow-yaml`，默认开）

```yaml
name: game-create
description: 从概念到可运行原型的多步骤游戏创作流程
phases:
  - id: design
    steps:
      - agent: 根据主题生成 GDD 大纲
        output: gdd
  - id: implement
    steps:
      - pipeline: ${{ gdd.mechanics }}
        each: |
          实现机制 ${item.name}，遵循 ${gdd.constraints}
        output: implementations
  - id: verify
    steps:
      - parallel:
          - agent: 审查 ${{ implementations }} 的边界情况
          - agent: 运行测试并修复失败
        output: review
```

- 零新依赖；确定性由构造保证（无脚本可执行）。
- 限制：不支持复杂条件分支与数据聚合；超出表达力时引导用户改用 B/C。

### B. `flow.star` Starlark 运行时（`flow-starlark`，默认开）

- 复用 workspace 已有 `starlark = "0.14.2"` 与 execpolicy 集成经验。
- Starlark 语言层面禁随机性/时间戳，契合可重放要求；`Date.now()` 类问题在语言层就不存在（C 方案需要在引擎层显式禁用）。
- **开放问题（需 spike 验证）**：starlark-rust 0.14 对挂起式宿主函数（async `agent()`）的集成方式尚未核实，动手前先做最小验证原型。

### C. `workflow.js` V8 运行时（`flow-v8`，默认关）

- 直接复用 `ody-code-mode` V8；脚本形态对齐 Claude Code dynamic workflows：`export const meta = { name, description }` + 顶层 await + `agent()/pipeline()/parallel()/phase()`。
- 确定性：引擎层使 `Date.now()` / `Math.random()` / 无参 `new Date()` 抛错（Claude Code 同款做法），保证中断重放一致。
- 无 `v8` feature 的构建：skills loader 检测到 `workflow.js` 载体但 runtime 未注册时，给出明确编译期错误提示。

---

## 4. 触发机制（Flow 从“死技能”变活的另一半）

1. **用户侧**：Flow 技能注册 slash 命令 `/flow-name`（复用 Inline 的 slash 路径）；`/flows` 列出运行中/已完成的 run。
2. **模型侧**：ext/skills 新增 `flow__run(name, args)` 模型工具（参照现有 `skills__list` / `skills__read` 模式）。catalog 需把“对用户可见（slash 列表）”与“对模型可见”拆成两个维度，Flow 默认对模型可见。
3. **审批**：运行前展示脚本/plan 摘要，一次性 guardian 审批（参照 Claude Code 的 “Allow workflows”）。

## 5. 执行、恢复与观测

- **checkpoint/replay**：每个 `agent()` 结果作为 checkpoint 落盘（`thread-store` / `rollout`）；中断后重跑 plan，已完成的调用直接返回缓存。这是 Temporal durable execution 与 LangGraph checkpointer 的精简版。
- **进度**：`phase()` / `log()` 经 app event 总线上报，TUI 提供按 phase 分组的进度视图（参照 Claude Code `/workflows`）。
- **提示缓存**：fan-out 时相同前缀的 agent 共享缓存；可选实现 stagger hold（`CLAUDE_CODE_WORKFLOW_PREFIX_STAGGER_MS` 同款）。

## 6. 分期计划

| 阶段 | 内容 | 验收 |
|---|---|---|
| M1 | A 方案（flow.yaml）+ slash 触发 + phase/log 进度上报 + multi_agents 映射 | 默认构建可运行顺序 + fan-out Flow；`cargo nextest run -p ody-core-skills -p ody-skills-extension` |
| M2 | checkpoint/replay 恢复；`flow__run` 模型工具；catalog 可见性拆分；运行前审批 | 中断后重跑不重跑已完成 agent；模型可触发 Flow |
| M3 | B 方案（Starlark，前置 spike）+ C 方案（flow-v8）+ 降级报错 | 三载体在对应 feature 组合下均可执行；无 v8 构建对 workflow.js 报明确错误 |
| M4 | 结构化输出 schema 校验、并发上限、提示缓存优化、TUI `/flows` 视图完善 | 与 Claude Code dynamic workflows 行为对齐项 checklist |

## 7. 风险

- **Starlark async 能力未验证**（见 §3B）——M3 前置 spike，失败则 B 降级为“仅同步数据变换脚本”，agent 调用仍走 YAML 声明。（已消解：2026-09-12 spike 三项全过，M3.1 交付。）
- **三 runtime 语义漂移**——宿主函数语义必须以 trait 文档为单一事实源，三实现共享同一套集成测试套件（同一 plan 三种载体行为一致）。
- **feature 组合测试矩阵膨胀**——CI 需覆盖 {yaml} × {starlark} × {v8} 的关键组合，不跑全笛卡尔积。

---

## External Evidence

- Status: Required
- Reason: 问题依赖行业实践、开源先例与版本敏感事实（Claude Code dynamic workflows 的设计、LangGraph/Temporal 的能力、Agent Skills 规范边界），属于技术选型论证，必须外部核实。

| Claim | Source | Version/date | Decision impact |
|---|---|---|---|
| Claude Code dynamic workflows 采用“JS 脚本 + 运行时编排子代理”模型：`agent()/pipeline()/parallel()/phase()`、`meta` 块、结构化输出 schema、可恢复、脚本内禁 `Date.now()`/`Math.random()`、slash 触发、运行前审批 | https://code.claude.com/docs/en/workflows | 2026 年现行文档（`/workflow-authoring` 需 v2.1.248+） | 作为 C 方案蓝本与整体宿主函数语义来源 |
| Agent Skills 开放规范只定义 SKILL.md 声明式技能（frontmatter + 渐进披露），不含流程运行时 | https://agentskills.io/specification | 现行规范 | Flow 是 ody 的扩展设计空间，无标准约束；A 方案的 YAML 载体为自选格式 |
| LangGraph 是开源（MIT）agent 编排运行时：确定性节点与 agentic 节点混合、durable execution、HITL interrupt、persistence | https://docs.langchain.com/oss/python/langgraph/overview | 现行文档 | 论证“checkpoint/replay”为业界标准能力，ody 采用其精简版而非完整图引擎 |
| Temporal 为 AI 应用提供 durable execution：崩溃/超时/等待人工审批后自动恢复，Event History 重放，含 agent cookbook | https://docs.temporal.io/ai | 现行文档 | 借鉴恢复语义；完整引入判定为过度设计（本地 CLI 场景） |
