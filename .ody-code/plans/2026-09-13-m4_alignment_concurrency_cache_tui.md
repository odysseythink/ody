# M4 实施计划：Claude Code 行为对齐（schema 跨载体 / 并发上限 / 提示缓存 / `/flows` 视图）

**日期：** 2026-09-13
**父报告：** `.ody-code/reports/2026-09-11-flow-skill-multi-runtime-report.md` §6 M4 行
**状态：** 已完成（2026-09-13，T01–T04 全部交付并验证）
**Last Updated:** 2026-09-13

## Goal

落地 M4 四项「与 Claude Code dynamic workflows 行为对齐」项：

1. **结构化输出 schema 校验**——从 yaml 单载体（M2.4）扩展到 starlark / v8 两脚本载体
2. **并发上限**——run 内并发 agent 数上限 + run 级 agent 总数上限
3. **提示缓存优化**——fan-out stagger hold（首个 agent response 开始后释放同批其余 agent）
4. **TUI `/flows` 视图**——运行中/已完成 flow run 列表 + 进度展示

## 对齐基准（Claude Code dynamic workflows 现行文档，2026-09-13 核实）

来源：`https://code.claude.com/docs/en/workflows.md`（`How a workflow runs` / `Behavior and limits` / `Watch the run` / `Prompt caching in a fan-out` 四节全文）。

| 对齐项 | Claude Code 行为 | ody 现状 | M4 处置 |
|---|---|---|---|
| 单批 4096 上限 | 硬错误拒绝 | ✅ 已有 `FLOW_BATCH_LIMIT=4096`（M1） | 不动 |
| 并发上限 | 16 并发（CPU 受限时更少） | ❌ fan-out 全量并发 | M4.2：host 层 semaphore=16 |
| run 总 agent 数 | 1,000 / run 硬上限 | ❌ 无 | M4.2：host 层 spawn 计数 |
| fan-out stagger | 同批其余 agent hold 到首 agent response 开始，cap 默认 5000ms（env `CLAUDE_CODE_WORKFLOW_PREFIX_STAGGER_MS`，0 禁用） | ❌ 无 | M4.3：host 层 `PrefixStagger` |
| 结构化输出 schema | `agent(prompt, {schema})`，校验失败重试 | yaml ✅（M2.4）；star/v8 ❌ 仅 prompt | M4.1：两脚本载体加 schema 选项，复用 M2.4 校验+重试 |
| `/workflows` 视图 | phase 分组（agent 数/token/耗时）、逐 agent 钻取、暂停/停止/重启、`Large workflow` 警告 | ❌ TUI 无任何 flow 视图 | M4.4：`/flows` 只读视图（见范围裁定） |
| 脚本无 fs/shell、无 mid-run 输入 | 硬约束 | ✅ 三载体引擎层均无此能力 | 不动 |

## 关键裁定（决策记录）

**决策 A（M4.4 范围）：`/flows` 做只读视图，不做控制键。** Claude 完整版含 `p` 暂停 / `x` 停止 agent / `r` 重启 / `s` 存脚本。ody 的 flow run 是 in-turn 执行（trigger.rs），无 run 级暂停/单 agent 重启的执行面；强行对齐需要新增执行控制通路，超出 M4 合理范围。M4.4 = run 列表 + yaml 结构化 phase 进度 + 脚本载体 log 行展示 + Esc 返回。token 总计/耗时/逐 agent 状态需扩展协议事件面（FlowLogEvent 仅 call_id/flow_name/message），留待后续里程碑。

**决策 B（M4.2 实现层）：并发控制放 host 层，三载体零改动。** `SessionFlowAgentHost` 是 yaml runtime、star driver、v8 `FlowWorkflowHost` 的共同落点（spawn 都经它）。semaphore(16) + spawn 计数(1000) 放 host 内新的 `RunConcurrency` 结构，三载体自动继承，无需改任何 driver。checkpoint replay 命中不 spawn，不占 permit/计数（与 Claude「completed agents return saved results」语义一致）。

**决策 C（M4.3 信号源）：stagger 释放信号 = 首个 agent 首次非初始状态。** Claude 的 "response has begun" 在 ody 可观测等价物是 `wait_for_final_status` 首次观察到 agent 离开 `PendingInit`。`PrefixStagger` 结构：首个 spawn 记录时间并立即放行；窗口内后续 spawn 等待 notify（首 agent 状态变化触发）或剩余窗口超时；窗口外直接放行。配置 env `ODY_FLOW_PREFIX_STAGGER_MS`（默认 5000，0 禁用），host 构造时读一次。

**决策 D（M4.1 签名）：`WorkflowHost::run_agent` 加 `schema: Option<String>` 透传参数。** code-mode 不理解 schema 内容（JSON 字符串透传），core 侧解析为 Map 后走与 yaml 完全相同的 `run_one_agent`（schema prompt 注入 + bounded retry，runtime.rs:312-357）。checkpoint 键 = prompt SHA-256，schema 已注入 prompt 尾部，天然入键，M2.1 checkpoint 语义不变。starlark API：`agent(prompt, schema=None)`（dict）；v8 API：`agent(prompt, {schema})`，与 Claude Code 形态一致。

## File Structure

| 文件 | 责任 |
|---|---|
| `core/src/flow/concurrency.rs`（新建，~150 行含测试） | `RunConcurrency`（semaphore + spawn 计数）+ `PrefixStagger`（窗口/notify/超时）+ 单测 |
| `core/src/flow/host.rs`（修改） | `SessionFlowAgentHost` 持 `RunConcurrency` + `PrefixStagger`；spawn 接入 acquire/计数/stagger，wait 接入 permit 释放 + 首响应信号 |
| `core/src/flow/mod.rs`（修改） | `mod concurrency;`、并发常量文档、`FLOW_CONCURRENCY_LIMIT=16` / `FLOW_MAX_AGENTS_PER_RUN=1000` |
| `core/src/flow/runtime.rs`（修改） | `run_one_agent` 改 `pub(crate)`（M4.1 共享） |
| `core/src/flow/star.rs`（修改） | `agent(prompt, schema=None)` 命名参数；schema dict → JSON → `run_one_agent` |
| `core/src/flow/v8.rs`（修改） | `FlowWorkflowHost::run_agent(prompt, schema_json)` → 解析 → `run_one_agent` |
| `code-mode/src/runtime/workflow.rs`（修改） | `WorkflowHost::run_agent` 签名 + `schema` 选项解析（第二参数对象） |
| `core/src/flow/flow_tests.rs`（修改） | M4 段：schema 跨载体测试、并发/stagger 结构测试 |
| `tui/src/slash_command.rs` + 新增 `tui/src/flows_view.rs`（修改/新建） | `/flows` 命令注册 + 只读视图 |
| `tui/src/chatwidget/…`（修改） | flow 事件订阅 + run 注册表维护 |
| `AGENTS.md`（修改） | M4 对齐项记录 |

## Dependency Overview

```
T01 并发上限（concurrency.rs RunConcurrency + host 接线）
T02 stagger hold（同文件 PrefixStagger + host 接线）— 依赖 T01（同文件同 host，顺路）
T03 schema 跨载体（runtime.rs 可见性 + star.rs + v8.rs + code-mode 签名）— 独立于 T01/T02
T04 /flows TUI 视图 — 独立（只依赖已交付事件通路）
```

T01→T02 顺序执行（共享 host.rs 改动区）；T03 可在 T02 后执行（避免 host.rs 冲突）；T04 最后。

## Implementation record（2026-09-13 收尾补记）

| Task | Commit | 交付与验证 |
|---|---|---|
| T01+T02 | `02147dc9` | `RunConcurrency`（semaphore 16 + spawn 计数 1000）+ `PrefixStagger`（env `ODY_FLOW_PREFIX_STAGGER_MS` 默认 5000，0 禁用）于 `core/src/flow/concurrency.rs`；host.rs spawn 接线（acquire→permit 存 inflight map，wait 时 remove 释放；abort-aware hold），wait 入口 `signal_response_began`。8 单测 + flow 套件通过 |
| T03 | `a64c237f` | `runtime.rs` 提取 `pub(crate) run_agent_text`（schema 指令注入移入内部，yaml 侧预注入删除，retry/checkpoint 单一事实源）；starlark `agent(prompt, schema={..})`；v8 `agent(prompt, {schema})`（`WorkflowHost::run_agent` 加 `schema: Option<String>`，code-mode `MockWorkflowHost` 加 `schemas` 记录）；`FlowWorkflowHost`/star driver 解析 transport JSON 走 `run_agent_text`。双 feature 85/85、code-mode 12/12（后全量 59/59）、默认 97/97 |
| T04 | `08b9aa82` | `ThreadItem::FlowLog`（app-server-protocol）+ `EventMsg::FlowLog` 接入 v2（原被 app-server `_ => {}` 丢弃）；TUI `chatwidget/flows.rs`（`FlowRuns` BTreeMap 注册表 + 5 单测，只读视图，决策 A）；slash 命令注册 + dispatch + replay + agent_status_feed + thread_transcript + analytics reducer 穷尽 match。flows 6/6、slash 191/191、analytics 85/85 |

收尾全量验证：`cargo nextest run -p ody-core -E 'test(flow_tests::)'` 默认构建 75/75、`--no-default-features --features flow-starlark,flow-v8` 85/85、`cargo nextest run -p ody-code-mode --features v8` 59/59、`cargo check --workspace --all-targets` 0 错误（仅既有 warning）。

实施期偏差记录：T03 第一版 `run_agent_text` 漏移 schema prompt 注入（原在 yaml FlowStep 分支），被 `js_agent_schema_option_validates_and_retries_with_feedback` 抓出后修正——注入现已完全内聚于 `run_agent_text`。

已核实 pre-existing 基线噪声（stash 基线重跑证实，非 M4 引入）：`ody-app-server-protocol export::tests::generated_ts_optional_nullable_fields_only_in_params`、`ody-app-server` dynamic_tools 组若干图像 content item 断言、guardian 套件 2 失败。

## Spec coverage

| Requirement | Task | Status |
|---|---|---|
| M4 行：结构化输出 schema 校验 | T03 | covered |
| M4 行：并发上限 | T01 | covered |
| M4 行：提示缓存优化 | T02 | covered（stagger hold；provider cache_control 注入不存在于 ody provider 层，经调查无此能力面，stagger 为运行时层可达的对齐子集） |
| M4 行：TUI `/flows` 视图完善 | T04 | covered（只读范围，决策 A） |
| Claude 对齐：4096 批上限 / 1000 run 上限 / 16 并发 / stagger 5s | T01, T02 | covered（4096 已预先对齐） |

## Risks & Open Questions

| # | Risk | Mitigation |
|---|---|---|
| R1 | `WorkflowHost` 签名变更波及其他调用方 | workspace 内 grep 全量调用方（core v8.rs + code-mode 测试），双构建 typecheck 收尾 |
| R2 | stagger 与 abort 交互：hold 中的 spawn 被 abort | hold 用 `tokio::select!` 挂 abort 通道（host 已有 abort 通路），abort 时放行退出 |
| R3 | TUI flow 事件订阅引入新协议面 | 不改协议（FlowLogEvent/FlowPhaseBegin/End 为 M1/M3 已交付事件），TUI 纯消费 |
| R4 | `/flows` 视图在无 flow 会话的空列表体验 | 空态文案「No flow runs in this session」 |
| R5 | 16 并发在 CI/低核环境拖慢 fan-out e2e 测试 | 现有 e2e fan-out 规模小（<16）；常量文档注明 CPU 受限可降 |

## Out-of-scope

| 名称 | 处置 |
|---|---|
| provider 层 cache_control 注入 | ody provider 层无此能力面（已调查）；stagger 为运行时层可达子集 |
| `/flows` 控制键（暂停/停止/重启）、token/耗时统计、逐 agent 钻取 | 需扩展协议事件面 + 执行控制通路，后续里程碑 |
| `Large workflow` 警告（>25 agents / >1.5M token 投影） | token 统计面无，后续随控制键一起做 |
| size guideline（small/medium/large） | Claude 为生成期建议非运行时约束；ody flow 计划由用户/模型写就，无对应生成面 |
| 脚本载体 fs/shell 禁用 | 三载体构造上无此能力（starlark 无 fs 内置、v8 引擎删 Date/random 且无 fs shim），已对齐 |

## External Evidence

- Status: Required
- Reason: M4 定义为「与 Claude Code dynamic workflows 行为对齐项 checklist」，对齐基准是版本敏感外部事实，必须核实现行文档。

| Claim | Source | Version/date | Decision impact |
|---|---|---|---|
| 16 并发上限、1000 agent/run、stagger 默认 5000ms（`CLAUDE_CODE_WORKFLOW_PREFIX_STAGGER_MS`）、4096 批上限 | https://code.claude.com/docs/en/workflows.md `Behavior and limits` / `Prompt caching in a fan-out` | 2026-09-13 核实 | M4.2/M4.3 数值与语义直接来源 |
| `/workflows` 视图完整控制键集（p/x/r/s）与 phase 统计面 | 同上 `Watch the run` 节 | 2026-09-13 核实 | M4.4 范围裁定（决策 A）依据 |
| resume 语义（completed 返缓存、失败后序重跑、nothing to resume 报错） | 同上 `Resume after a pause` 节 | 2026-09-13 核实 | 记录为已对齐/既有：ody M2.1 checkpoint 语义同构；「失败后序全部重跑」ody 语义为仅失败步重试（M2.4 bounded retry），差异记录在案不追平 |
