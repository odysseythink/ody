# M3 执行计划：Starlark / V8 双运行时 + 三载体分派 + 降级报错

日期：2026-09-12
上位文档：`2026-09-11-flow-skill-multi-runtime-report.md`（§6 M3 范围）
状态：spike 已完成，待评审锁定
Last Updated: 2026-09-12

---

## Execution Rubric

复用 M1/M2 已批准的同一套 rubric（拆分粒度三触发器、~256k 执行上下文预算、normal/plan/design 三档判定与平票规则），不重新立法。

### 前置 spike 结论（2026-09-12，/tmp/m3-spike，starlark 0.14.2 独立原型）

**B 方案全量形态可行，不降级。** 三个关键问题全部验证通过：

1. **异步 agent 桥**：starlark eval 跑在独立 OS 线程；原生 `agent()` 经 `eval.extra`（`ProvidesStaticType`）拿桥，发送请求后 `oneshot::blocking_recv` 阻塞；tokio 侧 driver 任务执行真实 async host 调用后回送结果。串行多步 + args 插值（f-string）全通。
2. **pipeline 真并发**：函数是一等值，`eval_function` 可在原生函数内回调顶层函数。`pipeline(items, each)` 先同步渲染全部 prompt 再一次性发出，driver 侧并发 spawn——原型实测 3 个 item 的 spawn 时间差 <150ms（host 延迟 200ms），结果按 item 序绑定。
3. **abort 语义**：drop driver → `tx.send` 失败 / 应答 channel 关闭 → 原生函数报错 → eval 立即退出，eval 线程秒级回收。

spike 中踩平的 API 坑（0.14.2）：`starlark::Error` 不实现 `std::error::Error`（`?` 进 anyhow 需 `map_err`）；`Heap<'v>` 是 `&StarlarkHeap` 引用类型按值传递；`Module::with_temp_heap` 作用域受限（返回值须在闭包内转成自有数据）；`UnpackList.items` 字段遍历。

---

## 已锁定的 M3 架构决策

1. **Plan 类型泛化**：`FlowRuntime::validate` 今天返回 yaml AST（`FlowPlan`）。M3 引入
   `FlowPlanSource` 枚举：`Yaml(FlowPlan)` / `Starlark(String)` / `V8(String)`（后两者
   validate 阶段做语法校验、运行期重新解析，源码即 plan）。`FlowRuntime::run` 签名
   改收该枚举。运行时按载体扩展名分派：`select_flow_runtime(artifact)` 返回编译期
   enum `AnyFlowRuntime{Yaml,Starlark,V8}`（cfg feature 装配，呼应 mod.rs 既有
   NOTE 与父报告 §2.3「enum 而非 dyn」）。
2. **降级报错为触发期**：`workflow.js` 技能在**无 `flow-v8` 构建中可正常加载**（不进
   死技能），触发执行时返回明确错误 `compiled without the `flow-v8` feature`（风格
   对齐 `CodeModeService` stub 错误）。理由：加载期硬失败会把整个 skills 目录拖垮，
   一个载体缺失不应影响其他技能。
3. **loader 泛化**：`core-skills/src/loader.rs` 的 `FLOW_ARTIFACT_FILENAME = "flow.yaml"`
   硬编码改为三选一发现（`flow.yaml`/`flow.star`/`workflow.js`，恰好一个，多于一个报
   错）。eager validate 只对 yaml 保留（`parse_flow_plan` 已在 core-skills）；
   starlark 语法校验下放触发期 `runtime.validate`（core-skills **不引** starlark 重
   依赖）。`flow_artifact: Option<AbsolutePathBuf>` 字段不变。
4. **Starlark 桥接形态**（spike 形态直接产品化，`core/src/flow/star.rs`，feature
   `flow-starlark` 默认开）：eval 线程 + `std::sync::mpsc` 请求 + oneshot 应答；
   driver 为 tokio task（`spawn_blocking` 收请求），取消安全：run future 被 drop →
   driver 中止 → eval 线程经 send 失败即刻退出。宿主函数：`agent(str)->str`、
   `pipeline(items, each_fn)`、`parallel(fns)`（each/fns 为顶层函数、**纯 prompt
   渲染**，文档明示其内调 agent 只会串行）、`phase(str)`/`log(str)`。checkpoint
   在 driver 处理 agent 请求处做（read→miss→run→write，键 = prompt SHA-256，
   与 M2.1 yaml 路径一致）。
5. **新增 `FlowProgress::Log { message }`**：script 载体无结构化 phase/step，
   phase()/log() 都映射到它；TUI/事件渲染端补臂（编译器穷尽匹配强制）。
6. **结果契约**：script 顶层 `result` 全局变量 →
   `FlowOutcome.outputs = {"result": <json>}`（spike 已验证 `to_json_value` 转换链）。
7. **C 方案不复用 cell-actor `CodeModeService`**：其 session/cell 生命周期与一次性
   确定性脚本不匹配，且禁 `Date.now()` 需自定义 global。改为 `ody-code-mode` 新增
   `workflow` 入口（`v8` feature 后）：`run_workflow_script(source, &dyn WorkflowHost)`；
   `WorkflowHost`（async RPITIT：`run_agent(prompt)->Result<String,String>` +
   `report_progress(msg)`）定义在 code-mode，core 侧实现映射 `FlowAgentHost`；
   checkpoint 也在 core 侧 bridge 实现内做。JS 侧 `agent()` 返回 Promise（同桥接
   模式：Rust 回调启动 tokio 任务、消息泵驱动 microtask 直至 settle）；引擎层删除/
   替换 `Date.now`、`Math.random`、无参 `new Date`。core feature `flow-v8 = ["v8"]`
   （默认关），`ody-cli` 转发。脚本形态对齐父报告 §3C：`export const meta` + 顶层
   await + 宿主函数全局注入。
8. **审批摘要泛化**：`FlowPlanSummary` 拆 `for_yaml(name, &FlowPlan)`（现结构化摘要）
   与 `for_script(name, kind, source)`（载体名 + 源码前 40 行预览）；
   `GuardianApprovalRequest::FlowRun` 结构不变。
9. **Conformance 套件**（父报告 §7 语义漂移防护）：同一份"计划"用三种载体各写一遍，
   共享同一 mock host，断言 spawn 序列 + 输出 + 进度事件完全一致。yaml/star 在默认
   feature 跑，v8 用 `#[cfg(feature = "flow-v8")]` 门控。

---

## 子阶段拆分

### M3.1 ✅ 载体分派 + Starlark 运行时 — `core/src/flow/` + `core-skills/src/loader.rs`

**实施记录（2026-09-13）**

- 决策 1/3/4/5/6 全部落地 + conformance 的 yaml×star 部分：
  - `core/Cargo.toml`：`default = ["flow-starlark"]`，`flow-starlark = ["dep:starlark"]`，`flow-v8 = ["v8"]`（M3.2 用）；`starlark = { workspace = true, optional = true }`。
  - `core/src/flow/mod.rs`：`FlowPlanSource` 枚举（Yaml/Starlark/V8，后两者 cfg）；`FlowRuntime` trait 泛化（`validate -> FlowPlanSource`、`run(FlowPlanSource, ...)`）；`AnyFlowRuntime` 编译期 enum + `FlowRuntime` 委托 impl；`select_flow_runtime` 按扩展名分派，未编译载体报 "compiled without the `flow-x` Cargo feature"（触发期，决策 2）；`FlowProgress::Log { message }`。
  - `core/src/flow/star.rs`（新，~470 行）：spike 形态产品化——eval 独立 OS 线程 + `std::sync::mpsc` 请求 + oneshot 应答；driver 在 `run()` future 内以 `tokio::select!` 交错请求接收与 `FuturesUnordered` 在飞 agent（借用 `host`，非 'static JoinSet；drop future = 关闭全部 reply channel = eval 线程秒级退出）；checkpoint 在 driver 的 agent 处理中（read→miss→run→write，与 M2.1 yaml 语义一致）；宿主函数 `agent/pipeline/parallel/phase/log`；`FLOW_BATCH_LIMIT` 在 fan-out 前硬检查；结果契约 = 模块全局 `result` → `outputs["result"]`。
  - **`flow_json` 自托管命名空间（对计划的偏差，原因已查明）**：starlark 0.14.2 的 `LibraryExtension::Json` 位于私有模块 `starlark::stdlib`，外部 crate 无法命名其类型（`GlobalsBuilder::extended_by` 参数不可构造）。改为在 flow runtime 内用 `#[starlark_module]` 实现 `json.decode`/`json.encode`，经 `GlobalsBuilder::namespace("json", flow_json)` 注册，底层复用本文件的 serde 双向桥。
  - `core-skills/src/loader.rs`：三选一发现（`flow.yaml`/`flow.star`/`workflow.js`，读取探测存在性——`ExecutorFileSystem` 无 `exists`；多于一个报错）；eager parse 校验只对 yaml 保留。
  - `core/src/flow/plan_summary.rs`：`FlowPlanSummary` 增加 `carrier` + `source_preview`（前 40 行）字段，`for_plan` 按载体分派（yaml 结构化 / script 源码预览）——**比计划提前**（原列 M3.3），因为 M3.1 的 starlark 触发就需要审批摘要。
  - `protocol/src/protocol.rs`：`FlowLogEvent` + `EventMsg::FlowLog` + `From` impl；下游臂 rollout-trace（2）、rollout policy（1）、core turn.rs（`realtime_text_for_event` 映射为实时文本）、mcp-server（1）。**app-server schema 无需再生成**（flow 进度事件自 M1.4 起就不在 schema 导出集合内，已核实无变化）。
- 测试（15 个新）：starlark 段 9（顺序+args 插值、json.decode、pipeline 序绑定、parallel、agent 失败传播、batch limit、checkpoint replay、unset result、artifact/validate 元数据）+ conformance 1（同计划 yaml×star：排序 prompt 多重集相等 + `result == outputs`）+ dispatch 2（yaml 选择 + 未知载体；无 flow-v8 时 workflow.js 报错）+ loader 3（flow.star / workflow.js 发现、多载体报错）。flow 套件 83/83；core-skills 143/143。
- 回归核实：`guardian` 套件 5 个失败 + app-server `dynamic_tool_call_round_trip` 均 **pre-existing 环境敏感**（其中之一经 git stash baseline 复现证实；其余为同类 review-model/prewarm harness，改动面零重叠）。
- 踩坑记录：① 同文件连续 `edit_file` 多次调用只保留最后一个 hunk（此前 M2 已遇，本次在 protocol.rs/star.rs/flow_tests.rs 反复踩中）——同文件多 hunk 必须合并为单次 edit 或用 apply_patch；② starlark `starlark::Error` 不进 anyhow 的 `?`；③ `tokio::task::JoinSet::spawn` 要求 'static，借用 host 的并发任务须用 `FuturesUnordered`；④ eval 线程 panic 载荷是 `Box<dyn Any>`，与正常 `Err(String)` 是 join 结果的两层不同错误。

### M3.2 [plan] V8 运行时（flow-v8）— `code-mode/src/workflow.rs` + `core/src/flow/v8.rs`

- 内容：决策 7 + conformance 的 v8 部分 + 决策 2 的 v8 臂（无 feature 报错测试走
  默认构建即可断言）。V8 消息泵/TLA/确定性禁用在 code-mode 内自含测试
  （`#[cfg(feature = "v8")]`）。
- 与 M3.1 串行（共享 `FlowPlanSource`/`WorkflowHost` 接线点），不并行。
- 验收：`cargo nextest run -p ody-core --features flow-v8 -E 'test(flow::)'`；
  默认构建下 `cargo nextest run -p ody-core -E 'test(flow::)'`（降级报错测试）；
  `cargo test -p ody-code-mode --features v8`。

### M3.3 [normal] 审批摘要泛化 + 文档收尾

- 内容：决策 8；`docs/` 或 AGENTS.md 的 flow 载体说明更新；父报告 M3 行打勾；
  真实 TUI 走查 starlark 载体（可选，视时间）。
- 验收：flow + guardian 套件全绿；`cargo check --workspace --all-targets`。

---

## 风险与对策

- **starlark eval 线程泄漏**（abort 后线程未退）：driver drop → 请求 channel 断开 →
  原生函数报错退出（spike T3 已验证秒级回收）；测试里加"abort 后 join 成功"断言。
- **pipeline each 内调 agent 的串行陷阱**：文档明示 + runtime 文档注释；不在 M3
  做多 eval 交错（真需求进 M4 再评估）。
- **V8 TLA/消息泵复杂度**：M3.2 开工先写最小 smoke（注入 agent 全局 + 顶层 await +
  确定性禁令），失败则收窄 C 范围为「无 TLA 的同步脚本 + async agent Promise」
  （agent/pipeline 仍可用，顶层 await 不支持），并在本文件记录。
- **feature 矩阵**：不跑全笛卡尔积（父报告 §7）；CI 关键组合 = 默认（yaml+star）
  与 `--features flow-v8` 两套。
