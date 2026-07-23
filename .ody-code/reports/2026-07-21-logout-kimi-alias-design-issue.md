# `/logout` 无法删除 `kimi`：问题分析与设计改进

**日期**: 2026-07-21  
**状态**: 已定位根因，待修复  
**相关模块**: `tui`, `core/src/config`, `model-provider-info`, `ody-config`

---

## 1. 问题现象

在 TUI 中执行 `/logout` → 选择 Kimi → 选择 `kimi` 删除，提示：

```text
Logged out of kimi alias 'kimi'.
```

但下次进入 `/logout` 菜单时，`kimi` 仍然出现在可删除账号列表中，无论删除多少次都无法消失。

菜单显示如下：

```text
Select Kimi account to log out
Choose an account to remove

› 1. kimi_ranweiwei    Remove kimi account 'kimi_ranweiwei'
  2. kimi_xi           Remove kimi account 'kimi_xi'
  3. managed:ody-code  Remove kimi account 'managed:ody-code'
  4. kimi              Remove kimi account 'kimi'
```

---

## 2. 关键证据

### 2.1 用户配置文件中没有 `kimi` 这个 alias

读取 `C:\Users\hkb819\.ody-code\config.toml`，其中包含：

```toml
[providers.kimi_ranweiwei]
type = "kimi"
...

[providers.kimi_xi]
type = "kimi"
...

[providers."managed:ody-code"]
type = "kimi"
...
```

没有 `[providers.kimi]`，也没有 `[model_providers.kimi]`。

### 2.2 登录流程明确禁止把 `kimi` 当作用户 alias

`tui/src/login/validation.rs:22-38`：

```rust
pub(crate) fn validate_custom_alias(alias: &str) -> Result<(), String> {
    ...
    for reserved in [
        LoginProvider::Kimi.id(),      // "kimi"
        LoginProvider::Deepseek.id(),  // "deepseek"
        LoginProvider::Glm.id(),       // "glm"
    ] {
        if trimmed.eq_ignore_ascii_case(reserved) {
            return Err(format!("'{trimmed}' is a reserved provider alias"));
        }
    }
    ...
}
```

测试也确认：

```rust
assert!(validate_custom_alias("kimi").is_err());
assert!(validate_custom_alias("DEEPSEEK").is_err());
assert!(validate_custom_alias("glm").is_err());
```

这说明系统设计层面已经知道 `kimi`/`deepseek`/`glm` 是**保留的 provider type ID**，不是用户可创建的 alias。

### 2.3 `/logout` 菜单读取的是合并后的 provider map

`tui/src/chatwidget/slash_dispatch.rs:1269-1280`：

```rust
fn configured_aliases_for_provider(&self, provider: LoginProvider) -> Vec<String> {
    self.config
        .model_providers  // ← 合并了内置 + 用户配置的 map
        .iter()
        .filter(|(_, p)| match provider {
            LoginProvider::Kimi => p.is_kimi(),
            LoginProvider::Deepseek => p.is_deepseek(),
            LoginProvider::Glm => p.is_glm(),
        })
        .map(|(alias, _)| alias.clone())
        .collect()
}
```

### 2.4 合并后的 map 始终包含内置 `kimi`

`model-provider-info/src/lib.rs:479-493`：

```rust
pub fn built_in_model_providers() -> HashMap<String, ModelProviderInfo> {
    [
        (KIMI_PROVIDER_ID, create_kimi_provider()),
        (DEEPSEEK_PROVIDER_ID, create_deepseek_provider()),
        (GLM_PROVIDER_ID, create_glm_provider()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}
```

`KIMI_PROVIDER_ID` 定义为 `"kimi"`（`model-provider-info/src/lib.rs:40`）。

`core/src/config/mod.rs:3580-3582`：

```rust
let model_providers =
    merge_configured_model_providers(built_in_model_providers(), configured_model_providers)
        .map_err(...)?;
```

### 2.5 删除逻辑只能清除用户配置的 provider

`tui/src/login/config.rs:74-112`：

```rust
pub(crate) fn build_logout_provider_edits(
    aliases_to_remove: &[String],
    configured_models: &HashMap<String, OdyCodeModelConfig>,
    default_model: Option<&str>,
) -> Vec<ConfigEdit> {
    ...
    for alias in aliases_to_remove {
        edits.push(clear_config_value(format!("providers.{alias}")));
        ...
    }
    ...
}
```

它只能生成 `clear_config_value("providers.kimi")` 来删除用户配置，但 `kimi` 是内置的，不在用户配置中，所以删除操作无效。

### 2.6 登出 guard 也基于合并后的 map

`tui/src/app/config_persistence.rs:848-865`：

```rust
let is_matching_alias = self
    .config
    .model_providers
    .get(&alias)
    .map(|p| match provider { ... })
    .unwrap_or(false);
```

因为合并后的 map 包含内置 `kimi`，所以 guard 认为 `kimi` 是合法的、可删除的 alias。

---

## 3. 根因分析

问题的本质不是配置没删干净，而是**概念模型不统一**：

1. **登录流程**知道 `kimi`/`deepseek`/`glm` 是保留的 provider type ID，不能作为用户 alias。
2. **登出流程**却把它们和用户自定义 alias（如 `kimi_ranweiwei`）混在一个 map 里，全部当成可删除账号展示。
3. 删除逻辑只能作用于用户配置，对内置 provider 无效。

因此，用户看到的“可删除 `kimi` 账号”是一个**没有实际意义的操作项**，点击后会执行一次无效的清除，然后刷新时内置 `kimi` 再次出现。

更深一层看，`Config.model_providers` 的 key 同时承载两种语义：

| key 来源 | 语义 | 是否可删除 |
|---|---|---|
| `built_in_model_providers()` | provider type ID（如 `kimi`） | 否 |
| `cfg.normalized_providers()` | 用户自定义 alias（如 `kimi_ranweiwei`） | 是 |

把这两个不同语义的数据结构放进同一个 map，是当前设计问题的根源。

---

## 4. 短期修复（最小改动）

目标：让 `/logout` 只展示用户实际创建的 alias，内置 provider type ID 不再出现在可删除列表中。
目标：让 `/logout` 只展示用户实际创建的 alias，内置 provider type ID 不再出现在可删除列表中。

### 4.1 在 `core` 的 `Config` 中记录用户配置的 alias 集合

在 `core/src/config/mod.rs` 的 `Config` 结构体新增：

```rust
/// Provider aliases explicitly configured by the user, excluding built-in providers.
pub user_configured_provider_aliases: HashSet<String>,
```

在 `load_config_with_layer_stack` 中初始化：

```rust
let configured_model_providers = cfg.normalized_providers();
let user_configured_provider_aliases: HashSet<String> =
    configured_model_providers.keys().cloned().collect();
```

### 4.2 在 TUI 中同步该字段

`tui/src/chatwidget.rs` 的 `sync_provider_config`：

```rust
pub(crate) fn sync_provider_config(&mut self, config: &Config) {
    self.config.model_providers = config.model_providers.clone();
    self.config.configured_models = config.configured_models.clone();
    self.config.model = config.model.clone();
    self.config.user_configured_provider_aliases =
        config.user_configured_provider_aliases.clone();
}
```

### 4.3 修改 `/logout` 菜单数据来源

`tui/src/chatwidget/slash_dispatch.rs` 的 `configured_aliases_for_provider`：

```rust
fn configured_aliases_for_provider(&self, provider: LoginProvider) -> Vec<String> {
    self.config
        .user_configured_provider_aliases
        .iter()
        .filter(|alias| {
            self.config
                .model_providers
                .get(*alias)
                .map(|p| match provider {
                    LoginProvider::Kimi => p.is_kimi(),
                    LoginProvider::Deepseek => p.is_deepseek(),
                    LoginProvider::Glm => p.is_glm(),
                })
                .unwrap_or(false)
        })
        .cloned()
        .collect()
}
```

### 4.4 修改删除 guard

`tui/src/app/config_persistence.rs` 的 `logout_provider_alias`：

```rust
let is_matching_alias = self.config.user_configured_provider_aliases.contains(&alias)
    && self
        .config
        .model_providers
        .get(&alias)
        .map(|p| match provider { ... })
        .unwrap_or(false);
```

如果 `alias` 不在用户配置集合中，直接拒绝并提示“该 alias 不是用户配置，无法删除”。

### 4.5 补充测试

- 测试 `configured_aliases_for_provider` 不返回内置 `kimi`/`deepseek`/`glm`。
- 测试 `logout_provider_alias` 对内置 `kimi` 返回错误或不生成任何编辑。

---

## 5. 长期设计改进

短期修复是在现有架构上打补丁。长期应该把 **provider type catalog** 和 **user provider aliases** 在数据模型上彻底分开。

本章按 `roadmap-architect` 规则拆分为可独立执行、可测试的子 phase。所有子 phase 共享下面的执行标准（rubric），避免每轮重新判断。

### Execution Rubric

#### A. 拆分粒度原则

1. 子 phase 不应跨越 >~8 个源文件/模块；如果调用扇出超过此范围，必须再拆。
2. 共享基础设施（新字段、新模块、新公共函数）与叶子使用点必须分离：先做基础设施，再迁移调用点。
3. 每个子 phase 必须独立可测试，即可以单测/集成测验证其自身行为。
4. 执行足迹（需要阅读的源码 + 生成的 diff + 测试输出）必须 fit 在 ~256k token 工作窗口内，避免执行中被迫 compaction 导致静默丢需求。

#### B. 执行模式判定标准

| 模式 | 适用场景 | 原因 |
|---|---|---|
| `normal` | 机械、低风险、答案唯一；孤立改动，无共享签名或架构决策 | 可直接编辑代码，无需前置计划 |
| `plan` | 多步骤实现，有真实依赖、共享签名/调用扇出，或需要按 TDD 列出任务清单 | 强制先写计划文件和依赖图，禁止直接改代码 |
| `design` | 架构、数据模型、公共接口或迁移语义存在真正未知；猜错会浪费大量返工 | 用户批准设计前禁止实现 |

**Tie-break**：若可同时归为两种模式，优先更谨慎的模式；但不要把常规工作人为升级为 `design`。

> **Last Updated**: 2026-07-23 — 将第 5 章长期改进拆分为可执行子 phase。

### 5.0 数据模型最终方案决策 `[design]`

**目标**：在方案 A（彻底拆分）与方案 B（温和过渡）之间做出有约束的最终选择，并输出设计契约。

**交付物**：
- 确定 `Config` 字段名及类型（如 `built_in_providers` / `user_configured_aliases` / `effective_providers`，或保留 `model_providers` + 新增 `configured_provider_aliases`）。
- 明确三个数据源的语义边界（见下表）。
- 向后兼容策略：旧配置如何迁移、旧字段是否保留别名/废弃周期。
- 新增/重命名字段的 serde 序列化约定。

**依赖**：无（本章首个 phase）。
**阻塞后续**：5.1、5.2、5.4、5.5.x。
**理由**：数据模型和公共接口是后续所有迁移的契约，未确定前实现任何迁移都是高返工风险。

| 场景 | 应使用的来源 | 原因 |
|---|---|---|
| `/login` 选择 provider 类型 | `built_in_providers` | 用户选择要登录哪种 provider 后端 |
| 输入自定义 alias 校验冲突 | `user_configured_aliases` + 保留 ID 表 | alias 必须唯一且不能是保留 ID |
| `/logout` 展示可删除账号 | `user_configured_aliases` | 只能删除用户自己创建的账号 |
| 发送模型请求 | `effective_providers` | 合并后的最终运行时配置 |
| 模型选择器 / 默认模型解析 | `effective_providers` | 展示所有可用 provider |

### 5.1 保留 ID 单一事实来源 `[plan]`

**目标**：把目前 `tui/src/login/validation.rs` 单独维护的保留 ID 列表提升到 `model-provider-info` 或 `ody-config`，供登录、登出、配置校验共同使用。

**涉及代码**：
- `model-provider-info/src/lib.rs` — 新增 `RESERVED_PROVIDER_IDS` / `is_reserved_provider_alias()` / `reserved_provider_ids()`。
- `tui/src/login/validation.rs` — 删除本地列表，调用 `model-provider-info` 的公共函数。
- `tui/src/app/config_persistence.rs` — `logout_provider_alias` guard 可复用该函数拒绝保留 ID（如果 5.3 还未完成）。

**依赖**：5.0（需确认保留 ID 集合是否就是 built-in provider IDs，还是包含未来扩展）。
**可并行于**：5.2。
**测试要求**：
- `validate_custom_alias("kimi")` / `"DEEPSEEK"` / `"glm"` 仍失败。
- `is_reserved_provider_alias("kimi_ranweiwei")` 返回 false。
- 新增 `model-provider-info` 单测覆盖大小写不敏感。

**理由**：涉及公共 API 变更和两个 crate（`ody-tui`、`ody-model-provider-info`），需要 plan 模式制定调用点清单和测试计划。

### 5.2 在 `Config` 中暴露 `user_configured_provider_aliases` `[plan]`

**目标**：在 `core/src/config/mod.rs` 的 `Config` 结构体中新增 `user_configured_provider_aliases: HashSet<String>` 字段，从 TOML `providers` 表派生，保持现有 `model_providers` 不变（方案 B 的过渡步骤）。

**涉及代码**：
- `core/src/config/mod.rs` — `Config` 结构体新增字段；在 `Config::load`/`build` 流程中从 `cfg.normalized_providers()` 或原始 TOML 提取用户配置的 alias 集合。
- 可能需要 `config/src/config_toml.rs` 配合暴露原始 `providers` 表。

**依赖**：5.0（字段名和类型由设计契约决定）。
**可并行于**：5.1。
**阻塞后续**：5.3。
**测试要求**：
- 当用户配置 `providers.kimi_ranweiwei`、`providers.kimi_xi` 时，集合仅包含这两个 alias，不含内置 `kimi`。
- 无用户 provider 时集合为空。

**理由**：这是共享基础设施改动，改变 `Config` 公共结构，且需要决定从哪一层 TOML 派生，适合 plan 模式。

### 5.3 TUI 登出流程切换到用户 alias 源 `[plan]`

**目标**：让 `/logout` 只展示和删除用户配置的 alias，不再把内置 provider type ID 当成可删除账号。

**涉及代码**：
- `tui/src/chatwidget/slash_dispatch.rs` — `configured_aliases_for_provider()` 改为遍历 `self.config.user_configured_provider_aliases`，再回查 `effective_providers` 确认类型。
- `tui/src/app/config_persistence.rs` — `logout_provider_alias()` guard 检查 alias 是否在 `user_configured_provider_aliases` 中。
- `tui/src/login/config.rs` — `build_logout_provider_edits()` 保持不变（它只生成 TOML 编辑，前提是输入已过滤）。

**依赖**：5.2（需要 `Config.user_configured_provider_aliases`）。
**可并行于**：无（必须等 5.2）。
**测试要求**：
- `configured_aliases_for_provider(LoginProvider::Kimi)` 不返回 `"kimi"`。
- `logout_provider_alias(Kimi, "kimi")` 返回错误或不生成编辑。
- 删除 `kimi_ranweiwei` 后确实从配置中消失。

**理由**：多文件协作，有明确行为契约，需要 plan 模式列任务清单和集成测试。

### 5.4 重构 `Config.model_providers` 语义 `[design]`

**目标**：按 5.0 选定的方案执行数据模型重构。

#### 方案 A（彻底拆分）
- 将 `Config.model_providers` 拆为：
  - `built_in_providers: HashMap<String, ProviderTypeInfo>`
  - `user_configured_aliases: HashMap<String, ProviderAliasConfig>`
  - `effective_providers: HashMap<String, ModelProviderInfo>`
- `effective_providers` 在 `Config::load` 中由前两者合并生成。

#### 方案 B（温和过渡）
- 保留 `model_providers` 作为运行时合并结果（即 `effective_providers` 的语义）。
- 新增 `configured_provider_aliases: HashSet<String>`（同 5.2 字段，可能改名）用于 `/logout` 和账号管理。
- 逐步将其他场景引导到正确来源，而非一次性重命名。

**涉及代码**：
- `core/src/config/mod.rs` — 结构体、构造函数、合并逻辑。
- `model-provider-info/src/lib.rs` — 可能需要新增 `ProviderTypeInfo` 类型。
- `ody-config`/`config/src/config_toml.rs` — 反序列化层是否需要感知拆分。

**依赖**：5.0。
**阻塞后续**：5.5.0–5.5.5。
**理由**：这是本章最核心的架构决策落地，字段命名和类型影响全仓库 >100 处调用点，必须 design 模式输出契约。

### 5.5 迁移 `model_providers` 调用点

`model_providers` 当前在仓库中有 100+ 处使用，必须按使用场景分组迁移，否则单 phase 会超出上下文窗口。

#### 5.5.0 共享迁移基础设施 `[plan]`

**目标**：为 5.5.x 叶子 phase 提供辅助函数/适配器，避免每个叶子 phase 重新推导访问模式。

**交付物**：
- 若方案 A：`Config` 上提供 `effective_model_provider(alias)`、`built_in_provider_types()`、`configured_aliases()` 等 helper。
- 若方案 B：明确 `model_providers` 的语义仅为 `effective_providers`，并提供 `is_configured_alias()` helper。
- 在 `core/src/config/mod.rs` 或 `model-provider-info/src/lib.rs` 中落地。

**依赖**：5.4。
**阻塞后续**：5.5.1–5.5.5。
**理由**：共享 helper 是后续并行迁移的前提，先定义可减少重复决策和编译错误。

#### 5.5.1 迁移 Core 运行时调用点 `[plan]`

**目标**：让 `core` 中发送模型请求、默认模型解析、personality migration 等代码使用 `effective_providers`（或保持 `model_providers` 但语义明确）。

**涉及代码**（估计）：
- `core/src/config/mod.rs`
- `core/src/realtime_context.rs`
- `core/src/personality_migration.rs`
- `core/src/tasks/review.rs`
- 相关测试辅助代码。

**依赖**：5.4、5.5.0。
**可并行于**：5.5.2–5.5.5。
**理由**：这些调用点主要使用运行时合并结果，迁移路径一致，但涉及多个文件和测试，需要 plan。

#### 5.5.2 迁移 App-server 配置管理调用点 `[plan]`

**目标**：让 `app-server` 中读取/校验 `model_providers` 的代码使用正确来源。

**涉及代码**（估计）：
- `app-server/src/config_manager_service.rs` 及测试
- `app-server/src/request_processors/thread_processor.rs`
- `app-server/tests/common/config.rs`
- 大量 `app-server/tests/suite/v2/*` 测试用例

**依赖**：5.4、5.5.0。
**可并行于**：5.5.1、5.5.3–5.5.5。
**理由**：测试文件众多，独立成 phase 防止上下文爆炸；需要 plan 模式列出调用点清单。

#### 5.5.3 迁移 TUI 登录/模型选择调用点 `[plan]`

**目标**：让 `/login` 选择 provider 类型时使用 `built_in_providers`，让模型选择器使用 `effective_providers`。

**涉及代码**（估计）：
- `tui/src/login/*`
- `tui/src/chatwidget.rs` 的 provider 配置同步
- `tui/src/chatwidget/slash_dispatch.rs` 中除登出外的其他使用点

**依赖**：5.4、5.5.0。
**可并行于**：5.5.1、5.5.2、5.5.4–5.5.5。
**注意**：5.3 已完成登出菜单迁移，本 phase 不再重复。

#### 5.5.4 迁移 MCP / 工具 / 其他 leaf 调用点 `[plan]`

**目标**：清理 `exec`、`tools`、`mcp-server`、`code-mode` 等 crate 中对 `model_providers` 的使用。

**涉及代码**（估计）：
- `exec/src/lib.rs`
- `tools/src/*`（如有）
- `mcp-server/*`
- `code-mode/*`

**依赖**：5.4、5.5.0。
**可并行于**：5.5.1–5.5.3、5.5.5。
**理由**：leaf crate 通常只读 effective provider，迁移较机械，但范围广，需要 plan 模式防止遗漏。

#### 5.5.5 迁移测试公共辅助代码 `[plan]`

**目标**：统一更新 `core/tests/common`、`app-server/tests/common`、`tui` 测试等公共 helper，避免下游测试因字段变更集体失败。

**涉及代码**（估计）：
- `core/tests/common/test_ody.rs`
- `core/tests/common/test_ody_exec.rs`
- `app-server/tests/common/*`
- 其他 crate 的测试辅助

**依赖**：5.4、5.5.0。
**可并行于**：5.5.1–5.5.4。
**理由**：测试 helper 是多个测试共享的签名，必须在叶子测试 phase 前完成。

### 5.6 回归测试与废弃处理 `[plan]`

**目标**：确保短期 bug 不回归，旧字段按 5.0 的兼容策略平稳退出。

**内容**：
- `/logout` 不展示内置 `kimi`/`deepseek`/`glm` 的回归测试。
- 删除内置 provider ID 返回明确错误的测试。
- 保留 ID 校验统一后的交叉测试。
- 若方案 A/B 涉及字段废弃：添加 `#[deprecated]` 或文档警告，并在内部调用点全部迁移后启用。

**依赖**：5.1–5.5.5。
**可并行于**：5.7（文档）。
**理由**：跨越多个已完成的 phase，需要 plan 模式整理测试矩阵，避免重复或遗漏。

### 5.7 文档更新 `[normal]`

**目标**：同步 `docs/multi_provider.md`、内联 rustdoc、AGENTS.md/报告中的说明。

**涉及代码**：
- `docs/multi_provider.md`
- 新增/修改字段的 rustdoc
- 本报告第 6 节结论

**依赖**：5.4、5.6。
**理由**：纯文字工作，无架构决策，可在 5.6 基本完成后直接编辑。

### 依赖图

```text
5.0 (design)
│
├─→ 5.1 (plan) ─┐
│                │
├─→ 5.2 (plan) ─┼─→ 5.3 (plan)
│                │
├─→ 5.4 (design)─┘
│        │
│        └─→ 5.5.0 (plan)
│                │
│        ┌───────┼───────┬───────┬───────┐
│        ▼       ▼       ▼       ▼       ▼
│      5.5.1   5.5.2   5.5.3   5.5.4   5.5.5
│      (plan)  (plan)  (plan)  (plan)  (plan)
│        │       │       │       │       │
│        └───────┴───────┴───────┴───────┘
│                    │
│                    ▼
│                  5.6 (plan)
│                    │
│                    ▼
│                  5.7 (normal)
```

### 执行顺序建议

1. 先完成 **5.0**（design）和 **5.4**（design），输出数据模型契约。
2. **5.1** 与 **5.2** 可并行推进，互不阻塞。
3. **5.3** 在 **5.2** 完成后进行，可快速修复 `/logout` 展示问题（相当于更优雅的短期修复）。
4. **5.5.0** 完成后，**5.5.1–5.5.5** 可分组并行给不同 executor；每组按场景迁移调用点。
5. 最后 **5.6** 补充回归测试，**5.7** 更新文档。

---

## 6. 结论

> “provider 和 provider alias 是两个概念，不应该混在一起。”

这个判断是正确的。当前代码在登录侧已经意识到了这一点（`validate_custom_alias` 禁止保留 ID 作为 alias），但在登出侧和 `Config.model_providers` 的数据模型上把它们混成了一个 map，导致：

- `/logout` 菜单把内置 provider type ID 显示为“可删除账号”；
- 删除内置 `kimi` 没有实际效果；
- 用户反复删除仍会复现。

这不仅是实现 bug，更是**概念模型不一致**带来的设计问题。推荐的修复方向是：

1. 短期：在登出流程中区分“用户配置 alias”和“内置 provider type ID”，只展示和删除前者。
2. 长期：在数据模型上拆分 `provider type catalog` 和 `user provider aliases`，避免同一字段承载两种语义。

---

## 7. 相关代码路径汇总

- `tui/src/login/validation.rs` — 自定义 alias 校验，已区分保留 ID
- `tui/src/chatwidget/slash_dispatch.rs:1269-1280` — `/logout` 菜单生成
- `tui/src/app/config_persistence.rs:842-907` — `/logout` 删除逻辑
- `tui/src/login/config.rs:74-112` — 生成删除配置编辑
- `tui/src/chatwidget.rs:1856-1862` — provider 配置同步到 widget
- `core/src/config/mod.rs:3580-3582` — 合并内置与用户 provider
- `model-provider-info/src/lib.rs:40` — `KIMI_PROVIDER_ID = "kimi"`
- `model-provider-info/src/lib.rs:479-512` — 内置 provider 定义与合并逻辑
