# Part 5 — `WebSearchTool` 执行与 TUI chip 渲染

## 来源 [C:UPSTREAM]

- TS: `packages/agent-core/src/tools/builtin/web/web-search.ts:65-124` — `WebSearchTool` 实现、输入 schema、输出格式化、错误处理。
- TS: `packages/agent-core/src/tools/builtin/web/web-search.ts:40-59` — `WebSearchInputSchema`（`query`、`limit`、`include_content`）。
- TS: `apps/ody-code/src/tui/components/messages/tool-renderers/chip.ts:103-111` — `webSearchChip` 计数规则。
- Rust 现状：`ext/web-search/src/tool.rs` — 旧的 `web/run` namespace tool；`tui/src/chatwidget/tool_lifecycle.rs:70-109` — `on_web_search_begin` / `on_web_search_end` 专用事件；`tui/src/chatwidget/protocol.rs:294-296` — `ThreadItem::WebSearch` 路由。

## 工具实现位置 [C:USER]

`WebSearchTool` 放在 `ext/web-search/src/tool.rs`（重写）。原因：
- 工具执行需要 `ody_extension_api::ToolExecutor` / `ody_extension_api::ToolCall`；
- 工具被 `ext/web-search/src/extension.rs` 的 `ToolContributor` 注册；
- `ody-core` 通过 trait object 调用，不直接依赖 `ody-web-search` 的具体类型。

### 工具定义

```rust
use std::sync::Arc;

use ody_extension_api::FunctionCallError;
use ody_extension_api::ToolCall;
use ody_extension_api::ToolExecutor;
use ody_extension_api::ToolExecutorFuture;
use ody_extension_api::ToolName;
use ody_extension_api::ToolOutput;
use ody_extension_api::ToolSpec;
use ody_extension_api::ToolExposure;
use ody_extension_api::parse_tool_input_schema;
use ody_tools::ResponsesApiTool;
use ody_tools::ResponsesApiNamespace;
use ody_tools::ResponsesApiNamespaceTool;
use ody_tools::ToolExposure as ToolExposureNs;
use ody_web_search::{WebSearchOptions, WebSearchProvider, WebSearchError, classify_search_error};
use serde_json::json;

const WEB_SEARCH_TOOL_NAME: &str = "WebSearch";

pub(crate) struct WebSearchTool {
    provider: Arc<dyn WebSearchProvider>,
}

impl WebSearchTool {
    pub(crate) fn new(provider: Arc<dyn WebSearchProvider>) -> Self {
        Self { provider }
    }
}

impl ToolExecutor<ToolCall> for WebSearchTool {
    fn tool_name(&self) -> ToolName {
        ToolName::new(WEB_SEARCH_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        let parameters = parse_tool_input_schema(&json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The query text to search for."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 20,
                    "description": "The number of results to return. Typically you do not need to set this value. When the results do not contain what you need, you probably want to give a more concrete query."
                },
                "include_content": {
                    "type": "boolean",
                    "description": "Whether to include the content of the web pages in the results. It can consume a large amount of tokens when this is set to true. You should avoid enabling this when `limit` is set to a large value."
                }
            },
            "required": ["query"]
        }))
        .expect("WebSearch tool schema is valid JSON");

        ToolSpec::Namespace(ResponsesApiNamespace {
            name: "".to_string(),
            description: "".to_string(),
            tools: vec![ResponsesApiNamespaceTool::Function(ResponsesApiTool {
                name: WEB_SEARCH_TOOL_NAME.to_string(),
                description: include_str!("../web_search_description.md"),
                strict: false,
                parameters,
                output_schema: None,
                defer_loading: None,
            })],
        })
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Direct
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }

    fn handle(&self, call: ToolCall) -> ToolExecutorFuture<'_> {
        Box::pin(self.handle_call(call))
    }
}
```

注意：
- 工具名 `WebSearch`（首字母大写，与 TS 一致）。
- `parse_tool_input_schema` 替代旧 `parse_tool_input_schema_without_compaction`；因为新 schema 简单，无需特殊处理。
- `description` 从 `web_search_description.md` 文件读取（新增，对齐 TS `web-search.md`）。

### 工具执行

```rust
impl WebSearchTool {
    async fn handle_call(&self, call: ToolCall) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
        let args = call.function_arguments()?;
        let input: WebSearchInput = serde_json::from_str(args)
            .map_err(|err| FunctionCallError::RespondToModel(format!("Invalid WebSearch arguments: {err}")))?;

        if input.query.trim().is_empty() {
            return Err(FunctionCallError::RespondToModel("query must be non-empty".to_string()));
        }

        let options = WebSearchOptions {
            limit: input.limit,
            include_content: input.include_content,
            tool_call_id: Some(call.call_id.clone()),
        };

        let results = self
            .provider
            .search(&input.query, options)
            .await
            .map_err(|err| FunctionCallError::Fatal(classify_search_error(&err).to_string()))?;

        Ok(Box::new(WebSearchOutput::new(results)))
    }
}

#[derive(Debug, serde::Deserialize)]
struct WebSearchInput {
    query: String,
    limit: Option<u32>,
    #[serde(rename = "include_content")]
    include_content: Option<bool>,
}
```

### 输出格式化 [C:UPSTREAM]

严格对齐 TS `web-search.ts:96-115`。

```rust
use ody_extension_api::ToolOutput;
use ody_web_search::WebSearchResult;

struct WebSearchOutput {
    text: String,
}

impl WebSearchOutput {
    fn new(results: Vec<WebSearchResult>) -> Self {
        if results.is_empty() {
            return Self {
                text: "No search results found.".to_string(),
            };
        }

        let mut lines = Vec::new();
        let mut first = true;
        for result in results {
            if !first {
                lines.push("---\n".to_string());
            }
            first = false;

            lines.push(format!("Title: {}", result.title));
            if let Some(date) = result.date {
                lines.push(format!("Date: {}", date));
            }
            lines.push(format!("URL: {}", result.url));
            lines.push(format!("Snippet: {}\n", result.snippet));
            if let Some(content) = result.content {
                lines.push(format!("{}\n", content));
            }
        }

        Self {
            text: lines.join("\n"),
        }
    }
}

impl ToolOutput for WebSearchOutput {
    fn text(&self) -> String {
        self.text.clone()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
```

- 无结果时返回固定文本 `No search results found.`。
- 结果之间用 `---\n` 分隔（TS 使用 `---\n\n`，但 Rust join 逻辑等价）。
- `Title`/`Date`/`URL`/`Snippet` 顺序与 TS 一致。
- `content` 若存在，追加在最后。

### 错误处理 [C:UPSTREAM]

- 输入 JSON 解析失败 → `FunctionCallError::RespondToModel`（让模型修正参数）。
- `query` 为空 → `FunctionCallError::RespondToModel`。
- provider 搜索失败 → `FunctionCallError::Fatal` + 分类前缀文本（如 `Search timed out: ...`）。
- 分类函数使用 `classify_search_error`（见 `trait-crate.md`）。

---

## TUI 渲染 [C:UPSTREAM]

### 删除旧 WebSearch 专用事件

旧的 `ThreadItem::WebSearch` 和 `WebSearchAction` 将被删除。因此 TUI 以下代码同步删除：

- `tui/src/chatwidget/tool_lifecycle.rs:70-109`：`on_web_search_begin` / `on_web_search_end`。
- `tui/src/chatwidget/protocol.rs:294-296`：`ThreadItem::WebSearch` 分支。
- `tui/src/history_cell/` 中所有 `WebSearchCell` / `new_web_search_call` / `new_active_web_search_call` 相关代码。
- `app-server-protocol` 中 `WebSearchAction` / `WebSearchItem` 类型定义。

### 通用工具 chip 新增 `WebSearch` [C:UPSTREAM]

对齐 TS `webSearchChip`。

在 Rust 中，通用工具 chip 注册表位于 `tui/src/thread_transcript.rs`（或类似位置）。新增分支：

```rust
fn web_search_chip(tool_call: &ThreadItem, result: &ToolResult) -> String {
    let output = &result.output;
    let non_empty_lines: Vec<&str> = output
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();

    let count = non_empty_lines
        .iter()
        .filter(|line| line.starts_with(|c: char| c.is_ascii_digit()) || line.starts_with("-") || line.starts_with("*"))
        .count();

    if count == 0 {
        if non_empty_lines.is_empty() || output.trim() == "No search results found." {
            return "no results".to_string();
        }
        return "web result".to_string();
    }

    if count == 1 {
        "1 result".to_string()
    } else {
        format!("{count} results")
    }
}
```

- 计数规则与 TS 一致：按行首数字或 `-`/`*` 计数。
- 未匹配时：若输出为空或 `No search results found.` 显示 `no results`；否则显示 `web result`。
- 在 chip 注册表中按工具名 `WebSearch` 注册。

### 活动工具状态

- 新 `WebSearchTool` 作为普通工具调用，开始和结束通过通用 `DynamicToolCall` 或 `ToolCall` 事件呈现。
- 不再需要 `WebSearchCell` 专用动画单元；使用通用工具调用进度动画。

---

## 复用分析 [C:INFERRED]

- 复用 `ody_extension_api::ToolExecutor` 和 `ody_tools::ResponsesApiTool` 定义工具。
- 复用通用工具 chip 注册表机制（`tui/src/thread_transcript.rs`）。
- 不复用 `ody_protocol::models::WebSearchAction` / `ody_protocol::items::WebSearchItem`；删除。
- 不复用旧 `web_run_description.md`；替换为 `web_search_description.md`。

---

## 风险与决策

| 决策 | 选择 | 理由 |
|---|---|---|
| 工具放在 `ext/web-search` 还是 `core` | `ext/web-search` | 与注入点同 crate，减少 `ody-core` 依赖。 |
| 工具 schema 是否用 `ResponsesApiNamespace` | 是 | 与现有 extension tool 一致。 |
| 输出格式是否严格与 TS 一致 | 是 | 便于 parity 测试和 chip 计数。 |
| 错误是否返回 `FunctionCallError::Fatal` | 是 | 与 TS 将错误作为 `isError: true` 返回一致。 |
| 是否保留 `WebSearch` 专用 TUI cell | 否 | 删除协议字段后无数据来源；使用通用工具输出。 |
| chip 计数是否复刻 TS 规则 | 是 | 按数字/`-`/`*` 开头行计数；输出非列表时显示 `web result`。 |

---

## 测试要点 [C:INFERRED]

- 工具 schema 正确性：通过 `ToolExecutor::spec()` 断言参数列表。
- 输入解析：缺失 `query` 报错；`limit` 为非整数/越界报错；`include_content` 非布尔报错。
- 输出格式化：空结果 → `No search results found.`；单结果和多结果格式正确；content 字段正确追加。
- 错误分类：timeout/network/auth 等映射到正确 `FunctionCallError::Fatal` 文本。
- TUI chip：输出空 → `no results`；输出非列表 → `web result`；输出 3 个列表项 → `3 results`。

---

## 文件改动清单 [C:INFERRED]

- `ext/web-search/src/tool.rs`：完全重写为 `WebSearchTool`。
- `ext/web-search/web_search_description.md`：新增（替换 `web_run_description.md`）。
- `ext/web-search/web_run_description.md`：删除。
- `tui/src/thread_transcript.rs`：新增 `WebSearch` chip 分支。
- `tui/src/chatwidget/tool_lifecycle.rs`：删除 `on_web_search_begin` / `on_web_search_end`。
- `tui/src/chatwidget/protocol.rs`：删除 `ThreadItem::WebSearch` 分支。
- `tui/src/history_cell/mod.rs` 及相关文件：删除 `WebSearchCell` 相关函数。
- `app-server-protocol`：删除 `WebSearchItem` / `WebSearchAction` 类型。
