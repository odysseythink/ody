# `apply_patch` 写文件频繁报错问题分析：与 `ody-code`（TS 实现）的对比

**日期**: 2026-07-21  
**相关 session**: `C:\Users\hkb819\.ody-code\sessions\2026\07\20\rollout-2026-07-20T17-58-40-019f7ef6-a1f3-7c10-a893-e39814a743a4.jsonl`  
**涉及模块**: `ody_apply_patch`, `core/src/tools/handlers/apply_patch`, `core/src/tools/handlers/apply_patch_spec`

---

## 1. 问题现象

在该 session 的最后一条用户消息中，要求把问题分析和长期设计改进保存到 `.ody-code/reports/`。模型随后发起了大量写文件操作：

- `apply_patch`：27 次
- `read_file`：9 次
- `grep`：2 次
- 其中 `apply_patch` 失败 7 次

最终耗时约 10 分钟才完成一份报告的写入。

---

## 2. 根因分析

### 2.1 前几次失败：patch 边界标记格式错误

| 调用行 | 补丁首行 | 补丁末行 | 错误信息 |
|---|---|---|---|
| 1209 | `*** Add File: ...` | `+*** End of File` | `The first line of the patch must be '*** Begin Patch'` |
| 1214 | `*** Begin Patch` | `+*** End of File` | `The last line of the patch must be '*** End Patch'` |
| 1219 | `*** Begin Patch` | `+*** End Patch` | `The last line of the patch must be '*** End Patch'` |
| 1230 | `*** Begin Patch` | `+*** End Patch` | `The last line of the patch must be '*** End Patch'` |

模型把 `*** End Patch` 和 `*** End of File` 也当成了文件内容行，给它们加了 `+` 前缀。

### 2.2 解析器严格检查边界

`apply-patch/src/parser.rs:254-271`：

```rust
fn check_start_and_end_lines_strict(
    first_line: Option<&&str>,
    last_line: Option<&&str>,
) -> Result<(), ParseError> {
    let first_line = first_line.map(|line| line.trim());
    let last_line = last_line.map(|line| line.trim());

    match (first_line, last_line) {
        (Some(first), Some(last)) if first == BEGIN_PATCH_MARKER && last == END_PATCH_MARKER => Ok(()),
        (Some(first), _) if first != BEGIN_PATCH_MARKER => Err(InvalidPatchError(String::from(
            "The first line of the patch must be '*** Begin Patch'",
        ))),
        _ => Err(InvalidPatchError(String::from(
            "The last line of the patch must be '*** End Patch'",
        ))),
    }
}
```

`trim()` 只去掉首尾空白，不会去内容前缀 `+`，所以 `+*** End Patch` 不等于 `*** End Patch`。

### 2.3 工具规格

`core/src/tools/handlers/apply_patch_spec.rs:8-30`：

```text
The patch text must be exactly:

*** Begin Patch
[one or more file sections]
*** End Patch

A file section is one of:

*** Add File: path/to/file
+every line of the new file, each prefixed with +
```

模型误把 `*** End of File`（更新 hunk 的 EOF 标记）当作 `Add File` 的结束符。

### 2.4 后续 workaround 导致更多调用

第 1306 行尝试一次大 `Add File`，但末尾用了 `*** End of File`（不加 `+`），解析器报错：

```text
invalid hunk at line 168, '*** End of File' is invalid hunk
```

之后模型改为：

1. 先 `Add File` 只写标题；
2. 再用多次 `Update File` 追加内容；
3. 分段追加导致内容重复（第 1/2 行、第 83/84 行、第 322/323 行）；
4. 又用 `read_file`/`grep`/`apply_patch` 修复重复。

这形成了“格式错误 → 试错 → 分块 → 重复 → 修复”的连锁反应。

---

## 3. `ody-code`（TS 实现）为什么表现更好？

`D:\workspace\go_work\ody-code` 使用的是两套独立的简单工具：

- `Write`：直接写入/覆盖/追加原始内容
- `Edit`：基于 `old_string` / `new_string` 的精确字符串替换

### 3.1 `Write` 工具

`packages/agent-core/src/tools/builtin/file/write.ts`：

```ts
export const WriteInputSchema = z.object({
  path: z.string(),
  content: z.string(),
  mode: z.enum(['overwrite', 'append']).optional(),
});
```

实现就是：

```ts
if (mode === 'append') {
  await this.agent.kaos.writeText(safePath, args.content, { mode: 'a' });
} else {
  await this.agent.kaos.writeText(safePath, args.content);
}
```

模型只需把文件内容作为普通字符串传入，不需要 `*** Begin Patch` / `*** End Patch`，也不需要 `+` 前缀。

### 3.2 `Edit` 工具

`packages/agent-core/src/tools/builtin/file/edit.ts`：

```ts
export const EditInputSchema = z.object({
  path: z.string(),
  old_string: z.string().min(1),
  new_string: z.string(),
  replace_all: z.boolean().optional(),
});
```

实现是简单的字符串替换：

```ts
const newContent = replaceOnceLiteral(content, args.old_string, args.new_string);
await this.agent.kaos.writeText(safePath, materializeModelText(newContent, modelView.lineEndingStyle));
```

---

## 4. 两种方式客观对比

| 维度 | `E:\ody-rs`：`apply_patch` | `D:\workspace\go_work\ody-code`：`Write` + `Edit` |
|---|---|---|
| **工具形态** | 一个统一 patch 工具，支持 Add/Update/Delete/Move | 三个独立工具：Read / Write / Edit |
| **创建新文件** | 必须用 `*** Add File` + `+content` + `*** End Patch` | 直接 `Write { path, content }` |
| **LLM 出错概率** | 高：需要记住 patch 边界标记、内容前缀、上下文格式 | 低：`Write` 就是普通字符串；`Edit` 只需复制原文 |
| **典型错误** | `invalid patch: first/last line must be...` | `old_string not found` / `not unique` |
| **错误可定位性** | 较差：只告诉边界不对，不提示 `+` 前缀 | 较好：直接说哪段字符串找不到 |
| **多文件操作** | 一份 patch 可改多个文件 | 每个文件需一次调用（可并行） |
| **大文件处理** | 单个大字符串 patch，格式失败则整体作废 | 可用 `append` 模式分段 |
| **原子性** | 整份 patch 验证阶段要么全成要么全败 | 每次调用独立 |

---

## 5. 结论

`E:\ody-rs` 写文件频繁报错的根因是：**模型对 `apply_patch` 的格式理解错误，把 `*** End Patch` / `*** End of File` 写进了文件内容行**，而解析器只检查首尾行是否严格匹配，不提示 `+` 前缀问题，导致模型反复试错。

`ody-code`（TS）之所以表现更好，是因为它使用了 `Write`/`Edit` 这种更简单的接口，模型不需要学习 patch 格式，创建/修改文件的成功率更高，自然也不会因为写失败而连锁产生大量修复调用。

---

## 6. 建议

1. **改进 `apply_patch` 错误提示**：在边界检查失败时，如果检测到最后一行是 `+*** End Patch` 或 `+*** End of File`，明确提示“结束标记不应带 `+` 前缀”。
2. **增加 `Write`/`Edit` 式工具**：对于创建新文件或完整覆盖，提供一个直接写原始内容的工具，绕过 patch 格式。
3. **统一 patch 格式教育**：在工具描述中明确说明 `*** End of File` 只用于 `Update File` 的 EOF，不用于 `Add File`。
4. **考虑减少大 patch 的格式脆弱性**：例如允许在 `Add File` 内容中忽略对 `***` 行的解析，直到遇到真正的 `*** End Patch`。
