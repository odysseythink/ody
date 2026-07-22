# GPT-5 测试迁移方案

## 背景

`models.json` 已被另一个 agent 修改为仅保留 `kimi/deepseek/glm` 三个供应商，所有 `gpt-5.x` 模型均被移除。但大量测试代码和产品逻辑里仍保留对 `gpt-5.x` 的硬编码引用，导致测试失败。

## 失败根因

GPT-5 模型在旧测试中不仅是 slug，还承载了几个测试依赖的元数据：

- `personality` 支持（`model_messages` 包含 `{{ personality }}` 占位符）
- `reasoning_levels`（`medium` / `high` 等）
- `fast` service tier（状态栏显示 `fast`）
- 部分测试硬编码了 `provider_id == "kimi"` 或 `"openai"`

现在 `models.json` 只保留 kimi/deepseek/glm，因此修复需要两步：
1. 让产品/测试元数据对齐新模型；
2. 把测试里的 `gpt-5.x` slug 替换为当前支持的模型。

---

## 统一 slug 映射表

| 旧 slug | 建议映射到 | 说明 |
|---|---|---|
| `gpt-5.4` | `kimi-k3` | 大模型、1M context，可承担“fast” tier |
| `gpt-5.2` | `kimi-k2.5` | 标准模型，无 fast |
| `gpt-5.3-ody` | `kimi-k2.7-code` | 代码/个性模型 |
| `gpt-5.4-mini` | `glm-4.5` | 小模型，可切换 provider |
| `gpt-5.1-ody` | `kimi-k2.7-code` | 同 `gpt-5.3-ody` |
| `gpt-5.1-ody-max` | `kimi-k3` | 大模型 |
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

### 2. 给测试用模型注入缺失的元数据

TUI 测试通过 `test_model_catalog()` 加载真实 catalog，但真实模型现在没有 `personality` / `reasoning` / `fast` 这些测试需要的能力。建议在 `tui/src/chatwidget/tests/helpers.rs` 里给测试模型“注入”这些能力：

```rust
use ody_protocol::model_metadata::{
    ModelInstructionsVariables, ModelMessages, ModelServiceTier, ReasoningEffort, ReasoningEffortPreset,
};

pub(crate) const TEST_MODEL_FAST: &str = "kimi-k3";
pub(crate) const TEST_MODEL_STANDARD: &str = "kimi-k2.5";
pub(crate) const TEST_MODEL_CODE: &str = "kimi-k2.7-code";
pub(crate) const TEST_MODEL_MINI: &str = "glm-4.5";

pub(super) fn test_model_catalog(_config: &Config) -> Arc<ModelCatalog> {
    let mut presets = crate::test_support::TEST_MODEL_PRESETS.clone();
    for preset in &mut presets {
        if matches!(
            preset.model.as_str(),
            TEST_MODEL_FAST | TEST_MODEL_STANDARD | TEST_MODEL_CODE | TEST_MODEL_MINI
        ) {
            preset.supports_personality = true;
        }
        if matches!(preset.model.as_str(), TEST_MODEL_FAST | TEST_MODEL_STANDARD) {
            preset.default_reasoning_effort = ReasoningEffort::Medium;
            preset.supported_reasoning_efforts = vec![ReasoningEffortPreset {
                effort: ReasoningEffort::Medium,
                description: "medium".to_string(),
            }];
        }
        if preset.model == TEST_MODEL_FAST {
            preset.service_tiers.push(ModelServiceTier {
                id: ServiceTier::Fast.request_value().to_string(),
                name: "fast".to_string(),
                description: "Fastest inference with increased plan usage".to_string(),
            });
        }
    }
    Arc::new(ModelCatalog::new(presets))
}
```

同时把 `set_fast_mode_test_catalog` 里的 slug 改成新常量，并给 `test_model_info` 补上 `provider` 和 `model_messages`：

```rust
fn test_model_info(slug: &str, priority: i32, supports_fast_mode: bool) -> ModelInfo {
    // ...
    serde_json::from_value(json!({
        "slug": slug,
        "display_name": slug,
        "description": format!("{slug} description"),
        "provider": "kimi",
        "model_messages": {
            "instructions_template": "You are Ody, a coding agent.\n\n{{ personality }}\n\nbase instructions",
            "instructions_variables": {
                "personality_default": "",
                "personality_friendly": "friendly",
                "personality_pragmatic": "pragmatic"
            }
        },
        "default_reasoning_level": "medium",
        // ... 其余字段保持不变
    }))
}
```

```rust
pub(crate) fn set_fast_mode_test_catalog(chat: &mut ChatWidget) {
    let models: Vec<ModelPreset> = ModelsResponse {
        models: vec![
            test_model_info(TEST_MODEL_FAST, 0, true),
            test_model_info(TEST_MODEL_STANDARD, 1, false),
        ],
    }
    .models
    .into_iter()
    .map(Into::into)
    .collect();

    chat.model_catalog = Arc::new(ModelCatalog::new(models));
}
```

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

- 不要简单地把 `gpt-5.4` 全局替换成某个字符串就完事；`plan_mode` 测试依赖 reasoning 和 personality，必须先让测试模型拥有这些能力。
- 如果产品层希望 `kimi/deepseek/glm` 真实支持 personality / reasoning levels，更好的做法是在 `models-manager/src/model_info.rs` 或 `models.json` 里给这些模型补上元数据，而不是只在测试 helper 里伪造。
- 建议先把 `model_popups.rs` 的 `warn_for_model` 硬编码去掉，否则即使 slug 替换完，某些 reasoning popup 测试仍可能行为异常。
