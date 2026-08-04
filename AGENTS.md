## Skill crate boundaries

- `ody-core-skills` — host skill loader/parser (`SkillMetadata`, `SkillsService`).
- `ody-skills-extension` — unified integration surface (`SkillProvider`, catalog, selection, injection, model tools).
- `ody-skills` / `skills` — builtin system-skill installer; writes embedded samples to `$ODY_HOME/skills/.system`, which `ody-core-skills` discovers as a system-scope root.

### Migration note

`legacy_host_skill_injection` defaults to `true` in T3.1.1. When the unified extension is fully validated, it will default to `false` in T3.1.2 and be removed in T3.1.3.

## Running tests

- Use `cargo nextest run` — it runs test binaries in parallel and the repo already has tuned config in `.config/nextest.toml`.
- Test only the crate(s) you changed: `cargo test -p ody-core` (or the relevant package). Do not run a full-workspace `cargo test` for local iteration.
- Skip doc tests: `cargo test --tests`.
- Leave full-workspace test runs to CI (it is sharded by design).

## Code mode / V8 feature gate

- `ody-code-mode`'s V8 JS runtime lives behind the `v8` Cargo feature, **off by default** so local `cargo build` / `cargo test` never compile or statically link V8. Without the feature, `ody_code_mode::CodeModeService` keeps the same API as a stub whose runtime operations return a clear "compiled without the `v8` feature" error.
- Forwarding chain: `ody-code-mode/v8` ← `ody-core/v8` ← `ody-cli/v8`. Release/packaging builds must enable it: `cargo build -p ody-cli --release --features v8` — otherwise shipped binaries have no working code mode.
- Tests that execute real JS are `#[cfg(feature = "v8")]`-gated: all of `core/tests/suite/code_mode.rs`, the code-mode cases in `core/tests/suite/hooks.rs`, and `ody-code-mode`'s own service/runtime tests. Run them with the feature on, e.g. `cargo test -p ody-code-mode --features v8` or `cargo nextest run -p ody-core --features v8`.
- On Windows, building with `v8` requires the `RUSTY_V8_SRC_BINDING_PATH` environment variable (machine-level; points at the prebuilt binding file under the cargo registry) to bypass the v8 build script's symlink creation, which fails without Developer Mode/admin.

## Design Mode

- Design Mode is a collaboration mode entered via `/design`. It is read-only except for the current design file under `.ody-code/designs/` and its `<stem>/` split parts.
- When switching from Design to Plan mode, the session injects a handoff reminder that references the approved design file. The design must pass the C1–C8 completeness gate; with `enforcement = "Strict"` an incomplete design blocks the switch.
- Design Mode intentionally shares `PlanModeConfigToml` configuration (`enforcement`, `split_threshold`, `split_plan_compaction_ratio`) with Plan Mode.

## File editing tools

- `write_file` / `edit_file` are direct local-filesystem tools in `core/src/tools/handlers/file_tools/`. They reject `environment_id` / remote roots and are gated by the normal permission profile plus any turn-granted write permissions.
- Use `write_file` for creating, overwriting, or appending whole files.
- Use `edit_file` for small surgical string replacements (`old_string` -> `new_string`, with an optional `replace_all`).
- Use `apply_patch` for multi-file or multi-hunk changes; `apply_patch` is the canonical patch format and should be preferred for complex edits.
- Shared helpers live in `core/src/tools/handlers/file_tools/write_edit.rs`:
  - `resolve_write_path` / `resolve_write_cwd` for path resolution.
  - `ensure_write_allowed` for permission checks (reuses `write_permissions_for_paths` + `apply_granted_turn_permissions`).
  - `atomic_write` for atomic file writes (temp file + `rename`).
  - `compute_unified_diff` / `file_change_for_write` for diff reporting; diffs are skipped for content larger than `MAX_FILE_SIZE_FOR_DIFF` (1 MiB).

## apply_patch format

- Patch marker lines (`*** Begin Patch`, `*** End Patch`, `*** Add File`, `*** Update File`, `*** Delete File`, `*** End of File`) must be written exactly — do **not** prefix them with `+` or `-`.
- The parser detects common mistakes such as `+*** End Patch` / `-*** End Patch` / `+*** Begin Patch` and reports a clear error telling the model to remove the prefix.
- Content lines inside hunks still use `+` / `-` / ` ` prefixes as normal unified-diff content.

## Browser Control 工具

- `ody-browser-control` 提供 `browser__navigate`、`browser__evaluate`、`browser__click`、`browser__type`、
  `browser__go_back`、`browser__go_forward`、`browser__reload`、`browser__screenshot`、
  `browser__get_dom`、`browser__read_logs`、`browser__execute_raw_cdp` 等工具。
- 敏感操作（navigate/evaluate/click/type/go_back/go_forward/reload/execute_raw_cdp）默认需要 guardian 审批；
  只读操作（screenshot/get_dom/read_logs）不需要审批。
- `navigate` 在 loopback（`localhost`/`127.0.0.1`/`::1`）、`file://` 位于 cwd 下、短 `data:` URL（<1KiB）时自动豁免审批。
- `evaluate` 静态拒绝包含 `document.cookie`、storage API、`eval(`、`atob(`、`contentWindow` 等表达式；
  长表达式在审批 ticket 中会被截断到 500 字节。
- `execute_raw_cdp` 在外部浏览器模式下禁用；黑名单方法（cookie/storage/fetch 拦截等）直接拒绝。
- 网络日志在返回前会脱敏敏感 header 和响应 body，snapshot 只输出条目数和字节数。
- 全链路已加 `#[tracing::instrument]`，但日志字段只输出截断预览（URL/选择器 120 字节、表达式 200 字节）或长度/计数。
- 修改后优先跑 `cargo test -p ody-browser-control --tests`；依赖真实 Chrome 的测试被 `#[ignore]`，仅在手动验证时运行。
- 详细说明见 `docs/browser-control.md`。
