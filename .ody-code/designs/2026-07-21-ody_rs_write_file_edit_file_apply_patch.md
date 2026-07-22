# 为 `ody-rs` 新增 `write_file` / `edit_file` 工具并改进 `apply_patch` 错误提示

## Scope In / Scope Out

**Scope In [C:USER]**

- 新增 `write_file` 工具：支持创建、覆盖、追加本地文件，自动创建父目录。
- 新增 `edit_file` 工具：支持基于 `old_string` / `new_string` 的精确字符串替换，支持 `replace_all` 可选参数。
- 两个工具均纳入 `file_tools` 模块，与 `read_file` / `grep` / `glob` 共享本地文件系统解析逻辑。
- 两个工具写入完成后发出与 `apply_patch` 同质的 `FileChange` 事件，供 UI 和 diff 追踪器统一展示。
- 改进 `apply_patch` 解析错误提示：当检测到 `+*** End Patch` 或 `+*** End of File` 时，明确提示结束标记不应带 `+` 前缀；同时改进其他常见格式错误的提示。
- 默认启用，无需特性开关。

**Scope Out [C:USER]**

- 不支持远程环境（与现有 `read_file` 保持一致，后续可单独扩展）。
- 不替代 `apply_patch`；`apply_patch` 继续用于多文件批量、删除、移动等复杂场景。
- 不引入二进制文件写入、`write_file` 不直接支持流式分块写入。
- 不修改 `apply_patch` 的 patch 语法本身，仅改进错误提示和边界检测。

## Architecture Overview

```
┌─────────────────────────────────────────────────────┐
│  Tool Registry / Orchestrator                       │
│  - create_write_file_tool()                          │
│  - create_edit_file_tool()                           │
│  - create_apply_patch_tool() (updated description)   │
└──────────────┬──────────────────────────────────────┘
               │
               ▼
┌─────────────────────────────────────────────────────┐
│  file_tools module                                  │
│  - ReadFileHandler                                    │
│  - GrepHandler                                        │
│  - GlobHandler                                        │
│  - WriteFileHandler (new)                             │
│  - EditFileHandler (new)                              │
│  - local_search_root / confine_to_root (reused)      │
└──────────────┬──────────────────────────────────────┘
               │
               ▼
┌─────────────────────────────────────────────────────┐
│  Sandbox / Permission layer                         │
│  - confine_to_root                                  │
│  - apply_granted_turn_permissions (reused)          │
└──────────────┬──────────────────────────────────────┘
               │
               ▼
┌─────────────────────────────────────────────────────┐
│  ExecutorFileSystem (local)                           │
│  - write_file / read_file_text / create_directory   │
└──────────────┬──────────────────────────────────────┘
               │
               ▼
┌─────────────────────────────────────────────────────┐
│  Event / Diff stream                                │
│  - FileChange events with source metadata           │
│  - existing PatchApplyUpdated from apply_patch      │
└─────────────────────────────────────────────────────┘
```

## Design Decisions (confirmed)

| Decision | Source | Choice |
|----------|--------|--------|
| Add both tools | [C:USER] | `write_file` + `edit_file`, plus `apply_patch` error-message improvement |
| Data & State | [C:USER] | Independent handlers, emit change events after write |
| Integration | [C:USER] | Inside `file_tools` module, reuse local file resolution |
| Error & Degradation | [C:USER] | Auto-create parent dirs; `edit_file` gives near-text hint on mismatch; multi-match errors out |
| Security | [C:USER] | Reuse `apply_patch` sandbox approval model (`apply_granted_turn_permissions`) |
| Observability | [C:USER] | Emit `FileChange` events compatible with `apply_patch` |
| Operations | [C:USER] | Enabled by default, no feature flag |

## Data Models

### Tool input arguments

```rust
#[derive(Deserialize, Default)]
enum WriteMode {
    #[default]
    Overwrite,
    Append,
}

#[derive(Deserialize)]
struct WriteFileArgs {
    path: String,
    content: String,  // capped at MAX_WRITE_FILE_BYTES
    #[serde(default)]
    mode: WriteMode,
    #[serde(default)]
    environment_id: Option<String>,
}

#[derive(Deserialize)]
struct EditFileArgs {
    path: String,
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
    #[serde(default)]
    environment_id: Option<String>,
}
```

`environment_id` is passed to `local_search_root`, which rejects remote environments and resolves the local path. This matches `read_file` behavior.

### Tool output

Both return `FunctionToolOutput` (reuse `core/src/tools/context.rs:184`).

On success:
- `write_file`: `Wrote {len} bytes to {path}`
- `edit_file`: `Replaced {count} occurrence(s) in {path}`

On failure, both return `FunctionCallError::RespondToModel(message)`.

### Event payload

After success, emit one `FileChange` event. The event type is determined **after** writing, based on whether the file already existed before the write. The event metadata carries a `source` tag (`write_file` or `edit_file`) for telemetry and audit.

```rust
FileChange::Add { content: String }      // file did not exist before this write
FileChange::Update { unified_diff: String, move_path: None } // file existed before this write
```

`FileChange::Delete` is never emitted by these tools.

### Constants

```rust
const MAX_WRITE_FILE_BYTES: usize = 10 * 1024 * 1024; // 10 MiB per call
const MAX_FILE_SIZE_FOR_DIFF: usize = 1024 * 1024;    // 1 MiB: files larger than this get a summary diff
```

## Algorithms

### Atomic write helper

```text
function atomic_write(fs, abs_path, content):
    temp_path = abs_path + ".ody-write-tmp-" + random_uuid()
    fs.write_file(temp_path, content.bytes)
    fs.rename(temp_path, abs_path)
```

- The temp file is created in the same directory as the target, so `rename` is atomic on POSIX and avoids cross-filesystem issues.
- The random suffix prevents symlink-race attacks and avoids collisions with leftover temp files.
- If `rename` is unavailable, fall back to `fs.write_file(abs_path, content.bytes)` and document the loss of crash-atomicity as a known limitation.
- If the temp file already exists before writing (e.g., from a crashed prior run), we delete it first or fail closed.

### Write file

```text
function write_file(abs_path, content, mode):
    if content.len() > MAX_WRITE_FILE_BYTES:
        raise Error("content exceeds 10 MiB limit")

    parent = abs_path.parent()
    if parent is a file: raise ParentIsFileError
    if parent does not exist:
        fs.create_directory(parent, recursive=true)  // not atomic; see TOCTOU note below

    if mode == Append:
        if fs has native append:
            // Preferred: native append avoids reading the whole file.
            fs.append_file(abs_path, content.bytes)
            original_existed = true  // assume file exists; if not, native append creates it
        else:
            // Fallback: read-modify-write with a final-size guard.
            original = fs.read_file_text(abs_path).ok_or_default()
            if original.len() + content.len() > MAX_FILE_SIZE_FOR_DIFF:
                raise Error("append would exceed safe file size")
            new_content = original + content
            atomic_write(fs, abs_path, new_content)
    else: // Overwrite
        // Read original content for diff if the file exists and is small enough.
        original = fs.read_file_text(abs_path).ok_if_small(MAX_FILE_SIZE_FOR_DIFF)
        atomic_write(fs, abs_path, content)

    return
```

`append` creates the file if it does not exist, matching shell `>>` semantics. The native-append path is preferred; the read-modify-write path is only used when the filesystem lacks append and is capped to avoid memory blowup.

### Edit file

```text
function edit_file(abs_path, old_string, new_string, replace_all):
    if old_string.is_empty(): raise Error("old_string cannot be empty")
    if old_string == new_string: return "No changes needed"

    original = fs.read_file_text(abs_path)
    count = non_overlapping_occurrences(original, old_string)
    if count == 0:
        hint = near_text_hint(original, old_string)
        raise Error("old_string not found in {path}. Did you mean:\n{hint}")
    if count > 1 and not replace_all:
        raise Error("old_string appears {count} times in {path}. Set replace_all=true or use apply_patch.")

    if replace_all:
        new_content = original.replace(old_string, new_string)
    else:
        new_content = original.replacen(old_string, new_string, 1)

    atomic_write(fs, abs_path, new_content)
    return new_content
```

**Concurrency note**: `edit_file` is a read-modify-write operation. The final `atomic_write` is atomic, but the read and write are not a single atomic transaction. Concurrent modifications between the read and the write can be silently overwritten. This is the same limitation as editing a file through `apply_patch` (which also reads the original to build a diff and then writes it back). We document this as a known limitation; if strict concurrency safety is required, users should use `apply_patch` or a file-locking workflow.

### Non-overlapping occurrence counting

```rust
fn non_overlapping_occurrences(haystack: &str, needle: &str) -> usize {
    assert!(!needle.is_empty(), "needle must not be empty");
    let mut count = 0;
    let mut start = 0;
    while let Some(pos) = haystack[start..].find(needle) {
        count += 1;
        start += pos + needle.len();
    }
    count
}
```

This matches `String::replace` / `String::replacen` semantics.

### Near-text hint

```text
function near_text_hint(original, old_string):
    if old_string is empty or original is empty: return "(empty)"
    if original.len() > MAX_FILE_SIZE_FOR_DIFF: return "(file too large to generate hint)"
    first_line = first non-empty line of old_string
    candidates = lines in original that contain first_line as substring
    if candidates is empty: candidates = all lines
    best_line = candidate with longest common prefix with old_string
    context = ±3 lines around best_line, capped at 100 lines total
    return "Closest match at line {line_number}:\n```\n{context}\n```"
```

The hint is skipped for large files to avoid performance and privacy issues.

## Error Handling / Degradation

| Scenario | Tool | Behavior | Model-facing message |
|---|---|---|---|
| Path escapes workspace | write_file / edit_file | Error | `path X escapes the working directory Y` |
| Remote environment | write_file / edit_file | Error | `file tools are unavailable for remote environment X` |
| Content exceeds 10 MiB | write_file | Error | `content exceeds 10 MiB limit` |
| Parent path is a file | write_file | Error | `parent path X is a file, cannot create directory` |
| Parent directory missing | write_file | Auto-create recursively | (silent) |
| `mode` is invalid | write_file | Error | `mode must be "overwrite" or "append"` |
| File does not exist for append | write_file | Create then append | (silent) |
| File does not exist for edit | edit_file | Error | `cannot edit: X does not exist` |
| `old_string` is empty | edit_file | Error | `old_string cannot be empty` |
| `old_string` not found | edit_file | Error + hint | `old_string not found in X. Did you mean: ...` |
| `old_string` appears multiple times | edit_file | Error | `old_string appears N times in X. Set replace_all=true or use apply_patch.` |
| `old_string` == `new_string` | edit_file | No-op | `No changes needed` |
| Sandbox rejects write | write_file / edit_file | Propagate approval error | (passthrough from sandbox layer) |
| I/O failure | write_file / edit_file | Error | `unable to write X: <io error>` |
| Non-UTF-8 file | edit_file | Error | `X is not valid UTF-8; edit_file only works on text files` |
| Concurrent modification | edit_file | Known limitation | (documented, no runtime detection in v1) |

## Reuse Analysis

| Existing component | Path | Reuse rationale |
|---|---|---|
|`FileToolOptions` | `core/src/tools/handlers/file_tools_spec.rs:34` | Schema option parity with read_file/grep/glob |
|`local_search_root` / `confine_to_root` | `core/src/tools/handlers/file_tools/mod.rs:40-115` | Same workspace boundary as existing file tools |
|`ToolExecutor<ToolInvocation>` | `core/src/tools/registry.rs:44` | Handler contract already used by every core tool |
|`FunctionToolOutput` | `core/src/tools/context.rs:184` | Model-facing output wrapper |
|`ToolEmitter` / `FileChange` | `core/src/tools/events.rs:123` | Emit diff events the UI already understands |
|`apply_granted_turn_permissions` / `write_permissions_for_paths` | `core/src/tools/handlers/mod.rs:268`, `core/src/tools/handlers/apply_patch.rs:401` | Same sandbox approval model as apply_patch |
|`ExecutorFileSystem` | `file-system/src/lib.rs:191-238` | Provides `write_file`, `read_file_text`, `create_directory` |
|`similar` crate | already a dependency of `ody_apply_patch` | Generate unified diffs for FileChange events |

## Parts

| # | File | Scope | Status |
|---|---|---|---|
| 1 | `2026-07-21-ody_rs_write_file_edit_file_apply_patch/architecture.md` | handlers, spec, integration | done |
| 2 | `2026-07-21-ody_rs_write_file_edit_file_apply_patch/data-algorithms.md` | data models, algorithms, error behaviors | done |
| 3 | `2026-07-21-ody_rs_write_file_edit_file_apply_patch/security-ops.md` | security, observability, rollout, tests | done |

## Assumptions & Unverified Items

| # | Assumption | Confidence | Impact if wrong | How to verify |
|---|---|---|---|---|
| 1 | `ExecutorFileSystem` has `write_file` / `create_directory` / `read_file_text` | high | Verified in `file-system/src/lib.rs:191-238` | Read trait definitions |
| 2 | `ExecutorFileSystem` supports `rename` or can emulate it for atomic writes | medium | If not, atomic_write falls back to direct write and loses crash safety | Check trait for rename/copy; otherwise add to trait or accept limitation |
| 3 | `ExecutorFileSystem` may expose a native `append_file` primitive | low | If not, append falls back to read-modify-write, which has the limitations noted in the algorithm | Check `ExecutorFileSystem` trait for append methods |
| 4 | UI diff tracker can consume `FileChange` events without caring about source | medium | If UI filters by tool name, events from write_file/edit_file may not render | Inspect `PatchApplyUpdated` / `FileChange` consumers in `core/src/tools/events.rs` and UI code |
| 5 | `FileChange` schema can carry optional `source` metadata without breaking compatibility | medium | If not, source tracking moves to `ToolEmitter` internal logs | Verify `FileChange` schema and event envelope |
| 6 | Existing sandbox approval framework (`apply_granted_turn_permissions`) is acceptable for `write_file`/`edit_file` | medium | If stricter per-call approval is required, UX degrades significantly | Check how `apply_patch` behaves when granted permissions are narrow vs broad |
| 7 | `file_tools` can reuse `local_search_root` for not-yet-existing paths | high | Verified: `confine_to_root` uses lexical normalization only | Code inspection |
| 8 | Models will prefer `write_file`/`edit_file` over `apply_patch` for simple cases if descriptions are clear | medium | If not, apply_patch remains over-used and the original problem persists | A/B tool descriptions in integration tests or shadow evaluation |
| 9 | 10 MiB per-call content limit is acceptable | medium | If too low, legitimate large writes fail; if too high, abuse risk remains | Telemetry after rollout |

## Self-Review

### Most expensive decisions if wrong

1. **Reusing `apply_patch`'s sandbox approval model for `write_file`/`edit_file`** [C:INFERRED]
   - If the model is too permissive, unauthorized writes can occur. We audited `apply_patch.rs:401-431` and `mod.rs:268-312` and confirmed it uses the same permission-computation path as `apply_patch`. The security model is intentionally identical: once a path/directory is granted, all writes to it are allowed, just as with `apply_patch` today. This is a documented limitation, not a new risk.
2. **Assuming `FileChange` events are tool-agnostic in the UI** [C:INFERRED]
   - If the UI hardcodes that only `apply_patch` emits `FileChange`, new-tool writes will be invisible. We mitigate by emitting the same `TurnItem::FileChange` payload and by adding `source` metadata for future UI/telemetry use. We marked this as medium confidence and will verify during implementation.
3. **Assuming models will prefer the simpler tools** [C:INFERRED]
   - This is the whole premise of the fix. We mitigate by making `write_file`/`edit_file` descriptions explicit about their purpose and by updating `apply_patch` description to push complex cases there. We will monitor `apply_patch` error rates and tool-switch telemetry after rollout.

### Security sweep

- Path escaping: covered by `confine_to_root` (lexical normalization, same as `read_file`).
- Unauthorized writes: covered by reusing `apply_patch`'s `apply_granted_turn_permissions` model. The security posture is identical to `apply_patch`.
- Binary file corruption: `edit_file` reads as UTF-8 and fails closed; `write_file` writes whatever bytes it is given (same as any text tool).
- Write size abuse: mitigated by `MAX_WRITE_FILE_BYTES` (10 MiB) per `write_file` content argument; append read-modify-write path is capped by `MAX_FILE_SIZE_FOR_DIFF`.
- Directory spraying: parent directories are auto-created, but we check that the parent is not an existing file and the path stays within the workspace.
- Atomicity: overwrite uses temp-file-with-random-suffix-then-rename in the target directory. Append uses native append when available; read-modify-write append is capped and documented as non-atomic across the whole file.
- Symlink race: random temp-file suffix mitigates predictable-target symlink attacks; we document that the target path's symlink behavior follows the OS/ExecutorFileSystem semantics.
- Concurrent modification: `edit_file` and append fallback are read-modify-write and document this as a known limitation. `apply_patch` has the same limitation for single-file edits.

### Test / verification sweep

- Spec tests for schemas (required fields, enum values, `WriteMode` parsing, empty `old_string` rejection) in `file_tools_spec.rs`.
- Handler unit tests cover happy path, parent-dir auto-creation, parent-is-file error, invalid mode, append-to-new-file, append-to-existing, path escaping, content size limit, `old_string` empty, `old_string` mismatch, multi-match, `old_string == new_string` no-op, and non-UTF-8.
- Event tests verify `FileChange` payload shape and `source` metadata.
- `apply_patch` parser tests verify improved error messages for `+*** End Patch`, `+*** End of File`, and `*** End of File` outside Update hunk.
- Integration test covers end-to-end `write_file` → `edit_file` → `read_file` with event verification.

### Operations sweep

- Default enable: no feature flag, no migration.
- Tool registry registration follows existing pattern.
- Docs updates: `AGENTS.md` and code-mode templates to teach new tools.
- Rollback: if `write_file`/`edit_file` cause problems, the registry entry can be temporarily removed without affecting `apply_patch`.

### Integration sweep

- Two new handlers inside `file_tools` module.
- One new `ToolEmitter` variant (or reuse `ApplyPatch` with a single-file change map and source metadata).
- Optional `source` metadata added to `FileChange` event envelope.
- One schema update to `apply_patch` description.
- One parser diagnostic change in `ody_apply_patch`.

### Re-verified predicates

- `ExecutorFileSystem` has `write_file`, `read_file_text`, `create_directory` (yes, `file-system/src/lib.rs:191-238`).
- `confine_to_root` uses lexical normalization, not `canonicalize` (yes, `core/src/tools/handlers/file_tools/mod.rs:102-115`).
- `FileChange` enum has `Add`, `Delete`, `Update` (yes, `protocol/src/protocol.rs:3939-3950`).
- `apply_patch` parser already checks boundary markers strictly (`apply-patch/src/parser.rs:40-56` in the report).
- `String::replace` / `String::replacen` are non-overlapping (Rust standard library docs).

## User Approval

Design confirmed by the user through the seven-dimension clarification in this Design Mode session. The design was refined through two adversarial review rounds to address atomicity, concurrency, security-model consistency, and edge-case handling. Final approval is pending the host-run audit gate after `submit_design(final: true)`.

## Notes

- [C:USER] = user-confirmed during the seven-dimension clarification.
- [C:INFERRED] = assumption recorded in the table above.
- Detailed expansion of each subsystem lives in the `## Parts` files under this design's stem directory.
