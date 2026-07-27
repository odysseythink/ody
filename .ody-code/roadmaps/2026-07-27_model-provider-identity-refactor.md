# Roadmap: 模型 / Provider 身份与运行时状态统一重构

> 创建 2026-07-27。最后更新 2026-07-27（Roadmap Architect 标注完成）。
> 目标读者:后续每个子阶段由一个**冷启动的大模型 agent**(上下文 ~200k)独立执行。
> 因此每个子阶段都写成**自包含**的:有明确目标、涉及文件、执行步骤、验收门槛(编译 + 指定测试)、回滚点。
> 执行者**只需读该子阶段列出的文件**即可完成,不必通读全仓库。

## Execution Rubric（执行标注总纲——本文件内统一适用）

### A. 拆分粒度原则

- 一个子阶段触及 **>8 个文件**时必须拆分，即使是机械替换——执行者在 ~200k 上下文中要保持全貌意识。
- 混合 **共享基础设施变更 + 叶子消费方工作** 的必须拆分，因两者的验证节奏不同（基础设施变更需全仓编译绿，叶子消费方可单包验证）。
- 每个子阶段必须**独立可编译**（`cargo build` 零错误）且有明确的测试验收标准。
- 上下文预算天花板：假设执行者 ~200k 窗口，一个子阶段需读的源文件 + 发出的 diff + 测试输出 + 推理链之和应远低于此数，预留足够余量。

### B. 模式决策准则

| 模式 | 适用场景 | 理由 |
| --- | --- | --- |
| `[normal]` | 机械替换，局部变更，单一正确解；不改签名/契约/架构 | normal 可直接编辑代码，无需 planning 开销 |
| `[plan]` | 多步骤实现有真依赖，共享签名/调用方扇出，或任何能从「先排任务列表再写代码」中受益的 | plan 模式禁止编辑除 plan 文件外的代码，强制先出依赖图 + TDD 任务清单 |
| `[design]` | 架构、数据模型、公共接口/契约、迁移语义存在真正未知——猜错会浪费大量返工 | design 模式硬性禁止实现直到用户审批 spec |

**打破平局**：存在真正未知 → 选更谨慎的模式。否则选更便宜的。不把常规工作升格。

---

## 全量依赖图（源级验证）

```
model-provider-info crate（最低层，所有上层 crate 依赖它）
│
A1 [normal] ProviderKind/ProviderRef/ModelRef 类型定义
├── A2 [normal] provider_kind() 方法 ───────────────────┐
├── A3 [normal] Config 规范访问器                        │
│                                                        │
B1 [normal] 状态栏 ModelName ──┐                        │
B2 [normal] picker 比较 ───────┤                        │
B3 [normal] 登录 config/emit ──┼── 全部并行，仅依赖 A1   │
B4 [normal] models-manager ────┤                        │
B5 [normal] review.rs ─────────┘                        │
│                                                        │
C1 [plan]  resolve_provider 集中 ─── 依赖 A1             │
├── C2 [normal] 会话侧替换 ──┐                           │
├── C3 [normal] TUI 侧替换  ─┤ 并行（不同文件）           │
│                            │                           │
C4 [normal] is_kimi 调用收敛 ─── 依赖 A2 ◄───────────────┘
│
D1 [plan]  resolve_runtime_model_state ── 依赖 A1, C1
├── D2 [normal] 冷启动改用 resolve ── 依赖 D1
│
D3 [plan]  reload 改用 resolve ── 依赖 D1, D2
│
E1 [design] 单一数据源决策 ── 依赖 D（统一 resolve 到位后）
├── E2 [normal] 清理冗余变更 ── 依赖 E1
│
F1 [normal] /login 集成测试（用纯数字别名 123456）── 依赖 B3, D3
F2 [normal] 状态栏快照测试 ── 依赖 B1（可提前到 B1 完成后立即做）
F3 [normal] ModelRef/点分路径 数字段单测 ── 依赖 A1（可最早做）
```

**关键并行机会：**
- A2 ∥ A3（同 crate 不同文件，编译隔离）
- B1 ∥ B2 ∥ B3 ∥ B4 ∥ B5（全部独立文件，无共享签名）
- C2 ∥ C3（不同 crate 的不同文件）
- F1 ∥ F2 ∥ F3（独立测试文件）
- F3 可在 A1 完成后即做；F2 可在 B1 完成后立即做，均不必等到 D/E

---

## 0. 背景:为什么反复坏

`/login` + provider 切换这块历史上多次"改好又坏"。根因不是单点 bug,而是四个结构性断层:

1. **模型身份没有规范表示,每层各自用字符串拼/拆。** 同一模型:
   - 配置 `default_model` = `alias/model`(如 `kimi_ranweiwei/kimi-for-coding`)
   - `StaticModelsManager` 的 `ModelInfo.slug` = **裸** `kimi-for-coding`(来自 `[models.*].model` 字段,见 `models-manager/src/model_info.rs:304`)
   - 状态栏 = `format!("{}/{}", model_provider_id, model)` 现拼(`tui/src/chatwidget/status_surfaces.rs:571`)
   - picker `is_current` = 拿 `preset.model`(裸 slug)跟 `current_model()` 比(`tui/src/chatwidget/model_popups.rs`)
   - `split_once('/')` 散落:`core/src/tasks/review.rs:214`、`tui/src/login/config.rs:177`、`models-manager/src/manager.rs:448`
   → 任一处改动都在赌其他所有地方的切分假设。

2. **provider "类型" vs "别名" 概念混用。** `ModelProviderInfo` 无类型字段,只有 `name`;类型靠启发式 `is_kimi()/is_deepseek()/is_glm()`(匹配 name/base_url,`model-provider-info/src/lib.rs:435+`)判断。别名 = `config.model_providers: HashMap<String, _>` 的 key + 松散的 `config.model_provider_id: String`(全仓 ~159 处读写)。解析时时而回退内置类型、时而需要别名在 map 里 → `kimi` vs `kimi_ranweiwei` 类 bug。

3. **启动快照散落多处,reload 只刷一部分。** `ThreadManagerState.models_manager`、`SessionServices.models_manager`、`config.model_providers`、`config.model_catalog`、`config.configured_model_catalog`、`config.model_provider_id` 均在会话启动时快照;历史上 `refresh_runtime_config`(`core/src/session/mod.rs`)只刷 config_layer + tool_suggest,manager 与 provider 集合都不刷 → "打地鼠"。(2026-07-27 已临时补:manager 热替换 + reload 拷贝 model_providers/catalog,但仍是分散的补丁。)

4. **实时更新路径 与 冷启动路径 是两套逻辑,会漂移。** "重启就正常"因为冷启动(config load)是完整权威路径;`/login` 走"事件 + 服务端 ThreadSettingsUpdated 回显"这条只复制子集的路径,两者对同一状态得出不同结果。

## 1. 目标与非目标

**目标**
- 引入**规范类型** `ProviderRef` / `ModelRef`,在边界解析一次,内部只传结构化值。
- 把 provider 的**类型(kind)**与**别名(alias)**拆成明确字段,消灭启发式判类型。
- 把所有"从 Config 派生的模型运行时状态"收敛到**单一 resolve 函数**,冷启动与 reload 都调它。
- 让实时更新与冷启动**同源**,消除回显漂移。
- 用集成/快照测试**锁死** `/login` 全流程,防回归。

**非目标(本轮不做)**
- 不改协议 wire 格式 / 存储格式(config.toml 键名、`default_model` 仍是 `alias/model`)。
- 不改 provider 的鉴权 / HTTP 逻辑。
- 不引入新依赖 crate(复用现有 `arc-swap` 等)。
- 不追求把全部 159 处 `model_provider_id` 改成新类型——只改**解析/拼接/判类型**站点,纯读取的保持 `String`。

## 2. 目标架构

### 2.1 新类型(放在低层 crate,零行为变更地引入)

```
// 建议落点:model-provider-info crate(最低层,core/tui/models-manager 都依赖它)
//
// provider 别名 + 已解析类型。alias 是 config.model_providers 的 key。
pub struct ProviderRef { pub alias: String, pub kind: ProviderKind }
pub enum ProviderKind { Kimi, Deepseek, Glm, Custom }

// 模型引用:provider 别名 + 裸 model id。
pub struct ModelRef { pub provider_alias: String, pub model_id: String }
impl ModelRef {
    pub fn parse(qualified: &str) -> ModelRef      // "alias/model" -> {alias, model}; 无 '/' 时 alias=""
    pub fn qualified(&self) -> String              // -> "alias/model"(alias 空时退化为 model)
    pub fn bare(&self) -> &str                     // -> model_id
    pub fn from_parts(alias: &str, model: &str) -> ModelRef
}
```

**关键不变量(执行者必须维持):**
- `default_model` / config 存储 / picker `preset.model` 三处对"裸 vs 完整"的期望必须一致。当前事实(2026-07-27 核实):
  - `StaticModelsManager` 暴露 `slug = 裸 model_id`,`provider = 别名`。
  - picker `preset.model = slug = 裸`,`preset.provider = 别名`。
  - 状态栏拼 `provider_id/model` → 期望 model 传**裸**、provider 传**别名**。
  - 会话内 `collaboration_mode.model()` 在正常态是**裸**(冷启动经 mask 解析),但 `config.model`(constructor.rs:31)存的是 default_model=**完整**。这个不一致是断层 #1 的核心,重构后由 `ModelRef` 统一。

### 2.2 单一 resolve 边界(消灭断层 #3/#4)

```
// 建议落点:core(如 core/src/config/runtime_model_state.rs 或 thread_manager.rs 内)
pub struct RuntimeModelState {
    pub models_manager: SharedModelsManager,
    pub providers: HashMap<String, ProviderRef>,  // 别名 -> 解析后的 provider
    pub model_catalog: Option<ModelsResponse>,
    pub active: Option<ModelRef>,                  // 当前默认/活动模型
}
```

- `resolve_runtime_model_state(config) -> RuntimeModelState`:冷启动与 reload 的唯一入口。
- `SessionServices.apply_runtime_model_state(state)`:一次性灌入 manager + providers + catalog,替代当前分散拷贝。

### 2.3 provider 类型解析集中化(消灭断层 #2)

- `ProviderKind` 在**构造 `ModelProviderInfo` 时解析一次**并随 `ProviderRef` 携带,不再运行时 `is_kimi()` 猜。
- 内置 fallback(`create_kimi_provider` 等)集中到一个 `resolve_provider(alias, config) -> ProviderRef` 函数,`SessionConfiguration::apply`(`core/src/session/session.rs:236`)和 TUI `sync_active_model_provider_config_from_provider_id`(`tui/src/chatwidget.rs:1900`)都调它,消除两份重复的 match。

## 3. 执行原则(每个子阶段都遵守)

1. **始终编译绿**:每个子阶段结束时 `cargo build` 全 workspace 0 error。
2. **行为不变除非显式声明**:Epic A/B/C 是"等价重构"(引入类型、替换拼接),不改可观察行为;只有 Epic D/E 显式改运行时收敛行为。
3. **既有失败基线**:core 有既有失败(guardian/fork 3、multi_agents 服务档位 6、models-manager 2、tui 快照 4、exec 29 需 KIMI_API_KEY)。判断回归时先 `git stash` 对比干净树,勿把既有失败算到自己头上。
4. **每子阶段独立可回滚**:一个 commit 一个子阶段。
5. **测试门槛**:每子阶段列出必须跑绿的具体测试;新增逻辑必须带单测。

---

## 4. 阶段拆分

> 依赖顺序:见文首全量依赖图。A1 是所有后续工作的前置；B1-B5 可全并行；C1 依赖 A1；D1 依赖 A1+C1；E1 依赖 D 全完成；F1 依赖 B3+D3，F2 依赖 B1。
> 每个子阶段标注**模式标签**和**预计涉及文件数**。

### Epic A — 基础类型(纯新增,零行为变更)

```
依赖图: A1 → [A2 ∥ A3]
全部 [normal]: 纯类型定义 + 访问器，无架构决策。
```

**[normal] A1. 定义 `ModelRef` / `ProviderRef` / `ProviderKind`**
> 理由: 纯类型定义，单一正确解。2-3 个文件，全在 model-provider-info crate 内。
- 落点:`model-provider-info/src/`(新增 `model_ref.rs`,在 `lib.rs` `pub mod` 导出)。
- 步骤:定义三个类型 + `ModelRef::{parse,qualified,bare,from_parts}` + `ProviderKind` 从 name/base_url 解析的 `fn resolve_kind(info: &ModelProviderInfo) -> ProviderKind`(内部复用现有 `is_kimi/is_deepseek/is_glm` 逻辑,先委托、后续 C 阶段反向替换)。
- 单测:`parse("a/b")`、`parse("b")`(alias 空)、`qualified()` 往返、含多段(`a/b/c`,按首个 `/` 切)。
- 涉及文件:2-3。**不改任何调用点。**
- 验收:`cargo test -p ody-model-provider-info` 绿。
- Depends on: _(无)_

**[normal] A2. `ModelProviderInfo` 携带解析后的 `kind`(可选缓存)**
> 理由: 方法委托，不改调用方。1-2 文件，机械重构。
- 落点:`model-provider-info/src/lib.rs`。
- 步骤:加 `pub fn provider_kind(&self) -> ProviderKind { resolve_kind(self) }`(先做成方法,不加存储字段,避免序列化改动)。`is_kimi()` 等改为 `self.provider_kind() == ProviderKind::Kimi`(保持对外行为一致)。
- 涉及文件:1-2。
- 验收:`cargo test -p ody-model-provider-info`(含既有 `is_kimi_matches_*` 测试)全绿。
- Depends on: A1

**[normal] A3. Config 暴露规范访问器**
> 理由: 简单访问器包装。1-2 文件，仅 core。
- 落点:`core/src/config/mod.rs`。
- 步骤:加 `pub fn active_model_ref(&self) -> Option<ModelRef>`,从已解析的 `self.model`(canonical,`default_model.or(model)`)`ModelRef::parse`。加 `pub fn provider_ref(&self, alias: &str) -> Option<ProviderRef>`。
- 单测:构造 Config with default_model → 断言 active_model_ref。
- 涉及文件:1-2。
- 验收:`cargo test -p ody-core --lib config::` 相关新增测试绿(注意既有失败基线)。
- Depends on: A1
- ∥ 可与 A2 并行

### Epic B — 收编模型字符串拼/拆(逐面机械替换)

```
依赖图: A1 → [B1 ∥ B2 ∥ B3 ∥ B4 ∥ B5]
全部 [normal]: 机械替换，每个子阶段 1-2 文件，独立可验证。
B1-B5 之间无共享文件，无相互依赖，可全并行执行。
```

**[normal] B1. 状态栏 ModelName**
> 理由: 单站点 format!() → ModelRef::qualified()，1-2 文件。
- 文件:`tui/src/chatwidget/status_surfaces.rs`(571 行附近)+ 可能 `settings.rs` 的 `model_display_name`。
- 步骤:把 `format!("{}/{}", model_provider_id, model_display_name())` 改为从 `ModelRef{provider_alias: model_provider_id, model_id: bare}` 构造 → `.qualified()`。**先确认 `model_display_name()` 返回裸值**;若当前会传入完整 slug(如登录态),用 `ModelRef::parse` 归一后再取 `bare()`,保证不再出现 `a/a/b` 三重前缀。
- 验收:`cargo test -p ody-tui --lib status` + 相关快照(必要时 `cargo insta review`)。
- Depends on: A1
- ∥ 与 B2-B5 并行

**[normal] B2. picker `is_current` / preset 比较**
> 理由: 字符串比较改为 ModelRef 语义比较，2 文件，机械。
- 文件:`tui/src/chatwidget/model_popups.rs`、`tui/src/chatwidget/settings.rs`(`find(|preset| preset.model == model)` 站点,295/313 行附近)。
- 步骤:比较改为 `ModelRef::parse(current).bare() == preset.model` 或统一到 `ModelRef`,消除"裸 vs 完整"歧义。
- 验收:`cargo test -p ody-tui --lib "model_popups\|popups_and_settings"`。
- Depends on: A1
- ∥ 与 B1,B3-B5 并行

**[normal] B3. 登录 config 编辑 + emit**
> 理由: 已确定的 emit 行为（裸 id + 别名 provider），用 ModelRef 固化。2 文件。
- 文件:`tui/src/login/config.rs`、`tui/src/app/config_persistence.rs`。
- 步骤:`build_login_models_edits` 用 `ModelRef::from_parts(alias, model).qualified()` 生成 `default_model` 键;`persist_login_provider` 的 emit 明确用 `ModelRef` 取 `bare()` 作 `UpdateModel`、`alias` 作 `UpdateModelProvider`(把 2026-07-27 的"发裸 id"决定固化为类型保证)。
- 验收:`cargo test -p ody-tui --lib login::config`(14 项)。
- Depends on: A1
- ∥ 与 B1-B2,B4-B5 并行

**[normal] B4. 模型 manager slug/preset 构造**
> 理由: 2 文件中 split_once → ModelRef 语义，机械。
- 文件:`models-manager/src/model_info.rs`(`to_model_info`,304 行)、`models-manager/src/manager.rs`(`find_model_by_namespaced_suffix` 448 行)。
- 步骤:slug 派生与命名空间后缀匹配改用 `ModelRef` 语义(裸 model_id 为 slug,alias 为 provider),去掉裸手写 `split_once`。
- 验收:`cargo test -p ody-models-manager`(注意既有 2 项失败基线)。
- Depends on: A1
- ∥ 与 B1-B3,B5 并行

**[normal] B5. 残余 split_once 站点 — review.rs**
> 理由: 单文件单站点，最简单的替换。
- 文件:`core/src/tasks/review.rs:214`。
- 步骤:改用 `ModelRef::parse` 解析 model 字符串，用结构化字段替代手动 split + tuple 解构。
- 验收:`cargo build -p ody-core`。
- Depends on: A1
- ∥ 与 B1-B4 并行

### Epic C — provider 类型/别名分离

```
依赖图: A1 → C1 [plan] → [C2 ∥ C3]
        A2 → C4 [normal]

C2 ∥ C3（不同 crate 的不同文件，无冲突）
C4 依赖 A2，可与 C1-C3 并行（它改的是 is_kimi() 调用方，独立于 resolve_provider）
```

**[plan] C1. 集中 `resolve_provider(alias, config) -> ProviderRef`**
> 理由: 多步骤提取——需统一 `session.rs:236` 和 `chatwidget.rs:1900` 两份重复的 provider 解析 fallback 到一处。涉及共享签名（返回 ProviderRef，两处调用方都要改签），受益于 TDD 任务清单。落点选择（model-provider-info vs core）也有小的设计判断。
- 落点:`model-provider-info` 或 `core/src/config/`。把内置 fallback(`create_kimi_provider/deepseek/glm`)与 `model_providers.get(alias)` 查找合并成一处。
- 涉及文件:2-3。
- 验收:新增单测(alias 命中 config、alias=内置类型名、未知 alias)。
- Depends on: A1

**[normal] C2. 替换会话侧 provider 解析**
> 理由: 单文件改调 C1 的 resolve_provider，删本地 match。
- 文件:`core/src/session/session.rs`(`apply`,236-260 行)。改调 `resolve_provider`,删本地 match。
- 验收:`cargo test -p ody-core --lib session::` 相关 + provider 切换测试。
- Depends on: C1
- ∥ 与 C3 并行

**[normal] C3. 替换 TUI 侧 provider 解析**
> 理由: 单文件改调 C1，删本地 match。
- 文件:`tui/src/chatwidget.rs`(`sync_active_model_provider_config_from_provider_id`,1900 行)。改调统一解析,删本地 kimi/deepseek/glm match。
- 验收:`cargo test -p ody-tui --lib chatwidget`。
- Depends on: C1
- ∥ 与 C2 并行

**[normal] C4. 收敛 `is_kimi/is_deepseek/is_glm` 调用**
> 理由: grep 确认外部调用仅 3 个文件（config_persistence.rs、slash_dispatch.rs、provider.rs），全部用 `provider_kind()` 直接比较替换 `is_kimi()` 调用。保留启发式实现在 `resolve_kind` 内部一处。≤4 文件，单批次可完成。
- 用 `provider_kind()` 替换散落调用(先 grep 全站点),保留启发式实现在 `resolve_kind` 内部一处。
- 涉及文件: tui/src/app/config_persistence.rs、tui/src/chatwidget/slash_dispatch.rs、model-provider/src/provider.rs、model-provider-info/src/model_provider_info_tests.rs（共 4 文件）。
- 验收:`cargo build` 全仓 + 相关包测试。
- Depends on: A2
- ∥ 可与 C1-C3 并行

### Epic D — 统一运行时状态 resolve(核心结构改动)

```
依赖图: A1 + C1 → D1 [plan] → D2 [normal] → D3 [plan]

串行链，每步依赖前一步。D 是风险最高阶段，严格按序执行。
```

**[plan] D1. 实现 `resolve_runtime_model_state(config) -> RuntimeModelState`**
> 理由: 新函数，需理解 `build_models_manager` + provider 解析 + catalog 的交互。多步骤实现，涉及核心数据结构 `RuntimeModelState`，有边界条件（无 config、有 Static models、有 Openai catalog）。受益于先写 test cases 再实现。
- 落点:`core/src/thread_manager.rs`(靠近 `build_models_manager`)或新文件。内部复用 `build_models_manager` + provider 解析 + catalog。
- 单测:无 config → 空 manager/空 providers;有 `[models.*]` → Static manager + providers 含别名 + active=ModelRef。
- 涉及文件:2-3。
- 验收:`cargo test -p ody-core --lib thread_manager` 相关新增。
- Depends on: A1, C1

**[normal] D2. 冷启动改用 resolve**
> 理由: 调用 D1 函数替换 `build_models_manager` 调用点，1-2 文件，机械连线。
- 文件:`core/src/thread_manager.rs`(`ThreadManager::new`,291 行)、bootstrap 路径。用 `resolve_runtime_model_state` 初始化 `SwappableModelsManager`。
- 验收:`cargo build -p ody-core` + `-p ody-app-server`;app-server 现有 model/list 测试绿。
- Depends on: D1

**[plan] D3. reload 改用 resolve,删散装补丁**
> 理由: 触及热路径（session reload），需协调 `config_processor.rs` 和 `session/mod.rs` 两处改动，同时**删除**临时补丁代码（2026-07-27 手加的拷贝）。多文件、有删改、需逐会话验证，适合 plan 模式先排任务。
- 文件:`app-server/src/request_processors/config_processor.rs`(`reload_user_config`)、`core/src/session/mod.rs`(`refresh_runtime_config`)。
- 步骤:reload 时 `let s = resolve_runtime_model_state(&next_config);` → `thread_manager.set_models_manager(s.models_manager)` + 每个会话 `session.apply_runtime_model_state(s)`(新方法:store manager + 灌 providers/catalog 进会话 config)。**删掉** `refresh_runtime_config` 里 2026-07-27 手加的 `config.model_providers/model_catalog/configured_model_catalog` 拷贝(逻辑并入 `apply_runtime_model_state`)。
- 验收:`cargo test -p ody-core --lib refresh_runtime_config`(2 项)+ `cargo build -p ody-app-server`。
- Depends on: D1, D2

### Epic E — 实时更新与冷启动同源

```
依赖图: D → E1 [design] → E2 [normal]

串行。E1 必须先出设计决策（二选一），E2 是决策后的机械清理。
```

**[design] E1. 明确 model/provider 显示的单一数据源**
> 理由: 真正的架构未知——两个方案 (a) 本地不写只等回显 vs (b) 回显时 ModelRef 合并，各有不同的并发正确性 tradeoff。选错会导致竞态行为反复。需要先写设计文档明确选择及理由，审批后再写代码。
- 文件:`tui/src/chatwidget/settings.rs`(`apply_thread_settings`,475 行)、`tui/src/app/thread_settings.rs`、`tui/src/chatwidget.rs`。
- 目标:确定"服务端 ThreadSettingsUpdated 回显为权威,本地 set 仅乐观更新",消除 UpdateModel 回显携带 stale provider_id 覆盖本地的竞态(2026-07-27 观察到的过渡态)。做法二选一,在子阶段内先写决策再改:
  - (a) 本地不直接写 `config.model_provider_id`,只等回显;或
  - (b) 回显时用 `ModelRef` 合并而非整体覆盖。
- 验收:设计文档审批通过 + 新增/更新 chatwidget 设置同步测试。
- Depends on: D（统一 resolve 到位后，数据流已清晰，再做同步策略决策）

**[normal] E2. 去除与回显竞争的冗余本地变更**
> 理由: 依 E1 决策清理，机械。
- 依 E1 决策清理 `sync_active_model_provider_config_from_provider_id` 等本地写点。
- 验收:`cargo test -p ody-tui --lib chatwidget`。
- Depends on: E1

### Epic F — 回归锁死

```
依赖图: B3 + D3 → F1 [normal]
        B1      → F2 [normal]
        A1      → F3 [normal]
∥ 三者并行，独立测试文件。
F2 可在 B1 完成后立即做；F1 需等到 D3 完成才能覆盖完整 reload 流程；F3 只依赖 A1,可最早开始。
```

**[normal] F1. mid-session `/login` 集成测试**
> 理由: 场景明确（空配置→登录→model/list→二次登录→provider解析），测试编写无未知。
- 落点:`app-server/tests/` 或 `tui` 集成测试。覆盖:空配置启动 → `/login` → `model/list` 非空 → 默认模型 = `alias/model` → 第二次登录不覆盖默认 → provider 解析成功。
- **强制用例数据:第一个 provider 的别名用纯数字 `123456`**(第二个可用普通别名)。这条锁死"点分路径段一路当字符串键"这个隐式不变量——纯数字别名的安全性完全依赖它,但当前无测试覆盖。断言:`providers.123456` / `models."123456/<model>"` / `default_model = "123456/<model>"` 正确 round-trip,状态栏/`is_current` 用 `123456/<model>` 单前缀显示,provider 类型仍解析为 `kimi`(读存储的 `type`,不受数字别名影响)。
- 验收:新测试绿。
- Depends on: B3, D3

**[normal] F2. 状态栏/header 快照(别名 provider)**
> 理由: insta 快照测试，机械。
- 落点:`tui` 快照测试。断言别名 provider 下状态栏为单前缀 `alias/model`,无三重前缀。
- 验收:`cargo insta` 新快照。
- Depends on: B1
- ∥ 与 F1 并行
- ⚡ 可提前：B1 完成后即可开始，不必等 D/E

**[normal] F3. `ModelRef` / 点分路径 数字段单测**
> 理由: 纯单测,无未知;把 F1 里数字别名的核心不变量下沉到最低层,快且不依赖服务端。
- 落点:`model-provider-info`(`ModelRef` 单测,紧邻 A1)+ 可选 `config`(`parse_key_path` / override 层)。
- 覆盖:
  - `ModelRef::parse("123456/kimi-for-coding")` → `{alias:"123456", model:"kimi-for-coding"}`;`qualified()` 往返得回原串。
  - `parse("999")`(无 `/`,纯数字)→ alias 空、model=`999`(与非数字裸 model 行为一致)。
  - 点分路径 `providers.123456.type` 写入后读回,`123456` 是**字符串表键**而非整数/数组下标(防止将来有人给 `parse_key_path`/override 层加"数字段当索引"的优化而悄悄破坏)。
- 验收:新单测绿。
- Depends on: A1(仅路径断言部分不依赖 A1,可更早)
- ⚡ 可最早开始:A1 一落地即可写,是 F1 数字别名不变量的下沉版,成本最低

---

## 5. 里程碑与取舍

- **A + B + C** 完成即消灭断层 #1/#2(反复复发的"字符串错位""kimi vs kimi_ranweiwei")——**性价比最高,建议至少做到这里。**
  - A1-A3 完成后即可开始 B1-B5（全并行）。
  - C1 依赖 A1，C1 完成后 C2/C3 可并行。
  - C4 依赖 A2，可与 C1-C3 并行推进。
- **D + E** 消灭断层 #3/#4(reload 打地鼠、回显漂移)——结构收益大但改动最深,可作为第二批。
  - D1-D3 严格串行（每步依赖前一步）。
  - E1 必须走 design 模式，先出文档审批。
- **F** 任何时候都应尽早补,给后续改动兜底。
  - F3 可在 A1 完成后立即跑（最早）；F2 可在 B1 完成后立即跑；均不需要等到 D/E。
  - F1 强制用纯数字别名 `123456` 作为用例数据,锁死"点分路径段一路当字符串键"这个隐式不变量。

## 6. 风险与回滚

- 每子阶段一个 commit,`git revert` 粒度到子阶段。
- Epic D 风险最高(碰会话/线程状态热路径):务必先 `git stash` 建立既有失败基线,逐会话验证 provider 切换 + turn 内模型解析。
- `ModelRef` 落点在 `model-provider-info`(最低层),若引发循环依赖,退而放在独立新 crate `model-ref` 或 `protocol`。
- **并行风险**: B1-B5 全并行时注意避免合并冲突——虽然它们改不同文件，但都新增 `use ody_model_provider_info::ModelRef` 导入。建议并行执行者在各自子阶段完成后立即 commit，合并时按文件列表检查无冲突。

## 7. 执行者速查:关键坐标(2026-07-27 核实)

| 关注点 | 文件:行 |
|---|---|
| 状态栏 `provider/model` 拼接 | `tui/src/chatwidget/status_surfaces.rs:571` |
| StaticManager slug=裸 model | `models-manager/src/model_info.rs:304` |
| manager 类型选择(Static/Openai) | `model-provider/src/provider.rs:248` |
| picker preset 比较 | `tui/src/chatwidget/model_popups.rs`(~295/313) |
| 登录 config 编辑 | `tui/src/login/config.rs:74,177` |
| 登录后 emit | `tui/src/app/config_persistence.rs`(persist_login_provider) |
| 会话 provider 解析(内置 fallback) | `core/src/session/session.rs:236-260` |
| TUI provider 解析(内置 fallback) | `tui/src/chatwidget.rs:1900` |
| provider 类型启发式 | `model-provider-info/src/lib.rs:435+`(is_kimi 等) |
| reload 入口 | `app-server/src/request_processors/config_processor.rs`(reload_user_config) |
| 会话 reload 合并 | `core/src/session/mod.rs`(refresh_runtime_config) |
| models_manager 热替换持有器 | `core/src/thread_manager.rs`(SwappableModelsManager) |
| ModelProviderInfo 定义 | `model-provider-info/src/lib.rs:233` |
| ModelPreset/ModelInfo 定义 | `protocol/src/model_metadata.rs:199,451` |

## 8. 已完成的前置修复(本重构的起点,勿重复)

见 `.ody-code/reports/2026-07-26_empty-model-until-login_plan.md` 的"补充修复三/四":
- models_manager 已改为 `SwappableModelsManager` 热替换(D 阶段在此基础上收敛)。
- `refresh_runtime_config` 已临时拷贝 model_providers/catalog(D3 要把它并入统一 resolve 并删除)。
- 登录 emit 已改为裸 model_id + 别名 provider(B3 要用类型固化)。

## 9. 执行顺序建议（执行者可直接照此排程）

```
第 1 轮（并行批）:
  A1 → 完成即释放后续
  ├── A2 ∥ A3（并行）

第 2 轮（并行批）:
  所有依赖 A1 的就绪
  ├── B1 ∥ B2 ∥ B3 ∥ B4 ∥ B5（5 路并行）
  ├── C1（单路，为 C2/C3 做前置）
  ├── C4（需要 A2，可与上面并行）
  └── F3（A1 已完成即可写，最早的回归测试）

第 3 轮:
  C1 完成后 → C2 ∥ C3（并行）

第 4 轮（串行链）:
  D1 → D2 → D3（严格串行）

第 5 轮:
  E1 [design] → E2

第 6 轮（并行批）:
  F1 ∥ F2（并行；F3 若未提前做则一并跑）
  F3 实际上 A1 完成后就能做，可提前插入第 2 轮
  F2 实际上 B1 完成后就能做，可提前插入第 2 轮之后
```
