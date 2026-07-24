# Part 6 — 旧代码删除清单与配置迁移策略

## 来源 [C:INFERRED]

基于 `e:/ody-rs` workspace 的全局 grep 结果（`web_search|WebSearch|web_search_mode|SearchClient|WebSearchItem|WebSearchAction`）整理。关键文件已在前序 part 中引用：
- `ext/web-search/src/extension.rs`、`ext/web-search/src/tool.rs` — standalone 扩展路径。
- `ody-api/src/endpoint/search.rs`、`ody-api/src/search.rs` — `SearchClient` / `alpha/search` 端点。
- `core/src/tools/hosted_spec.rs`、`core/src/tools/spec_plan.rs` — hosted web search 工具路径。
- `model-provider-info/src/lib.rs`、`models-manager/src/model_info.rs` — provider/model 能力门控。
- `protocol/src/model_metadata.rs` — `ModelCapabilities` / `WebSearchToolType`。
- `config/src/config_toml.rs`、`config/src/mod.rs`（实际为 `config/src/lib.rs`）— `web_search_mode` 配置。
- `features/src/lib.rs` — `Feature::WebSearchCached` / `Feature::WebSearchRequest` / `Feature::StandaloneWebSearch`。
- `cli/src/doctor.rs` — `--web-search` 强制 live。
- `app-server-protocol` — `WebSearchItem` / `WebSearchAction` 协议字段。
- `tui/src/chatwidget/tool_lifecycle.rs`、`tui/src/history_cell/` — WebSearch 专用 TUI 渲染。
- `packages/integration-tests/src/parity/known-gaps.md` — G14 已知未接线记录。

## 删除阶段总览 [C:USER]

删除按 5 个并行子任务（R1.1–R1.5）执行。每个子任务单独 commit，保证可回滚和可 review。

```
R1.1 ─ ext/web-search 整个 crate 重写准备
R1.2 ─ ody-api SearchClient / alpha/search 端点
R1.3 ─ core hosted/standalone web search 工具路径
R1.4 ─ provider/model 能力字段
R1.5 ─ config/feature/CLI 中 web_search_mode
```

## R1.1 — 删除 ext/web-search 旧内容 [C:USER]

### 删除文件
- `ext/web-search/src/extension.rs`（旧内容）
- `ext/web-search/src/tool.rs`（旧内容）
- `ext/web-search/src/history.rs`
- `ext/web-search/src/output.rs`
- `ext/web-search/src/schema.rs`
- `ext/web-search/web_run_description.md`
- `ext/web-search/src/lib.rs` 中旧 re-export

### 保留/重写文件
- `ext/web-search/Cargo.toml`：更新依赖，移除 `ody-api`、`ody-model-provider`、`ody-protocol` 旧类型；新增 `ody-web-search`、`ody-core`、`ody-extension-api`、`ody-tools`。
- `ext/web-search/src/lib.rs`：仅保留 `pub mod extension; pub mod tool;` 或等价。
- `ext/web-search/src/extension.rs`：重写为 host 注入逻辑（见 `config-injection.md`）。
- `ext/web-search/src/tool.rs`：重写为 `WebSearchTool`（见 `tool-tui.md`）。
- `ext/web-search/web_search_description.md`：新增工具描述文件。

### workspace 引用
- `Cargo.toml` workspace member 列表保持 `ext/web-search` 不变（仅保留目录）。
- `app-server/Cargo.toml` 中 `ody-web-search-extension` 依赖保留。
- `app-server/src/extensions.rs:75` 中 `ody_web_search_extension::install(&mut builder);` 保留。
- `core/Cargo.toml` 中 dev-dependency `ody-web-search-extension`：若仅用于旧测试，则删除；若新测试仍需要，保留。
- `core/tests/suite/code_mode.rs`、`core/tests/suite/responses_lite.rs`、`core/tests/suite/sqlite_state.rs` 中手动 install 旧扩展的测试代码：删除或改为新扩展。

## R1.2 — 删除 ody-api SearchClient / alpha/search 端点 [C:USER]

### 删除文件（若确认无其他使用方）
- `ody-api/src/endpoint/search.rs`
- `ody-api/src/search.rs` 中 `SearchRequest`、`SearchResponse`、`SearchCommands`、`SearchQuery`、`SearchSettings`、`ExternalWebAccess`、`SearchContextSize`、`ApproximateLocation` 等类型。

### 检查清单
- 全 workspace grep `SearchClient`、`SearchRequest`、`SearchCommands`、`alpha/search`、`SearchSettings` 确认无残留引用。
- 若某些类型仍被 analytics 或 protocol 使用，则仅删除 web search 相关字段，保留共享类型。

### 更新文件
- `ody-api/src/endpoint/mod.rs`：移除 `pub mod search;` / `pub use search::*;`。
- `ody-api/src/lib.rs`：移除对应 re-export。

## R1.3 — 删除 core 中 hosted/standalone web search 工具路径 [C:USER]

### 删除/修改文件
- `core/src/tools/hosted_spec.rs`：删除 `create_web_search_tool` 函数及 `WebSearchToolOptions`（若 image generation 也共用此文件，只删除 web search 部分）。
- `core/src/tools/spec_plan.rs`：
  - 删除 `hosted_model_tool_specs` 中 web search 分支；
  - 删除 `search_tool_enabled` 函数；
  - 删除 `standalone_web_search_enabled` 函数；
  - 删除 `standalone_web_search_available` 相关逻辑；
  - 删除 `turn_context.config.web_search_mode` 引用。
- `core/src/tools/spec_plan_tests.rs`：删除 web search 相关测试。
- `core/src/web_search.rs`：删除 `web_search_action_detail`、`web_search_detail` 导出（确认仅被 web search 使用）。
- `core/src/lib.rs`：移除 `web_search` 模块导出。
- `core/src/event_mapping.rs`：删除 `ResponseItem::WebSearchCall` 处理分支。
- `core/src/context_manager/history.rs`：删除 `WebSearchCall` 相关分支。
- `core/src/session/mod.rs`、`core/src/session/review.rs`、`core/src/session/turn_context.rs`：删除 `web_search_mode` per-turn 配置。
- `core/src/tasks/review.rs`：删除 web search mode 设置。
- `core/src/guardian/prompt.rs`、`core/src/guardian/review_session.rs`：删除 web search 相关 guardian 分支（若存在）。
- `core/src/image_preparation.rs`：检查是否使用 `web_search`（若存在）。
- `core/tests/suite/web_search.rs`：删除旧测试文件。
- `core/src/compact_remote.rs`：检查是否使用 `web_search` 能力字段。
- `core/src/client_common.rs`、`core/src/client_tests.rs`：检查并清理。

## R1.4 — 删除 provider/model 能力字段 [C:USER]

### 修改文件
- `model-provider-info/src/lib.rs`：
  - 删除 `ProviderCapabilities` 中 `web_search` 字段；
  - 删除 `default_provider_capabilities_for_wire_api` 中 web search 默认值；
  - 删除 `create_chat_provider` 中 `web_search: false` 初始化；
  - 删除 `normalize_capabilities` 中 web search 相关回填。
- `models-manager/src/model_info.rs`：
  - 删除 `resolve_model_capabilities` 中 provider caps 压制 model caps 的 web search 分支；
  - 删除 `default_model_capabilities_for_wire_api` 中 `supports_search_tool` 相关逻辑。
- `model-provider-info/src/model_provider_info_tests.rs`：删除相关测试。
- `models-manager/src/model_info_tests.rs`：删除相关测试。
- `model-provider/src/adapters/core.rs`、`model-provider/src/chat_provider.rs`、`model-provider/src/provider.rs`：检查并清理 `web_search` 能力引用。
- `protocol/src/model_metadata.rs`：
  - 删除 `ModelCapabilities` 中 `supports_search_tool` 字段；
  - 删除 `ModelCapabilities` 中 `web_search_tool_type` 字段；
  - 删除 `WebSearchToolType` 枚举；
  - 删除 `ModelInfo` 顶层 `web_search_tool_type` 字段（若存在）。
- `app-server-protocol/src/protocol/v2/model.rs`、`app-server-protocol/src/protocol/v2/config.rs`：删除 `web_search` 相关字段/枚举。
- `app-server/tests/suite/v2/model_provider_capabilities_read.rs`、`app-server/tests/suite/v2/web_search.rs`：删除或重写为不依赖旧能力的测试。
- `cli/tests/providers.rs`：检查是否包含 `web_search` 能力断言。

## R1.5 — 删除 config/feature/CLI 中 web_search_mode [C:USER]

### 修改文件
- `config/src/config_toml.rs`：
  - 删除 `ConfigToml::web_search: Option<WebSearchMode>` 字段；
  - 删除 `web_search_config` 相关字段（如 `WebSearchToolConfig` 引用）；
  - 新增 `services: Option<ServicesConfigToml>`（见 `config-injection.md`）。
- `config/src/mod.rs`（即 `config/src/lib.rs`）：移除 `WebSearchModeRequirement` 相关导出（若不再需要）。
- `config/src/config_requirements.rs`：删除 `WebSearchModeRequirement` 相关结构和校验。
- `config/src/types.rs`：删除 `WebSearchToolConfig` 等类型（若存在）。
- `config/src/config_tests.rs`：删除大量 `web_search_mode` 测试；新增 `services.webSearch` 测试。
- `config/src/config_requirements.rs`：删除 `allowed_web_search_modes` 相关。
- `features/src/lib.rs`：删除 `Feature::WebSearchCached`、`Feature::WebSearchRequest`、`Feature::StandaloneWebSearch`。
- `features/src/legacy.rs`、`features/src/tests.rs`：同步清理。
- `cli/src/doctor.rs`：删除 `--web-search` 强制 live 逻辑。
- `cli/src/main.rs`：删除相关 CLI flag 解析。
- `core/src/session/config_lock.rs`：删除 `web_search_mode` 相关锁定。
- `core/src/session/tests.rs`、`core/src/session/turn.rs`：删除 per-turn 配置相关测试。
- `protocol/src/config_types.rs`：删除 `WebSearchMode`、`WebSearchContextSize`、`WebSearchToolConfig` 等枚举/结构（若仅被 web search 使用）。
- `config/src/key_aliases.rs`、`config/src/merge_tests.rs`、`config/src/state_tests.rs`：清理相关引用。

### 旧配置迁移策略 [C:USER]

- 当 `ConfigToml` 反序列化时发现旧 key `web_search`（原 `Option<WebSearchMode>`），发出 `startup_warning`：
  ```
  The `web_search` config key is deprecated. Use `[services.webSearch]` instead.
  ```
- 不自动迁移旧值到 `services.webSearch`：因为语义完全不同（旧值是 mode 枚举，新值是 provider + API key），自动迁移无意义且可能引入错误配置。
- 若用户仅有旧配置，web search 工具不注册；用户需手动添加 `services.webSearch`。
- 配置 schema 中不再保留 `web_search` 字段；旧 key 通过 serde `default` 或自定义 visitor 检测并 warning。

### 实现建议

在 `ConfigToml` 中使用一个自定义反序列化辅助字段：

```rust
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ConfigToml {
    // ...

    /// Deprecated; kept only to emit a migration warning.
    #[serde(default, rename = "web_search")]
    #[schemars(skip)]
    pub deprecated_web_search: Option<serde_json::Value>,
}
```

在配置加载后：

```rust
if config_toml.deprecated_web_search.is_some() {
    startup_warnings.push(
        "The `web_search` config key is deprecated. Use `[services.webSearch]` instead.".to_string(),
    );
}
```

后续版本（T3.1.3 或更晚）可彻底删除 `deprecated_web_search` 字段，使旧 key 触发 schema 错误。

## 协议字段迁移 [C:USER]

### 删除字段
- `app-server-protocol/src/protocol/thread_history.rs` 中 `WebSearchItem` / `WebSearchAction` 相关。
- `app-server-protocol/src/protocol/v1.rs` / `v2/item.rs` 中 web search 相关 item 变体。
- `ody_protocol::items::WebSearchItem` 和 `ody_protocol::models::WebSearchAction` 删除。

### 旧数据兼容
- 若 thread history 或 session log 中已序列化旧 `WebSearchItem`：
  - 方案：在反序列化层保留一个兼容解析层，将旧 `WebSearchItem` 降级为纯文本 `ThreadItem::Message` 或 `ThreadItem::DynamicToolCall`。
  - 或：在加载时跳过未知 item 类型，仅记录 warning。
- 推荐方案：保留兼容解析层一个版本，显示降级文本 `"[legacy web search result]"`，避免旧 thread 无法打开。
- 该决策对 `core/src/context_manager/history.rs` 和 `app-server-protocol` 影响较大；需单独验证（见 `Assumptions` 第 2 条）。

## 依赖图清理 [C:INFERRED]

删除后，以下依赖关系应消失：
- `ody-web-search-extension` → `ody-api`（旧 `SearchClient`）
- `ody-web-search-extension` → `ody-model-provider`
- `core` → `ody-web-search-extension`（测试 dev-dependency 按需）
- `ody-api` → web search 端点（若 `alpha/search` 无其他使用方）
- `model-provider-info` / `models-manager` / `protocol` → `web_search` 能力字段

## 验证清单 [C:INFERRED]

- `cargo check -p ody-app-server -p ody-core -p ody-api -p ody-config -p ody-features -p ody-cli -p ody-model-provider-info -p ody-models-manager -p ody-protocol` 通过。
- 全 workspace grep 无 `web_search_mode`、`WebSearchMode`、`WebSearchToolType`、`supports_search_tool`、`SearchClient`、`alpha/search`、`WebSearchAction`、`WebSearchItem` 残留（除兼容层和测试外）。
- `cargo nextest run -p ody-core -p ody-app-server -p ody-config` 通过。
- 旧配置 warning 测试通过。

## 风险与决策

| 决策 | 选择 | 理由 |
|---|---|---|
| 是否彻底删除 `ext/web-search` 目录 | 否，保留目录重写内容 | 减少 workspace member 和 Cargo.toml 改动。 |
| 旧 `web_search` 配置是否自动迁移 | 否 | 语义完全不同；warning 提示手动迁移。 |
| 旧 `WebSearchItem` 历史数据是否兼容 | 保留降级解析层一个版本 | 避免旧 thread 无法打开；后续版本可删除。 |
| `alpha/search` 端点是否删除 | 是（若确认无其他使用方） | 本次目标就是完全删除后端搜索路径。 |
| 删除顺序 | 先 R1.1–R1.5 并行，再 I2.x 新建 | 避免新旧代码在同一 commit 中混合。 |

---

## 文件改动清单（汇总） [C:INFERRED]

### 删除/清理
- `ext/web-search/src/history.rs`
- `ext/web-search/src/output.rs`
- `ext/web-search/src/schema.rs`
- `ext/web-search/web_run_description.md`
- `ody-api/src/endpoint/search.rs`（若无其他使用方）
- `ody-api/src/search.rs` 中 web search 类型
- `core/src/web_search.rs`
- `core/src/tools/spec_plan_tests.rs` 中 web search 测试
- `core/tests/suite/web_search.rs`
- `protocol/src/model_metadata.rs` 中 `WebSearchToolType` / `supports_search_tool` / `web_search_tool_type`
- `protocol/src/config_types.rs` 中 `WebSearchMode` / `WebSearchContextSize` / `WebSearchToolConfig`
- `features/src/lib.rs` 中 `Feature::WebSearchCached` / `Feature::WebSearchRequest` / `Feature::StandaloneWebSearch`
- `cli/src/doctor.rs` 中 `--web-search` 逻辑

### 重写
- `ext/web-search/src/extension.rs`
- `ext/web-search/src/tool.rs`
- `ext/web-search/src/lib.rs`
- `ext/web-search/Cargo.toml`

### 新增
- `ext/web-search/web_search_description.md`
- `config/src/config_toml.rs` 中 `ServicesConfigToml` / `ConfigToml::services`
- `core/src/config/mod.rs` 中 `Config::services_web_search`
- `tui/src/thread_transcript.rs` 中 `WebSearch` chip 分支

### 修改
- `config/src/config_toml.rs`（删除 `web_search` 字段，新增 `services`）
- `core/src/tools/spec_plan.rs`（删除 hosted/standalone web search 逻辑）
- `core/src/tools/hosted_spec.rs`（删除 web search 分支）
- `model-provider-info/src/lib.rs`（删除 `web_search` 能力字段）
- `models-manager/src/model_info.rs`（删除 `supports_search_tool` 压制逻辑）
- `app-server-protocol`（删除 `WebSearchItem` / `WebSearchAction` 或保留兼容层）
- `tui/src/chatwidget/tool_lifecycle.rs`（删除 `on_web_search_begin` / `on_web_search_end`）
- `tui/src/history_cell/mod.rs` 及子文件（删除 `WebSearchCell`）
- `tui/src/chatwidget/protocol.rs`（删除 `ThreadItem::WebSearch` 分支）
