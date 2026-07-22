# 为 `ody-rs` 新增 `write_file` / `edit_file` 工具并改进 `apply_patch` 错误提示 — Implementation Plan

**Goal:** 在 `core/src/tools/handlers/file_tools` 中新增 `write_file` 与 `edit_file` 两个本地文件写入工具，统一复用现有文件工具的路径解析、沙盒审批与事件流；同时改进 `apply_patch` 的解析错误提示，并补充完整测试与文档。

**Architecture:** 新增工具以 `ReadFileHandler` 为模板，复用 `local_search_root` 做路径解析，复用 `apply_patch` 的 `write_permissions_for_paths` + `apply_granted_turn_permissions` 计算并校验写入权限；实际写入走 `ExecutorFileSystem` 以保留远程/沙盒扩展点。写入成功后通过 `ToolEmitter` 发出同质 `FileChange` 事件，并附加 `source` 元数据。`apply_patch` 的改进集中在边界标记检测与错误提示，不动 patch 语法本身。

**Tech Stack:** Rust (cargo, tokio), `ody-core`, `ody-file-system`, `ody-exec-server`, `ody-apply-patch`, `ody-protocol`, `similar` (unified diff), `serde_json`, `ody-tools` (ToolSpec/JsonSchema).

> For executing workers: implement this plan task-by-task (prefer a fresh subagent/Task per task — a clean context per task avoids single-session degradation). Steps use - [ ] checkboxes for tracking.

## File Structure

| Responsibility | File |
|---|---|
| `write_file` / `edit_file` JSON Schema + 常量 | `core/src/tools/handlers/file_tools_spec.rs` |
| 共享写入原子性、权限、diff 辅助函数 | `core/src/tools/handlers/file_tools/write_edit.rs` |
| `WriteFileHandler` 实现 | `core/src/tools/handlers/file_tools/write.rs` |
| `EditFileHandler` 实现 | `core/src/tools/handlers/file_tools/edit.rs` |
| 模块导出（新增 handler 公开） | `core/src/tools/handlers/file_tools/mod.rs`, `core/src/tools/handlers/mod.rs` |
| 工具注册到 turn plan | `core/src/tools/spec_plan.rs` |
| `FileChange` 事件附加 `source` 元数据 | `core/src/tools/events.rs` |
| `ExecutorFileSystem` 原子 rename | `file-system/src/lib.rs`, `exec-server/src/local_file_system.rs` |
| `apply_patch` 解析错误提示 | `apply-patch/src/parser.rs`, `apply-patch/src/streaming_parser.rs` |
| `apply_patch` 工具描述更新 | `core/src/tools/handlers/apply_patch_spec.rs` |
| 工具使用文档 | `AGENTS.md` |
| 单元 / 集成测试 | `core/src/tools/handlers/file_tools/write_edit_tests.rs`, `core/src/tools/handlers/file_tools_spec.rs`, `apply-patch/src/parser.rs` (tests), `core/src/tools/handlers/apply_patch_spec_tests.rs` |

## Dependency Overview

```
Part 1: Schemas, handlers, and registration
  ├─ Task 1: Add write_file/edit_file specs to file_tools_spec.rs (depends on nothing)
  ├─ Task 2: Add shared write_edit helpers (depends on Part 2: Task 6)
  ├─ Task 3: Implement WriteFileHandler (depends on Task 1, Task 2)
  ├─ Task 4: Implement EditFileHandler (depends on Task 1, Task 2)
  └─ Task 5: Wire handlers and register in spec_plan.rs (depends on Task 3, Task 4)

Part 2: Atomic writes, filesystem trait, and events
  ├─ Task 6: Add rename to ExecutorFileSystem trait (depends on nothing)
  ├─ Task 7: Implement native rename in local filesystems (depends on Task 6)
  ├─ Task 8: Test atomic write and diff helpers (depends on Task 7, Part 1: Task 2)
  └─ Task 9: Emit FileChange events with source metadata (depends on Task 8, Part 1: Task 5)

Part 3: apply_patch parser improvements and tests
  ├─ Task 10: Improve apply_patch parser error diagnostics (depends on nothing)
  ├─ Task 11: Update apply_patch tool description (depends on nothing)
  ├─ Task 12: Add parser and spec tests (depends on Task 10, Task 11)
  ├─ Task 13: Add end-to-end integration test (depends on Part 1: Task 5, Part 2: Task 9, Part 3: Task 10)
  └─ Task 14: Update AGENTS.md documentation (depends on Part 1: Task 5, Part 3: Task 10)
```

## Risks & Open Questions

| # | Risk | Mitigation |
|---|---|---|
| R1 | `ExecutorFileSystem` 没有 `rename`；临时文件重命名需要新增 trait 方法并覆盖主要本地实现。 | Task 6 + Task 7：新增 `rename` 默认实现（copy+remove 回退），并在 `LocalFileSystem` / `DirectFileSystem` / `UnsandboxedFileSystem` 实现原生重命名。 |
| R2 | `write_file`/`edit_file` 复用 `apply_granted_turn_permissions` 后，如果未预授权则直接报错，而非像 `apply_patch` 那样内联弹窗请求。 | 本次实现采用“未预授权则返回明确错误并引导调用 `request_permissions`”的路径；如后续需要 `apply_patch` 同款内联审批，需新增 `WriteFileRuntime` 走 `ToolOrchestrator`。 |
| R3 | `FileChange` 事件消费方（UI/turn diff tracker）若按工具名过滤，则 `write_file`/`edit_file` 的事件不可见。 | Task 9 复用 `ToolEmitter::ApplyPatch` 的 `TurnItem::FileChange` 载荷，并增加 `source` 字段以便未来 UI 区分；同时添加事件集成测试。 |
| R4 | 大文件 append 读改写路径内存爆掉。 | Task 2 中 `MAX_FILE_SIZE_FOR_DIFF` (1 MiB) 上限；append 回退路径超过此大小时返回错误。 |
| R5 | `edit_file` 基于字符串替换，对非 UTF-8 / 二进制文件会失败。 | Task 4 明确拒绝非 UTF-8，返回“not valid UTF-8”错误；`write_file` 可写任意字节。 |

## Spec coverage

| Requirement | Task(s) | Status |
|---|---|---|
| 新增 `write_file` 工具：创建/覆盖/追加，自动创建父目录 | Part 1: Task 1, Task 3 | covered |
| 新增 `edit_file` 工具：`old_string`/`new_string` 精确替换，支持 `replace_all` | Part 1: Task 1, Task 4 | covered |
| 纳入 `file_tools` 模块并复用 `local_search_root` | Part 1: Task 3, Task 4, Task 5 | covered |
| 写入成功后发出 `FileChange` 事件，兼容 `apply_patch` | Part 2: Task 8, Task 9 | covered |
| 复用 `apply_patch` 沙盒审批模型 | Part 1: Task 2, Task 3, Task 4 | covered |
| 改进 `apply_patch` 结束标记 `+` 前缀错误提示 | Part 3: Task 10 | covered |
| 改进 `apply_patch` 其他常见格式错误提示 | Part 3: Task 10 | covered |
| 更新 `apply_patch` 描述以区分复杂场景 | Part 3: Task 11 | covered |
| 默认启用，无特性开关 | Part 1: Task 5 | covered |
| 不支持远程环境（与 `read_file` 一致） | Part 1: Task 3, Task 4 | covered |
| 不替代 `apply_patch` | Part 3: Task 11 | covered |
| 不引入二进制/流式分块写入 | Part 1: Task 1 | covered |

## Out-of-scope

| Symbol / Path | Reason | Action |
|---|---|---|
| `ody-skills` / `ody-core-skills` 中的文件读写技能 | 与 `file_tools` 是不同模块，本次仅扩展核心 `file_tools` | 无需修改 |
| `ody-code-mode` V8 运行时 | 文件工具不依赖 V8 feature gate | 无需修改 |
| `windows-sandbox-rs` OS 级账户/文件权限 | 不属于 `file_tools` 本地写入路径 | 无需修改 |
| `apply_patch` 的 patch 语法本身 | 设计明确只改错误提示，不改语法 | 保留 |
| 远程 `ExecutorFileSystem` 的 `rename` 原生实现 | 设计限定本地文件系统；`RemoteFileSystem` 使用默认 copy+remove 回退 | 使用默认实现 |
| 沙盒 `SandboxedFileSystem` 的 `rename` 原生实现 | 需要扩展 sandbox helper 协议，超出设计限定的本地文件系统范围；默认 copy+remove 回退可接受 | 使用默认实现 |
| `exec::FileChangeItem` (`exec/src/exec_events.rs:182`) | 这是 `exec` 输出格式专用结构体，不是 `protocol::FileChangeItem` 事件信封；本次 `source` 只进入协议事件 | 无需修改 |

## Parts

| # | File | Scope | Status |
|---|---|---|---|
| 1 | `2026-07-21-ody_rs_write_file_edit_file_apply_patch_implementation_plan/schemas-handlers.md` | Schemas, handlers, registration | done |
| 2 | `2026-07-21-ody_rs_write_file_edit_file_apply_patch_implementation_plan/filesystem-events.md` | Atomic writes, filesystem trait, FileChange events | done |
| 3 | `2026-07-21-ody_rs_write_file_edit_file_apply_patch_implementation_plan/parser-tests.md` | apply_patch parser improvements, tests, docs | done |

## Self-review

- [x] 1. Spec-coverage table: 每一条 Scope In 需求都已映射到任务并标记 covered。Verified against `## Spec coverage` above.
- [x] 2. Placeholder scan: 计划中无 TODO/TBD/deferred 占位符；所有任务均包含具体代码、命令与预期输出。Verified by reading all three part files.
- [x] 3. No phantom tasks: 每个任务都产生可验证的代码或文档变更。Verified: Task 1-9 produce concrete code, Task 10-14 produce code/tests/docs.
- [x] 4. Dependency soundness: 所有 `Depends on:` 都指向更早完成的任务；无反向依赖。Verified against `## Dependency Overview`.
- [x] 5. Caller & build soundness: `ExecutorFileSystem` 新增 `rename` 使用默认实现，不破坏现有实现；每个修改 trait 的任务都包含全工作区 typecheck。Verified by checking Task 6-7 and Task 9 include `cargo check --workspace --all-targets`.
- [x] 6. Test-the-risk: 状态变更类任务（写入、编辑、事件、解析错误）均包含行为测试。Verified: Task 8/9/12/13 include behavioral tests for the mutations/errors.
- [x] 7. Type consistency: 后续任务使用的 `FileChange`、`ToolEmitter`、`ExecutorFileSystem` 类型与前置任务定义一致。Verified: Task 9 defines `source` as `Option<String>` and all call sites use the same field/type.