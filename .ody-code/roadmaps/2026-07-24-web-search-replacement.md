# WebSearch 重构：完全移除 Rust 现有实现，按 TS `ody-code` 参照重新实现

**Goal:** 删除 `e:/ody-rs` 中当前所有与 web search 相关的代码（hosted `web_search`、standalone `web/run` 扩展、`alpha/search` 端点、provider/model 能力门控、`web_search_mode` 配置），然后以 `D:/workspace/go_work/ody-code/packages/agent-core/src/tools/builtin/web/web-search.ts` 及其 `providers/web-search/*` 为唯一参照，重新实现一个 host 可注入 provider、多搜索后端注册表 + fallback、工具名 `WebSearch`、输入参数 `{query, limit, include_content}` 的 web search 工具。

**Architecture:**
- 移除 Provider/Model 层与 web search 相关的能力门控；web search 变为纯工具层能力。
- 引入 `WebSearchProvider` trait + `WebSearchResult`（对齐 TS `WebSearchProvider` / `WebSearchResult`）。
- 引入 `WebSearchProviderRegistry` + `FallbackWebSearchProvider`（对齐 TS registry / fallback）。
- 新增 `WebSearchTool` 作为 core builtin tool（或 extension tool），仅在有可用 provider 时注册。
- Provider 实例由 app-server/host 根据 `services.webSearch` 配置创建并注入，不对模型暴露 provider 细节。
- 工具输出为纯文本（Title/URL/Snippet/Content），TUI 侧用 chip 计数结果条数。
- 移除 `ody-api::SearchClient` / `alpha/search` 端点依赖；客户端直接调用公开搜索 API（或 host 注入的 provider）。

**Tech Stack:** Rust workspace（`ody-core`、`ody-tools`、`ody-protocol`、`ody-config`、`ody-features`、`ody-app-server`、`ody-tui`、`ody-api`、`ody-model-provider-info`、`ody-models-manager`、`ext/web-search` 或新 crate）。

**Scope In:**
- 删除旧 web search 全部运行时路径、配置、feature flag、provider/model 能力字段、CLI flag。
- 删除 `ody-api` 中 `SearchClient` / `alpha/search` 端点（若无其他使用方）。
- 新增 `WebSearchProvider` trait、注册表、fallback、错误分类。
- 实现 TS 中全部 12 个 provider 的 Rust 版本：DuckDuckGo、Bing、SerpApi、SearchApi、Serper、Baidu、Serply、SearXNG、Tavily、Exa、Perplexity、Moonshot（可分批）。
- 新增 `services.webSearch` 配置与 provider 配置 schema。
- 新增 `WebSearchTool` 注册与执行路径，输出纯文本。
- 工具结果 chip 渲染（TUI 侧）。
- 单测：provider 单元测、registry/fallback 单元测、工具执行单元测、配置解析单测；parity 测试与 TS 对齐。

**Scope Out:**
- 不保留 Responses API hosted `web_search` 原生工具路径（本次目标就是删掉它）。
- 不保留 `web/run` namespaced extension 工具路径。
- 不保留 `OpenPage` / `FindInPage` 语义；新的 `WebSearchTool` 只支持 `query` 搜索（与 TS 一致）。
- 本次不考虑 provider 的 OAuth 刷新链路（TS `resolveOAuthTokenProvider` 可后续补齐）。
- 不修改 LLM 协议层对工具 schema 的通用处理，只修改 web search 工具自身的 schema。

**Last Updated:** 2026-07-24

---

## 探索报告（当前 Rust vs TS 参照）

### Rust 端现状（`e:/ody-rs`）：双层门控，默认全部关闭

1. **Provider 层门控**  
   `model-provider-info/src/lib.rs` 中 `create_chat_provider` 给 Kimi/DeepSeek/GLM 显式设置 `capabilities.web_search = false`。`default_provider_capabilities_for_wire_api` 仅对 OpenAI Responses 默认开启 `web_search`，但 `normalize_capabilities` 只会回填“未显式配置”的自定义 provider。

2. **Model 层门控**  
   `models-manager/src/model_info.rs:83-86` 将 provider 层 `web_search` 作为模型层 `supports_search_tool` 的上界：Provider 一旦 false，模型强制 false。`default_model_capabilities_for_wire_api` 对 Responses 也默认不设置 `supports_search_tool`。

3. **Hosted 工具路径**  
   `core/src/tools/spec_plan.rs:306-333` 在 `provider.capabilities().web_search` 为 true 且 standalone 不可用时，向模型发送 hosted `web_search` tool spec。`core/src/tools/hosted_spec.rs` 将其映射为 `ToolSpec::WebSearch`。

4. **Standalone 扩展路径**  
   `ext/web-search/src/extension.rs` 在 `provider.capabilities().web_search` 为 true 且 `web_search_mode != Disabled` 时注册 `web/run` namespaced tool。`ext/web-search/src/tool.rs` 使用 `ody_api::SearchClient` 向 provider 的 `alpha/search` 端点发送请求。`Feature::StandaloneWebSearch` 默认关闭（`features/src/lib.rs:882-886`）。

5. **协议/配置/CLI 门控**  
   `protocol/src/model_metadata.rs` 定义 `WebSearchToolType`、`WebSearchMode`；`config/src/mod.rs` 解析 `web_search_mode` 默认 fallback 到 `Cached`；`cli/src/doctor.rs` 支持 `--web-search` 强制 live。

6. **已知未接线**  
   `packages/integration-tests/src/parity/known-gaps.md` 明确记录：  
   - G14: “Web-search user registration path not fully wired”  
   - “Rust user-registered tool execution path not fully wired”

### TS 端参照（`D:/workspace/go_work/ody-code`）：host 注入 + 多 provider 注册表

1. **工具接口**  
   `packages/agent-core/src/tools/builtin/web/web-search.ts` 定义 `WebSearchTool`：工具名 `WebSearch`，输入 `{query, limit, include_content}`，执行时调用注入的 `WebSearchProvider.search(query, {limit, includeContent})`，输出格式化为 Title/Date/URL/Snippet/Content 文本。

2. **Provider 注册表**  
   `packages/agent-core/src/tools/providers/web-search/registry.ts` 实现 `WebSearchProviderRegistry`，注册 12 个 provider：duckduckgo、serpapi、searchapi、serper、bing、baidu、serply、searxng、tavily、exa、perplexity、moonshot。`runtime.ts` 用 `resolveWebSearchConfig` 选择 primary/secondary，包装成 `FallbackWebSearchProvider`。

3. **注册入口**  
   `packages/agent-core/src/agent/tool/index.ts:474` 仅在 `toolServices?.webSearcher` 存在时实例化 `WebSearchTool`，即 host 注入。

4. **UI 侧**  
   `apps/ody-code/src/tui/components/messages/tool-renderers/chip.ts:103-121` 的 `webSearchChip` 仅按结果列表项计数，不依赖特殊事件类型。

### 对比速览

| 维度 | TS `ody-code` | Rust `ody-rs` 现状 |
| --- | --- | --- |
| 执行位置 | 客户端/Host 直接调公开搜索 API | 后端 hosted 或 provider `alpha/search` |
| Provider 数量 | 12+ 注册表 + fallback | 1 个（provider 后端） |
| 是否依赖 provider 能力 | 否 | 是，Provider/Model 双层门控 |
| 工具名 | `WebSearch` | `web_search` / `web/run` |
| 输入参数 | `query`/`limit`/`include_content` | `SearchCommands`（search/open/find/image_query） |
| 输出形式 | 纯文本 | 结构化事件 `WebSearchItem` |
| 默认启用 | 只要 host 注入 provider 即可 | 默认关闭，多道门控 |
| 完成度 | 成熟 | 已知未完全接线 |

---

## Execution Rubric

### A. 切分粒度原则

按“删除 → 抽象 → Provider 实现 → 接线 → UI → 测试”切分，每个子阶段：
- 独立可编译、可测试、可回滚；
- 不混合“删除旧代码”与“写新代码”在同一 commit；
- 单个子阶段一般不超过 8 个文件，避免 executor 在中途压缩上下文导致遗漏；
- Provider 实现彼此独立，可逐个交付。

### B. 模式判定准则

| 模式 | 判定标准 | 理由 |
| --- | --- | --- |
| **normal** | 机械删除、唯一正确解、改动局部、无架构决策 | 可直接改代码 |
| **plan** | 多步骤、有真实依赖、共享签名/调用方扇出、需要逐任务 TDD 计划 | 先写依赖图 + 任务列表 |
| **design** | 架构、数据模型、公开接口/契约、迁移语义存在真未知，猜错代价大 | 在批准 spec 前锁住实现 |

本次存在真未知：
- 新 `WebSearchProvider` trait 放在哪个 crate，如何被 app-server 注入；
- 旧 `WebSearchItem` / `WebSearchAction` 协议字段如何迁移/删除；
- `services.webSearch` 配置与现有 `web_search_mode` 配置的迁移路径。

因此总体骨架先走 **design**；删除死代码和逐个 provider 实现可降级为 **normal**；core 接线与注册表集成走 **plan**。

---

## 总览

| # | 子任务 | 范围 | 模式 | Depends on | 可并行 |
| --- | --- | --- | --- | --- | --- |
| D1.0 | 设计 spec：trait 位置、注入点、配置 schema、协议字段迁移 | 设计文档 | [design] | none | — |
| R1.1 | 删除 ext/web-search 扩展 | 整个 crate、workspace/Cargo.toml 引用 | [normal] | D1.0 | 否 |
| R1.2 | 删除 ody-api SearchClient / alpha/search 端点 | `ody-api/src/endpoint/search.rs`、`ody-api/src/search.rs` 等 | [normal] | D1.0 | 否（与 R1.1 同层，可并行） |
| R1.3 | 删除 core 中 hosted/standalone web search 工具路径 | `core/src/tools/hosted_spec.rs`、`core/src/tools/spec_plan.rs` 相关分支 | [normal] | D1.0 | 可并行（R1.1/R1.2 独立） |
| R1.4 | 删除 provider/model 能力字段 | `model-provider-info`、`models-manager`、`protocol` | [normal] | D1.0 | 可并行 |
| R1.5 | 删除 config/feature/CLI 中 web_search_mode | `config`、`features`、`cli` | [normal] | D1.0 | 可并行 |
| I2.1 | 新增 WebSearchProvider trait 与 WebSearchResult | 决定 crate（建议 `ody-core` 或新 `ody-web-search`） | [plan] | D1.0 | 否 |
| I2.2 | 新增 provider 注册表 + fallback + 错误分类 | 对齐 TS registry/runtime | [plan] | I2.1 | 否 |
| I3.1-I3.12 | 逐个实现 12 个 provider | 每个 provider 独立文件 + 单元测 | [normal] | I2.2 | 可并行（provider 之间） |
| I4.1 | 新增 `services.webSearch` 配置 schema | `ody-config` + `agent-core-shared` 风格 | [plan] | D1.0 | 可并行（与 I2.x 并行） |
| I4.2 | 实现 app-server 中 provider 解析与注入 | `app-server/src/extensions.rs` 或新注入点 | [plan] | I2.1, I4.1 | 否 |
| I5.1 | 实现 `WebSearchTool` 并接入工具规划 | `core/src/tools/` 或 extension | [plan] | I2.2, I4.2 | 否 |
| I6.1 | TUI chip 渲染 | `tui/src/...` 工具结果 chip | [normal] | I5.1 | 否 |
| T7.1 | provider 单元测试 | 每个 provider | [normal] | I3.x | 否 |
| T7.2 | registry/fallback 测试 | 对齐 TS `fallback.test.ts`、`runtime.test.ts` | [normal] | I2.2 | 否 |
| T7.3 | 工具执行与配置解析测试 | `core` 测试 | [normal] | I5.1, I4.1 | 否 |
| T7.4 | TS-Rust parity 测试 | `integration-tests` 或新 parity 用例 | [plan] | I5.1, I6.1 | 否 |

---

## 详细阶段

### D1.0 设计 spec：新架构契约 [design]

**目标：** 产出一份被批准的设计文档，明确新 web search 的 crate 边界、注入点、配置 schema、协议字段迁移方案。

**待决策问题：**
1. `WebSearchProvider` trait 放在哪个 crate？
   - 候选 A：`ody-core`（工具执行侧）；
   - 候选 B：新建 `ody-web-search` crate（被 `app-server` 和 `core` 共同依赖）；
   - 候选 C：保留 `ext/web-search` 但重写。
2. Provider 注入方式：
   - 候选 A：app-server 创建 provider 实例后通过 `ExtensionRegistry` 注入；
   - 候选 B：core 直接从 `Config` 解析 provider（更像 core 直接拥有）。
   推荐 A，因为 TS 是 host 注入，Rust 的 app-server 等价于 host。
3. 配置 schema：是否完全复刻 TS `services.webSearch`（provider + apiKey + timeoutMs + options）？是否保留 `web_search_mode` 的任何字段？
4. 协议字段：旧 `WebSearchItem` / `WebSearchAction` 是否直接删除？TUI 侧是否改为纯文本 chip，删除专用 `WebSearch` 渲染分支？
5. 同步/异步：trait 方法是否为 `async fn`？Rust 中是否需要 `Send` bound？

**交付物：** `.ody-code/designs/web-search-replacement.md` 或本 roadmap 中新增 design 章节。

**证据依赖：** 已读取 `ody-api/src/endpoint/search.rs`、`ext/web-search/src/tool.rs`、`packages/agent-core/src/tools/builtin/web/web-search.ts`、`packages/agent-core/src/tools/providers/web-search/registry.ts`。

---

### R1.1 删除 ext/web-search 扩展 [normal]

**文件：**
- `ext/web-search/` 整个目录（`Cargo.toml`、`src/extension.rs`、`src/tool.rs`、`src/history.rs`、`src/output.rs`、`src/schema.rs`、`web_run_description.md`、测试）。
- `Cargo.toml` workspace member 列表。
- `app-server/Cargo.toml` 中 `ody-web-search-extension` 依赖。
- `app-server/src/extensions.rs` 中 `ody_web_search_extension::install(&mut builder);`。
- `core/Cargo.toml` 中 dev-dependency `ody-web-search-extension`。
- `core/tests/suite/code_mode.rs`、`core/tests/suite/responses_lite.rs`、`core/tests/suite/sqlite_state.rs` 中手动 install 该扩展的测试代码。

**验证：**
- `cargo check -p ody-app-server -p ody-core` 通过；
- `cargo nextest run -p ody-app-server` 通过；
- `cargo nextest run -p ody-core` 通过。

**Depends on:** D1.0

---

### R1.2 删除 ody-api SearchClient / alpha/search 端点 [normal]

**文件：**
- `ody-api/src/endpoint/search.rs`（若确认无其他使用方则删除；否则保留为私有 stub）。
- `ody-api/src/search.rs` 中 `SearchRequest`、`SearchResponse`、`SearchCommands`、`SearchQuery` 等（若仅被 web search 使用则删除）。
- `ody-api/src/lib.rs` 中对应 re-export。
- 检查 `ody-api/src/endpoint/mod.rs` 中的 `pub use search::SearchClient;`。

**验证：**
- `cargo check -p ody-api` 通过；
- 全 workspace grep 确认无 `SearchClient`、`alpha/search` 残留引用。

**Depends on:** D1.0
**可并行：** 与 R1.1 并行。

---

### R1.3 删除 core 中 hosted/standalone web search 工具路径 [normal]

**文件：**
- `core/src/tools/hosted_spec.rs` 中 `create_web_search_tool` 函数及 `WebSearchToolOptions`（若 image generation 也共用此文件，只删除 web search 部分）。
- `core/src/tools/spec_plan.rs`：
  - `hosted_model_tool_specs` 中 web search 分支；
  - `search_tool_enabled` 函数；
  - `standalone_web_search_enabled` 函数；
  - `standalone_web_search_available` 相关逻辑。
- `core/src/tools/spec_plan_tests.rs` 中相关 web search 测试。
- `core/src/web_search.rs` 及 `core/src/lib.rs` 中 `web_search_action_detail`、`web_search_detail` 导出（确认是否仅被 web search 使用）。
- `core/src/event_mapping.rs` 中 `ResponseItem::WebSearchCall` 处理分支（若删除协议字段则同步删除）。
- `core/src/context_manager/history.rs` 中 `WebSearchCall` 相关分支。

**验证：**
- `cargo check -p ody-core` 通过；
- `cargo nextest run -p ody-core` 通过。

**Depends on:** D1.0
**可并行：** 与 R1.1/R1.2 并行。

---

### R1.4 删除 provider/model 能力字段 [normal]

**文件：**
- `model-provider-info/src/lib.rs`：
  - `ProviderCapabilities` 中 `web_search` 字段；
  - `default_provider_capabilities_for_wire_api` 中 web search 默认值；
  - `create_chat_provider` 中 `web_search: false` 初始化。
- `models-manager/src/model_info.rs` 中 provider caps 压制 model caps 的 web search 分支（`resolve_model_capabilities` 内 83-86 行）。
- `protocol/src/model_metadata.rs`：
  - `ModelCapabilities` 中 `supports_search_tool` 字段；
  - `ModelCapabilities` 中 `web_search_tool_type` 字段；
  - `WebSearchToolType` 枚举；
  - `ModelInfo` 顶层 `web_search_tool_type` 字段（若存在）。
- 检查 `models.json` 中是否有 web search 相关字段（当前无）。
- 检查 `model-provider-info/src/model_provider_info_tests.rs`、`models-manager/src/model_info_tests.rs` 中相关测试。

**验证：**
- `cargo check -p ody-model-provider-info -p ody-models-manager -p ody-protocol` 通过；
- 相关测试通过。

**Depends on:** D1.0
**可并行：** 与 R1.1-R1.3 并行。

---

### R1.5 删除 config/feature/CLI 中 web_search_mode [normal]

**文件：**
- `config/src/config_toml.rs` 中 `web_search` 字段解析（若存在）。
- `config/src/mod.rs` 中 `resolve_web_search_mode`、`resolve_web_search_config`、`web_search_mode` 相关配置项。
- `config/src/config_tests.rs` 中大量 web_search_mode 测试（需要删除或迁移）。
- `config/src/config_requirements.rs` 中 `allowed_web_search_modes` 相关。
- `features/src/lib.rs` 中 `Feature::WebSearchCached`、`Feature::WebSearchRequest`、`Feature::StandaloneWebSearch`。
- `cli/src/doctor.rs` 中 `interactive.web_search` 强制 live 逻辑。
- `session/mod.rs`、`session/review.rs`、`session/turn_context.rs` 中 `web_search_mode` 相关 per-turn 配置。
- `tasks/review.rs` 中 web search mode 设置。
- `core/src/tools/spec_plan.rs` 中 `turn_context.config.web_search_mode` 引用（已在 R1.3 处理）。

**验证：**
- `cargo check -p ody-config -p ody-features -p ody-cli -p ody-core` 通过；
- 相关测试通过。

**Depends on:** D1.0
**可并行：** 与 R1.1-R1.4 并行。

---

### I2.1 新增 WebSearchProvider trait 与 WebSearchResult [plan]

**文件（取决于 D1.0 决策，假设新建 `ody-web-search` crate）：**
- `crates/ody-web-search/Cargo.toml`
- `crates/ody-web-search/src/lib.rs`
- `crates/ody-web-search/src/provider.rs`（定义 `WebSearchProvider` trait、 `WebSearchResult` struct）

**接口形状（对齐 TS）：**
```rust
pub struct WebSearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub date: Option<String>,
    pub content: Option<String>,
}

#[async_trait]
pub trait WebSearchProvider: Send + Sync + std::fmt::Debug {
    fn name(&self) -> &str;
    async fn search(
        &self,
        query: &str,
        options: WebSearchOptions,
    ) -> Result<Vec<WebSearchResult>, WebSearchError>;
}

pub struct WebSearchOptions {
    pub limit: Option<u32>,
    pub include_content: Option<bool>,
    pub tool_call_id: Option<String>,
}
```

**验证：**
- `cargo check -p ody-web-search` 通过。

**Depends on:** D1.0

---

### I2.2 新增 provider 注册表 + fallback + 错误分类 [plan]

**文件：**
- `crates/ody-web-search/src/registry.rs`：`WebSearchProviderRegistry`、`create_default_registry`。
- `crates/ody-web-search/src/fallback.rs`：`FallbackWebSearchProvider`。
- `crates/ody-web-search/src/error.rs`：`WebSearchError`、`classify_search_error`（对齐 TS 错误分类）。
- `crates/ody-web-search/src/config.rs`：provider config schema（`WebSearchProviderName`、`WebSearchProviderConfig`）。

**验证：**
- 注册表可注册全部 12 个 provider；
- fallback 在 primary 失败时调用 secondary；
- 单元测试覆盖 unknown provider 抛错、fallback 切换。

**Depends on:** I2.1

---

### I3.1-I3.12 逐个实现 12 个 provider [normal]

每个 provider 一个子任务，文件 `crates/ody-web-search/src/providers/<name>.rs`，对应测试 `crates/ody-web-search/src/providers/<name>_tests.rs`。

| # | Provider | 主要参考文件 | 特殊依赖 |
| --- | --- | --- | --- |
| I3.1 | DuckDuckGo | `packages/agent-core/src/tools/providers/web-search/duckduckgo.ts` | HTML/JSON 解析 |
| I3.2 | Bing | `packages/agent-core/src/tools/providers/web-search/bing.ts` | MS Cognitive API |
| I3.3 | SerpApi | `packages/agent-core/src/tools/providers/web-search/serpapi.ts` | serpapi.com |
| I3.4 | SearchApi | `packages/agent-core/src/tools/providers/web-search/searchapi.ts` | searchapi.io |
| I3.5 | Serper | `packages/agent-core/src/tools/providers/web-search/serper.ts` | serper.dev |
| I3.6 | Baidu | `packages/agent-core/src/tools/providers/web-search/baidu.ts` | baidu.com |
| I3.7 | Serply | `packages/agent-core/src/tools/providers/web-search/serply.ts` | serply.io |
| I3.8 | SearXNG | `packages/agent-core/src/tools/providers/web-search/searxng.ts` | self-host |
| I3.9 | Tavily | `packages/agent-core/src/tools/providers/web-search/tavily.ts` | tavily.com |
| I3.10 | Exa | `packages/agent-core/src/tools/providers/web-search/exa.ts` | exa.ai |
| I3.11 | Perplexity | `packages/agent-core/src/tools/providers/web-search/perplexity.ts` | perplexity.ai |
| I3.12 | Moonshot | `packages/agent-core/src/tools/providers/web-search/moonshot.ts` + `packages/agent-core/src/tools/providers/moonshot-web-search.ts` | Kimi 搜索 API |

**验证：**
- 每个 provider 有独立单元测试（使用 mock HTTP transport）；
- 结果规范化通过 `normalize_result` / `normalize_results`。

**Depends on:** I2.2
**可并行：** 各 provider 之间可并行，但建议分 2-3 波交付以降低 review 负担。

---

### I4.1 新增 `services.webSearch` 配置 schema [plan]

**文件：**
- `config/src/config_toml.rs` 中新增 `services: Option<ServicesConfig>`，其中 `web_search: Option<WebSearchConfig>`。
- 或 `config/src/services.rs` 新文件。
- `config/src/mod.rs` 中解析逻辑。
- 配置 schema 对齐 TS：
  ```rust
  pub struct WebSearchConfig {
      pub primary: WebSearchProviderConfig,
      pub secondary: Option<WebSearchProviderConfig>,
  }
  pub struct WebSearchProviderConfig {
      pub provider: String,
      pub api_key: Option<String>,
      pub timeout_ms: Option<u64>,
      pub options: serde_json::Map<String, serde_json::Value>,
  }
  ```
- 兼容旧 `web_search_mode` 的迁移：若检测到旧配置，发出警告并提示用户改用 `services.webSearch`。

**验证：**
- `cargo test -p ody-config` 配置解析测试通过；
- 旧配置迁移测试通过。

**Depends on:** D1.0
**可并行：** 与 I2.x 并行。

---

### I4.2 实现 app-server 中 provider 解析与注入 [plan]

**文件：**
- `app-server/src/extensions.rs`（或新注入点）：根据 `Config.services.webSearch` 创建 provider 实例并注入到工具系统。
- 若沿用 extension 架构：新建 `WebSearchExtension`（或重写原 `ext/web-search`），在 `on_thread_start` 中解析配置、创建 provider、通过 `ToolContributor` 注册 `WebSearchTool`。
- 若采用 core 直接读取配置：在 `core/src/tools/spec_plan.rs` 的 `add_tool_sources` 中新增 `add_web_search_tool`。

**推荐方案（对齐 TS host 注入）：** app-server 创建 provider，通过 extension 或新机制注入 core。

**验证：**
- `cargo check -p ody-app-server` 通过；
- 未配置 `services.webSearch` 时 `WebSearchTool` 不注册；
- 配置后 provider 正确解析。

**Depends on:** I2.1, I4.1

---

### I5.1 实现 `WebSearchTool` 并接入工具规划 [plan]

**文件：**
- 若作为 core tool：`core/src/tools/builtin/web_search.rs`（新增） + `core/src/tools/spec_plan.rs` 中注册。
- 若作为 extension tool：`ext/web-search/src/tool.rs` + `ext/web-search/src/extension.rs`（重写）。

**工具接口（对齐 TS）：**
- 工具名：`WebSearch`。
- 输入 schema：`{ query: string, limit?: number, include_content?: boolean }`。
- 输出：格式化文本，包含 Title/Date/URL/Snippet/Content。

**验证：**
- `cargo test -p ody-core` 中工具执行测试通过；
- 无 provider 时不注册工具；
- 有 provider 时模型可见 `WebSearch`。

**Depends on:** I2.2, I4.2

---

### I6.1 TUI chip 渲染 [normal]

**文件：**
- `tui/src/chatwidget/components/tool_chips.rs`（或现有工具结果渲染位置）新增 `WebSearch` chip。
- 对齐 TS `webSearchChip`：按结果列表项计数，输出 `N results` / `no results` / `web result`。
- 若旧 `WebSearchItem` 已删除，同步移除专用 `WebSearch` 渲染分支。

**验证：**
- `cargo test -p ody-tui` 相关测试通过；
- 渲染结果与 TS 一致。

**Depends on:** I5.1

---

### T7.1 provider 单元测试 [normal]

每个 provider 在 I3.x 中已自带单元测试。本阶段做整体回归：
- `cargo nextest run -p ody-web-search` 全部通过。

**Depends on:** I3.x

---

### T7.2 registry/fallback 测试 [normal]

**文件：**
- `crates/ody-web-search/src/registry_tests.rs`
- `crates/ody-web-search/src/fallback_tests.rs`

**覆盖：**
- 注册表 unknown provider 报错；
- primary 成功时不用 secondary；
- primary 失败时 fallback 到 secondary；
- 两者都失败时返回错误分类消息。

**Depends on:** I2.2

---

### T7.3 工具执行与配置解析测试 [normal]

**文件：**
- `core/src/tools/web_search_tests.rs`（或 extension 对应测试）
- `config/src/web_search_tests.rs`

**覆盖：**
- 无 provider 时不注册 `WebSearch`；
- 有 provider 时 schema 正确；
- 工具执行返回格式化文本；
- 空结果返回 `No search results found.`；
- 错误返回分类前缀（`Search timed out:`、`Search failed (network):` 等）。

**Depends on:** I5.1, I4.1

---

### T7.4 TS-Rust parity 测试 [plan]

**文件：**
- `packages/integration-tests/src/parity/scenarios/web-search.ts` 扩展或新增 Rust 侧对应；
- 或在 `e:/ody-rs` 新建 `integration-tests/`  parity 测试。

**覆盖：**
- 同一配置下 TS 与 Rust 工具 schema 一致；
- 同一 mock 搜索结果下输出文本一致；
- fallback 行为一致。

**Depends on:** I5.1, I6.1

---

## 依赖图

```
D1.0 (design)
│
├─ R1.1 ─ R1.2 ─ R1.3 ─ R1.4 ─ R1.5   [删除旧实现，可并行]
│
├─ I2.1 ─ I2.2 ─ I3.1..I3.12           [新抽象 + provider 实现]
│         │
│         └─ I4.2 ─ I5.1 ─ I6.1        [配置 → 注入 → 工具 → UI]
│
├─ I4.1 ────────────────────────────── [配置 schema，与 I2.x 并行]
│
└─ T7.1 ─ T7.2 ─ T7.3 ─ T7.4           [测试，串行]
```

---

## 风险与前置假设

1. **旧代码无隐藏使用方**  
   假设 `ody_api::SearchClient`、`WebSearchItem`、`web_search_mode` 等仅被 web search 使用。删除前必须全 workspace grep 确认。
2. **TUI 协议字段迁移**  
   若 `WebSearchItem` 被持久化到 thread history 或 session log，直接删除会导致旧数据无法渲染。需要决定是降级为纯文本显示，还是保留一个兼容解析层。
3. **配置迁移**  
   现有用户可能有 `web_search = true` 或 `web_search_mode = "live"` 配置。需要明确是静默忽略、报错，还是自动迁移到 `services.webSearch`。
4. **Provider 实现工作量**  
   12 个 provider 每个都有独特的 HTTP API、认证、结果格式。即使彼此独立，总工作量仍然很大，建议分两批：第一批 DuckDuckGo + Bing + SerpApi + Moonshot；第二批其余 8 个。
5. **Host 注入语义**  
   Rust 的 app-server 就是 TS 中的 host。需要确保 provider 实例可以在 thread start 时根据配置创建，并且错误地配置 provider 不会导致整个线程崩溃。
6. **OAuth 后续补齐**  
   TS 部分 provider 支持 OAuth（`resolveOAuthTokenProvider`），Rust 第一期可以只做 API key，OAuth 作为后续 roadmap。

---

## 验收标准

- `cargo nextest run` 全 workspace 通过（或至少修改的 crates 通过）。
- 内置 Kimi/DeepSeek/GLM provider 不再依赖 `web_search` 能力字段；web search 只由 `services.webSearch` 配置控制。
- 配置 `services.webSearch` 后，模型可见 `WebSearch` 工具；未配置时不可见。
- 工具执行输出文本与 TS 版格式一致（Title/URL/Snippet）。
- TUI 工具结果 chip 显示 `N results` 或 `no results`。
- 旧 `web_search_mode`、`web/run`、`web_search` hosted tool 无残留引用。

---

## 自我检查

- [x] 每个超大阶段已拆分；删除与新建分离。
- [x] 每个子阶段 ≤~8 文件或独立模块。
- [x] `Depends on` 均指向更早子阶段，并有源码 grep 支撑。
- [x] 每个子阶段有唯一模式标签。
- [x] 顶层 rubric 存在，便于后续编辑保持一致。
- [x] 已标注可并行项与串行瓶颈（D1.0 是全局瓶颈）。
