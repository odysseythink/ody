# GPT-5 测试迁移方案

## 背景

`models.json` 已被另一个 agent 修改为仅保留 `kimi/deepseek/glm` 三个供应商，所有 `gpt-5.x` 模型均被移除。但大量测试代码和产品逻辑里仍保留对 `gpt-5.x` 的硬编码引用，导致测试失败。

## 落地进展（2026-07 更新）

- ✅ **personality 已真实支持**（commit d35a2b7）：kimi/deepseek/glm 全系在加载时注入 `model_messages`，`supports_personality()` 为真。**不再需要在 test helper 里伪造 personality。**
- ✅ **reasoning levels 已真实支持且模型驱动**（d35a2b7→811801e）：k3 `[low,high,max]`、DeepSeek reasoner/v4 `[medium,max]`、GLM 单档思考开关。**不再需要伪造 reasoning。** 详见 `reasoning-levels-provider-audit.md`。
- ✅ **Kimi 端点/slug 对齐**：`kimi-k2.7-code`→`kimi-for-coding`、`kimi-k3`→`k3`（官方 Kimi Code API id）。本文映射表已更新。
- ✅ **tui lib 测试编译阻塞已修**（commit ab28cc0，`windows_sandbox.rs` 跨平台 import bug，与 gpt-5 无关）。此前 `cargo test -p ody-tui --lib` 直接编译失败，现已可跑。
- 🗑️ **`fast` / service tier：决策为删除相关测试**。当前 models.json 无任何模型带 `service_tiers`；产品不再有 fast 模型，故 gpt-5 专有的 fast/状态栏测试直接删除。
- ⏳ **未做**：slug 批量替换 + 产品硬编码清理（步骤 1/3）。当前基线：`plan_mode` 80/87、`popups_and_settings` 64/74、`core model_switching` 0/12。

## 失败根因

GPT-5 模型在旧测试中不仅是 slug，还承载了几个测试依赖的元数据：

- ~~`personality` 支持~~ → **现在真实模型已支持**（见落地进展），无需伪造。
- ~~`reasoning_levels`~~ → **现在真实模型已支持**，无需伪造。
- `fast` service tier（状态栏显示 `fast`）→ **仍无真实模型承载**，需注入或补 tier。
- 部分测试硬编码了 `provider_id == "kimi"` 或 `"openai"` → 断言需改成真实 provider。

现在 `models.json` 只保留 kimi/deepseek/glm，剩余修复主要是：
1. 把测试里的 `gpt-5.x` slug 替换为当前支持的模型；
2. 处理少数硬编码判断与 `fast` tier；
（personality/reasoning 元数据已在产品层落地，不再是测试负担。）

---

## 统一 slug 映射表

| 旧 slug | 建议映射到 | 说明 |
|---|---|---|
| `gpt-5.4` | `k3` | 大模型、1M context，可承担“fast” tier |
| `gpt-5.2` | `kimi-k2.5` | 标准模型，无 fast |
| `gpt-5.3-ody` | `kimi-for-coding` | 代码/个性模型 |
| `gpt-5.4-mini` | `glm-4.5` | 小模型，可切换 provider |
| `gpt-5.1-ody` | `kimi-for-coding` | 同 `gpt-5.3-ody` |
| `gpt-5.1-ody-max` | `k3` | 大模型 |
| `gpt-5.1-test` | `kimi-k2.5` | 任意可用模型 |

> 如果某些测试只要求“两个不同模型”，也可以把 `gpt-5.4`/`gpt-5.2` 都映射到 `kimi` 家的两个模型，不一定照搬上表。

---

## 修复步骤

### 1. 先修产品代码里的硬编码 gpt-5 判断

最典型的是 `tui/src/chatwidget/model_popups.rs` 里的：

```rust
let warn_for_model = preset.model.starts_with("gpt-5.1-ody")
    || preset.model.starts_with("gpt-5.1-ody-max")
    || preset.model.starts_with("gpt-5.2");
```

应改为基于模型元数据/能力，或暂时禁用：

```rust
let warn_for_model = false;
// TODO: 后续在 models.json / model_info.rs 里加 `warn_for_high_reasoning` 标志
```

其它如 `app_server_event_targets.rs` 测试里的 `"gpt-5.4"` 是测试数据，随测试一起替换。

### 2. 测试模型的元数据（大部分已由真实 catalog 提供）

统一测试常量（放 `tui/src/chatwidget/tests/helpers.rs`）：

```rust
pub(crate) const TEST_MODEL_FAST: &str = "k3";
pub(crate) const TEST_MODEL_STANDARD: &str = "kimi-k2.5";
pub(crate) const TEST_MODEL_CODE: &str = "kimi-for-coding";
pub(crate) const TEST_MODEL_MINI: &str = "glm-4.5";
```

- **personality / reasoning 不需要再注入**：真实 catalog 里这些模型已带 `model_messages`（personality）和 `supported_reasoning_levels`（k3/deepseek/glm）。测试直接用真实能力即可，删掉原先 helper 里伪造 `supports_personality = true` / `supported_reasoning_efforts = …` 的代码。
- **`fast` service tier：决策为删除相关测试**。models.json 无任何模型带 tier，产品也不再有 fast 模型，故 `fast`/状态栏 fast 的 gpt-5 专有测试（如 `set_fast_mode_test_catalog` 及其调用者）直接删除，不迁移。
- `set_fast_mode_test_catalog` 里的 slug 换成上面常量即可；`test_model_info` 不用再手工塞 `model_messages`（真实模型已有）。

### 3. 按 crate 替换测试字符串

替换顺序建议：

1. **TUI 测试**（当前失败最集中的地方）
   - `tui/src/chatwidget/tests/plan_mode.rs`
   - `tui/src/chatwidget/tests/popups_and_settings.rs`
   - `tui/src/chatwidget/tests/slash_commands.rs`
   - `tui/src/chatwidget/tests/status_and_layout.rs`
   - `tui/src/app/tests/model_catalog.rs`
   - `tui/src/app/tests.rs`
   - `tui/src/app/app_server_event_targets.rs` 等测试 helper

2. **`ody-core` 集成测试**
   - 大量 `core/tests/suite/*.rs` 里的 `.with_model("gpt-5.4")`
   - 优先处理 `model_switching.rs`、`model_visible_layout.rs`、`code_mode.rs`

3. **`app-server` 测试**
   - `app-server/tests/suite/v2/*.rs`
   - `app-server/src/config_manager_service_tests.rs`

4. **其它 crate**
   - `analytics`、`exec`、`otel`、`memories` 等

替换时注意：
- 把断言里的 `provider_id == "openai"` 改成 `"kimi"`（或对应 provider）。
- 把 `expected` 消息里的 `"gpt-5.4"` 一并改成新模型名。
- 如果某个测试专门测 gpt-5 独有行为（如 `gpt-5.1-ody-max` 迁移提示、OpenAI 响应 API），应直接删除或改成新模型能力测试。

---

## 验证命令

先只跑 TUI：

```bash
cargo test -p ody-tui --lib chatwidget::tests::plan_mode
cargo test -p ody-tui --lib chatwidget::tests::popups_and_settings
```

再逐步扩大：

```bash
cargo test -p ody-tui --lib
cargo test -p ody-core --tests model_switching
cargo test -p ody-app-server --tests
```

---

## 关键提醒

- ~~personality / reasoning 需先让测试模型拥有能力~~ → **已在产品层落地**（真实 catalog 提供），现在 `plan_mode` 依赖的 reasoning/personality 直接来自真实模型。迁移时反而要**删掉** helper 里旧的伪造代码，避免与真实能力冲突。
- **`fast` tier：决策为删除**。没有真实模型带 tier，产品也不再有 fast 模型；`fast`/状态栏 fast 的 gpt-5 专有测试直接删除，不迁移。
- 建议先把 `model_popups.rs:678` 的 `warn_for_model` 硬编码（`gpt-5.1-ody` / `gpt-5.1-ody-max` / `gpt-5.2`）去掉或改成基于能力，否则即使 slug 替换完，某些 reasoning popup 测试仍可能行为异常。其它产品残留：`core/src/session/mod.rs:2879`（cyber fallback 文案）、`config/src/types.rs:815`（配置键 `hide_gpt-5.1-ody-max_migration_prompt`，改名有兼容性风险）、`tui/src/model_migration.rs`（OpenAI 迁移表）。
- provider 断言：把 `provider_id == "openai"` 改成对应真实 provider（`kimi` / `deepseek` / `glm`）。

## 2026-07-23 进展更新

- ✅ **slug 批量替换已完成**：所有测试文件（`tests/` 目录、`*_tests.rs`、`tests.rs` 模块）中的 `gpt-5.x` slug 已按统一映射表替换为当前支持的模型。扩展映射如下：
  | 旧 slug | 映射到 | 说明 |
  |---|---|---|
  | `gpt-5.3` | `kimi-k2.5` | 标准模型 |
  | `gpt-5.5` | `k3` | 大模型 |
  | `gpt-5.3-spark` | `glm-4.5` | 小/轻量模型 |
  | `gpt-5.1` | `kimi-k2.5` | 标准模型 |
  | `gpt-5.1-high` | `kimi-k2.5-high` | CLI 模型+思考档位参数 |
  | `gpt-5.2-ody` | `kimi-for-coding` | 代码模型 |
  | `test-gpt-5-remote` | `test-k3-remote` | 远程模型测试桩 |
- **当前测试基线**：`plan_mode` 87/87 通过；`popups_and_settings` 72/72 通过；`slash_commands` 和 `status_and_layout` 仍有 fast tier / snapshot 失败，需继续执行 fast tier 测试删除和快照更新。
- **仍待完成**：产品代码硬编码清理（步骤 1）、fast tier 相关测试删除、快照更新。

## 2026-07-23 执行结果

### 已完成的修复

1. **slug 批量替换**：完成所有测试文件中的 `gpt-5.x` 替换（含 `tests/` 目录、`*_tests.rs`、`tests.rs` 模块）。扩展映射表：
   | 旧 slug | 映射到 |
   |---|---|
   | `gpt-5.3` | `kimi-k2.5` |
   | `gpt-5.5` | `k3` |
   | `gpt-5.3-spark` | `glm-4.5` |
   | `gpt-5.1` | `kimi-k2.5` |
   | `gpt-5.1-high` | `kimi-k2.5-high` |
   | `gpt-5.2-ody` | `kimi-for-coding` |
   | `test-gpt-5-remote` | `test-k3-remote` |

2. **fast / service tier 测试删除**：删除了 `tui/src/chatwidget/tests/slash_commands.rs` 和 `tui/src/chatwidget/tests/status_and_layout.rs` 中专测 fast mode 的测试函数，以及 `tui/src/chatwidget/tests.rs` 中未使用的 `SERVICE_TIER_DEFAULT_REQUEST_VALUE` 导入、`slash_commands.rs` 中未使用的 `ServiceTierCommand` 导入。

3. **快照更新**：使用 `INSTA_UPDATE=always` 批量接受因模型名/默认模型变化产生的快照差异。

4. **迁移测试清理**：删除了 `tui/src/app/tests/model_catalog.rs` 中依赖真实 catalog 里已不存在的 `upgrade` 字段的 gpt-5 迁移测试；修正了 `select_model_availability_nux_uses_existing_model_order_as_priority` 的期望（列表按 priority 升序，`glm-4.5` 排在 `k3` 之前）。

5. **reasoning 默认档位测试修复**：`status_command_uses_catalog_default_reasoning_when_config_empty` 的期望从 `k3 (reasoning medium)` 改为 `k3 (reasoning high)`，以匹配真实 catalog 中 k3 的默认 reasoning。

6. **review_mode 默认模型测试修复**：`item_completed_pops_pending_steer_with_local_image_and_text_elements` 原来使用默认模型（现在是 `deepseek-chat`），改为显式使用 `k3` 以恢复行为。

### 当前 TUI 测试结果

```bash
cargo nextest run -p ody-tui --lib
# 2564 passed, 3 failed, 7 skipped
```

剩余 3 个失败均位于 `status::helpers::tests`：
- `compose_agents_summary_includes_global_agents_path`
- `compose_agents_summary_names_global_agents_override`
- `compose_agents_summary_orders_global_before_project_agents`

失败根因：路径格式化期望 `~\AppData\Local\Temp\...`，实际得到 `C:\Users\hkb819\AppData\Local\Temp\...`，属于 Windows 环境下 home 目录路径折叠的独立问题，与 gpt-5 迁移无关。

### 仍待后续处理

- 产品代码中的 gpt-5 硬编码残留（`tui/src/model_migration.rs`、`tui/src/app/startup_prompts.rs`、`config/src/types.rs` 等）。
- 其它 crate 的测试影响（`ody-core`、`app-server`、`exec`、`otel` 等）—— 测试文件中的 slug 已替换，但尚未分 crate 跑验证。

## 2026-07-23 执行结果（最终）

### 已完成的修复（slug 批量替换 + 关联测试修复）

1. **测试文件 slug 批量替换**：完成所有测试文件（`tests/` 目录、`*_tests.rs`、`tests.rs` 模块、含 `#[cfg(test)]` 的 `cli/src/main.rs`）中全部 `gpt-5.x` / `gpt-5` 变体到当前支持模型的替换。完整映射包括：
   | 旧 slug | 映射到 |
   |---|---|
   | `gpt-5.4` | `k3` |
   | `gpt-5.5` | `k3` |
   | `gpt-5.2` | `kimi-k2.5` |
   | `gpt-5.1` | `kimi-k2.5` |
   | `gpt-5.1-high` | `kimi-k2.5-high` |
   | `gpt-5.1-test` | `kimi-k2.5` |
   | `gpt-5.3` | `kimi-k2.5` |
   | `gpt-5.3-ody` | `kimi-for-coding` |
   | `gpt-5.2-ody` | `kimi-for-coding` |
   | `gpt-5-ody` | `kimi-for-coding` |
   | `gpt-5.4-mini` | `glm-4.5` |
   | `gpt-5.3-spark` | `glm-4.5` |
   | `gpt-5-mini` | `glm-4.5` |
   | `gpt-5.1-ody-mini` | `glm-4.5` |
   | `gpt-5-ody-mini` | `glm-4.5` |
   | `gpt-5` | `kimi-k2.5` |
   | `test-gpt-5-remote` | `test-k3-remote` |
   | `test-gpt-5-ody` | `test-kimi-for-coding` |
   | `gpt-5-child-override` | `k3-child-override` |

2. **fast / service tier 测试删除**：删除了 `slash_commands.rs`、`status_and_layout.rs` 中专测 fast mode 的测试函数；清理了未使用的 `SERVICE_TIER_DEFAULT_REQUEST_VALUE`、`ServiceTierCommand` 导入。

3. **快照更新**：使用 `INSTA_UPDATE=always` 接受所有因模型名/默认模型变化产生的快照差异。

4. **迁移/NUX 测试修复**：删除了依赖真实 catalog 中已不存在的 `upgrade` 字段的 gpt-5 迁移测试；修正了 `select_model_availability_nux_uses_existing_model_order_as_priority` 的期望；修正了 `status_command_uses_catalog_default_reasoning_when_config_empty` 的 reasoning 期望。

5. **personality 迁移影响修复**：`composer_submission::interrupted_turn_restore_keeps_active_mode_for_resubmission` 因新模型默认带 personality，放宽了对 `personality: None` 的严格匹配。

### 当前 TUI 测试结果

```bash
cargo nextest run -p ody-tui --lib --no-fail-fast
# 2870 passed, 3 failed, 7 skipped, 1 flaky（重试后通过）
```

剩余 3 个失败均位于 `status::helpers::tests`，与 gpt-5 迁移无关：
- `compose_agents_summary_includes_global_agents_path`
- `compose_agents_summary_names_global_agents_override`
- `compose_agents_summary_orders_global_before_project_agents`

根因：Windows 环境下路径格式化期望 `~\AppData\Local\Temp\...`，实际得到 `C:\Users\hkb819\AppData\Local\Temp\...`，属于 home 目录折叠的独立问题。

### 仍待后续处理

- 产品代码中的 gpt-5 硬编码残留（`tui/src/model_migration.rs`、`tui/src/app/startup_prompts.rs`、`config/src/types.rs`、`core/src/session/mod.rs` 等）。
- 其它 crate 的测试验证（`ody-core`、`app-server`、`exec`、`otel` 等）—— 测试文件中的 slug 已替换，但尚未分 crate 跑完整验证。
- Windows 路径折叠的 `status::helpers` 3 个失败。
