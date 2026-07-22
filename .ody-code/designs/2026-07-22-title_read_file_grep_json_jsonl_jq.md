# Title: 强化 read_file/grep 工具描述，让 JSON/JSONL 优先使用 jq

## Scope In [C:USER]
- 修改 `core/src/tools/handlers/file_tools_spec.rs` 中 `create_read_file_tool` 的 `description`，明确对 `.json` / `.jsonl` 文件应优先使用 `jq`。
- 修改同一文件中 `create_grep_tool` 的 `description`，明确对 `.json` / `.jsonl` 文件应优先使用 `jq` 的 `filter` / `count` 能力，而不是用 `grep` 做文本匹配。
- 保持现有工具行为、参数、权限、容量限制不变。

## Scope Out [C:USER]
- 不修改 `jq` 工具本身（当前代码已修复）。
- 不合并 `read_file` / `jq` / `grep` 工具。
- 不修改 `PathAccessMode`、`MAX_BYTES` 等权限或容量配置。
- 不修改 prompt 模板（本次仅修工具描述；若验证后仍无效，再追加 prompt 模板修改）。

## Architecture / Design [C:INFERRED]
- 仅改动 `core/src/tools/handlers/file_tools_spec.rs` 中两个描述字符串：
  - `create_read_file_tool` 的 description 在现有末尾追加一句：`For JSON or JSONL files, prefer `jq` for filtering, counting, or paging.`
  - `create_grep_tool` 的 description 在现有末尾追加一句：`For JSON or JSONL files, prefer `jq` (`filter` or `count`) over text search; it understands structure and can page outside the workspace.`
- 现有单元测试 `jq_prefers_itself_for_json_files` 继续保护 `jq` 描述。
- 可选：新增两个轻量测试断言 `read_file` 和 `grep` 描述中包含 "JSON or JSONL" 或 "prefer `jq`"，防止回退。

## Data Models [C:INFERRED]
- 无变更。仅字符串描述改动。

## Algorithms [C:INFERRED]
- 无变更。无新算法。

## Error Handling / Degradation [C:INFERRED]
- 无新增失败模式。
- 若模型仍忽略描述，行为回退到当前状态：`grep` 对工作区外路径失败，`read_file` 大文件被截断，与当前一致。
- 工具描述变更不会导致编译失败或运行时错误。

## Self-Review [C:INFERRED]
- 最昂贵的决策：模型是否会因为多了一句话就改变工具选择。该风险可控，因为改动前已有 `jq` 描述说“Prefer this over `read_file`”，但模型仍选了 `read_file`；说明需要在 `read_file`/`grep` 侧也反向提示，形成双向约束。
- 安全：无权限变更，无新攻击面。
- 测试：已有 `jq_prefers_itself_for_json_files`；新增描述断言测试成本低。
- 运维：无配置、无迁移、无文档更新需求。
- 集成：工具描述直接注入给模型，接口无变化。
- 范围复核：用户明确“只修工具选择问题”，本设计严格遵守 Scope Out，不做无关改动。

## Reuse Analysis [C:INFERRED]
- 复用 `core/src/tools/handlers/file_tools_spec.rs` 中现有的 `create_read_file_tool`、`create_grep_tool` 和 `spec_json` 测试辅助函数。
- 无需新建模块或新依赖。

## Assumptions & Unverified Items
| # | Assumption | Confidence | Impact if wrong | How to verify |
|---|---|---|---|---|
| 1 | 模型会阅读并遵循工具描述中的“优先使用 jq”提示 | medium | 若模型忽略，修描述无效 | 修改后重跑相同 prompt，观察第一个工具调用是否为 `jq` |
| 2 | 仅在 `read_file`/`grep` 描述中追加一句反向提示即可，无需同时修改 prompt 模板 | medium | 可能需要同时改 prompt 模板才能完全解决 | 先验证工具描述改动，若无效再追加 prompt 模板修改 |
| 3 | `jq` 当前描述已足够，不需要再调整 | high | 重复或冲突的指引会让模型困惑 | 代码审查确认 `jq` 描述仍包含 "Prefer this over `read_file`" |

## User Approval [C:USER]
- 用户已通过 `request_user_input` 选择“再强化 read_file/grep 描述”方向。
- 本设计仅修改工具描述字符串，符合用户“只修工具选择问题”的约束。
