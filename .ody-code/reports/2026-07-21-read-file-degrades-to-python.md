# 读文件退化为 Python 的原因分析

**日期**: 2026-07-21  
**相关 session**: `C:\Users\hkb819\.ody-code\sessions\2026\07\21\rollout-2026-07-21T10-19-44-019f8278-d4b7-7b20-99c0-7ca4d0876961.jsonl`  
**涉及模块**: `core/src/tools/handlers/file_tools`, `packages/agent-core/src/tools/policies/path-access`, `packages/agent-core/src/tools/builtin/file/read.ts`

---

## 1. 问题现象

在该 session 中，模型为了分析写文件问题，需要读取并解析 session JSONL 日志。结果：

- `shell_command`：27 次
- 其中 Python 脚本：13 次
- `read_file`：5 次
- `grep`：3 次

也就是说，**接近一半的 shell 调用是用来写 Python 读文件的**。

---

## 2. 根因分析

### 2.1 第一次 `read_file` 就被工作区隔离拒绝

第 13 行调用：

```json
{"path":"C:\Users\hkb819\.ody-code\sessions\2026\07\20\rollout-2026-07-20T17-58-40-019f7ef6-a1f3-7c10-a893-e39814a743a4.jsonl"}
```

第 14 行返回：

```text
path `C:\Users\hkb819\.ody-code\sessions\2026\07\20\rollout-...a743a4.jsonl`
escapes the working directory `E:\ody-rs`.
File tools are confined to the workspace; use shell_command if you genuinely need to read outside it.
```

### 2.2 解析器对工作区外路径严格拒绝

`core/src/tools/handlers/file_tools/mod.rs:76-100`：

```rust
fn confine_to_root(
    root: &AbsolutePathBuf,
    candidate: AbsolutePathBuf,
) -> Result<AbsolutePathBuf, FunctionCallError> {
    let normalized = lexically_normalize(candidate.as_path());
    if !normalized.starts_with(root.as_path()) {
        return Err(FunctionCallError::RespondToModel(format!(
            "path `{}` escapes the working directory `{}`. File tools are confined to the \
             workspace; use shell_command if you genuinely need to read outside it.",
            normalized.display(),
            root.as_path().display()
        )));
    }
    ...
}
```

这里用的是 `starts_with(root.as_path())`：只要路径不在 `E:\ody-rs` 下面，**不管是相对还是绝对路径，一律拒绝**。

### 2.3 即便能读，`read_file` 也不适合结构化 JSONL 解析

第 42 行的 reasoning 说明了为什么选 Python：

```text
The output is huge and truncated at 185540 chars. We need the exact list of attempts.
We can parse the JSONL for events of type response_item with function_call or function_call_output for that turn.
Use jq or Python to extract.
```

`read_file` 返回的是带行号的原始文本，`grep` 只能做正则匹配。要完成以下任务，必须靠脚本：

- 按 JSON 字段过滤（`type == "response_item"`）
- 按 `turn_id` 过滤
- 统计、聚合
- 解码内嵌的 patch 参数
- 比较每份 patch 的首尾行

---

## 3. `ody-code`（TS 实现）为什么很少出现这种情况？

### 3.1 路径策略允许读绝对路径 outside workspace

`packages/agent-core/src/tools/policies/path-access.ts`：

```ts
export const DEFAULT_WORKSPACE_ACCESS_POLICY: WorkspaceAccessPolicy = {
  guardMode: 'absolute-outside-allowed',
  checkSensitive: true,
};
```

`Read` 工具使用默认策略：

```ts
const path = resolvePathAccessPath(args.path, {
  kaos: this.kaos,
  workspace: this.workspace,
  operation: 'read',
});
```

这意味着：**绝对路径指向工作区外是被允许的**。session 日志在 `C:\Users\...` 下，可以直接用 `Read` 读。

### 3.2 文件工具本身更简单

`D:\workspace\go_work\ody-code` 使用 `Read`/`Grep`/`Glob`：

- `Read`：行号 + 内容，带分页
- `Grep`：ripgrep 正则搜索，可返回文件或内容
- `Glob`：文件模式匹配

虽然也没有原生 JSON 解析能力，但至少**读工作区外文件不需要先跳到 shell**。

### 3.3 写失败少，读日志的需求本身就少

`ody-code` 使用 `Write`/`Edit`，写文件成功率高，模型不需要频繁解析 session 日志来排查“为什么写了 27 次才成功”。读日志的需求减少，退化为 Python 的机会也随之减少。

---

## 4. 两种方式客观对比

| 维度 | `E:\ody-rs`（Rust 客户端） | `D:\workspace\go_work\ody-code`（TS 客户端） |
|---|---|---|
| **核心读工具** | `read_file`、`grep`、`glob` | `Read`、`Grep`、`Glob` |
| **工作区外路径** | 严格拒绝，即使绝对路径也不行 | 默认 `absolute-outside-allowed`，绝对路径可读 |
| **读 session 日志** | 必须先走 `shell_command`，再写 Python | 可直接用 `Read` |
| **返回格式** | 行号 + 内容，带分页提示 | 行号 + 内容，带 `<system>` 状态块 |
| **单文件上限** | 1000 行 / 2000 字符/行 / 100 KiB | 1000 行 / 2000 字符/行 / 100 KiB |
| **结构化查询** | 不支持，需要 Python/shell | 不支持，需要 Python/shell |
| **工具描述** | 只说“Prefer over cat/sed/rg” | 明确禁止直接用 shell grep/rg |
| **并发读取** | 未强调 | 鼓励一次响应并行多个 `Read` |

---

## 5. 结论

`E:\ody-rs` 读文件退化为 Python 的核心原因不是 `read_file` 本身设计差，而是**路径隔离策略太严格**：`read_file`/`grep` 被硬限制在 `E:\ody-rs` 内，而 session 日志、用户配置等常见数据都在外部，模型只能用 `shell_command`，进而用 Python 做结构化解析。

`ody-code`（TS）的 `Read` 工具默认允许绝对路径读工作区外，所以能直接处理这类路径；加上它的 `Write`/`Edit` 更稳定，写失败少，读日志排查的需求也少了。

---

## 6. 建议

1. **放宽文件工具的路径策略**：允许绝对路径读取工作区外文件（如 TS 的 `absolute-outside-allowed`），同时保留敏感文件检查。
2. **提供结构化日志查询工具**：例如 `jsonl_filter` 或 `jq`，支持按字段、类型、turn_id 过滤 session 日志，避免模型每次写 Python。
3. **在工具描述中明确路径规则**：如果 `read_file` 只能读工作区内，要在描述里提前说明，让模型知道该用 shell。
4. **减少写文件失败率**：通过 `Write`/`Edit` 或改进 `apply_patch` 错误提示，降低模型需要回头读日志排查的概率。
