## Skill crate boundaries

- `ody-core-skills` — host skill loader/parser (`SkillMetadata`, `SkillsService`).
- `ody-skills-extension` — unified integration surface (`SkillProvider`, catalog, selection, injection, model tools).
- `ody-skills` / `skills` — builtin system-skill installer; writes embedded samples to `$ODY_HOME/skills/.system`, which `ody-core-skills` discovers as a system-scope root.

### Game-development skill gate (`--game`)

- The embedded game-development bundle (the `/game` flow skill plus the vendored gamedev skill set, listed in `ody-skills::EMBEDDED_GAME_SKILL_NAMES`) is hidden by default to keep the skills context budget small. It becomes visible only when the session config stack has `skills.bundled.game = true`; the `ody --game` / `ody exec --game` CLI flag layers exactly that override (`SharedCliOptions::implied_config_overrides`).
- The filter runs in `SkillsService::build_skill_outcome` via `filter_skill_load_outcome_for_game` and only removes System-scope skills whose names are on the embedded game list — user- or repo-installed skills with the same name are unaffected.
- `ody-skills::tests::game_skill_classification_partitions_embedded_skills` enforces that every embedded skill is classified as game or non-game; update the lists when adding embedded skills.

### Frontend-design mode (`--frontend`)

- The frontend bundle is the two embedded craft skills `make-interfaces-feel-better` and `motion-design` (`ody-skills::EMBEDDED_FRONTEND_SKILL_NAMES`). It is opt-in per invocation: `ody --frontend` / `ody exec --frontend` layers `skills.bundled.frontend = true` onto the config stack through the same `SharedCliOptions::implied_config_overrides` path as `--game`.
- Unlike `--game` (additive), frontend mode is **exclusive**. `filter_skill_load_outcome_for_frontend` (called from `SkillsService::build_skill_outcome` right after the game filter) retains only skills whose names are on the frontend list, whatever scope they were installed in — user, repo, plugin and the rest of the bundled set all disappear so the model works from the two craft skills alone. `frontend-design` is deliberately not a member.
- Name-based matching means a repo- or user-installed copy of either skill still survives the filter.
- `--frontend` also overrides `skills.bundled.enabled = false` for the system root: the mode's only skill source is the bundled set, so honouring that switch would leave the mode empty.
- `ody-skills::tests::frontend_bundle_names_exist_in_the_embedded_set` fails if either bundled name is renamed, and asserts the two bundles stay disjoint.

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

## Builtin MCP bundling feature gate

- The builtin MCP servers (`fetch`, `sequentialthinking`, `arxiv`, `context7`) normally run as the separate `ody-builtin-mcp` stdio helper binary. The `bundled-builtin-mcp` Cargo feature **(off by default)** instead serves them in-process over tokio duplex streams via the existing `ody_rmcp_client::InProcessTransportFactory` plumbing, so a release build can distribute a single `ody` binary with no helper executable on `PATH`.
- Forwarding: `ody-mcp/bundled-builtin-mcp` (pulls in the `ody-builtin-mcp` lib as an optional dependency) ← `ody-cli/bundled-builtin-mcp` and `ody-app-server/bundled-builtin-mcp`. Release/packaging builds enable it with `cargo build -p ody-cli --release --features bundled-builtin-mcp` (same flag for `-p ody-app-server`).
- Only the exact default builtin launch shape is intercepted (`ody-builtin-mcp <name>`, local environment, no custom env/cwd); user overrides fall through to the normal stdio child-process path. The interception lives in `ody-mcp/src/builtin_in_process.rs` + `rmcp_client.rs::make_rmcp_client`; the serve entry point is `ody_builtin_mcp::serve_builtin`.
- Feature-gated tests: `cargo nextest run -p ody-mcp --features bundled-builtin-mcp` (includes the in-process factory handshake test); `cargo nextest run -p ody-builtin-mcp` covers the duplex-stream serving of all four servers.

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
- **默认安全姿态：** 本地模式使用临时 profile（`--user-data-dir` 等危险启动参数被 `sanitize_args` 过滤），敏感操作走 guardian 审批，`evaluate` 静态拒绝 cookie/storage 读取，网络日志脱敏后返回。
- **关键配置字段：** `mode`（Local/External）、`headless`、`sandbox`、`allow_local_network`、`external_browser_allow_sensitive`、`disable_extensions`、`extra_args`（经 `sanitize_args` 过滤）。详细配置说明见 `docs/browser-control.md`。
- **残余风险：** 静态拒绝无法穷尽所有 JS 绕过；企业级 URL allowlist 和外部 browser-use MCP 去重尚未实现。
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

## Flow multi-runtime (M3)

- Flow skills support three carriers; the loader discovers exactly one of `flow.yaml` (declarative), `flow.star` (Starlark script), `workflow.js` (V8 script) and errors when several are present.
- `flow-starlark` is a default feature of `ody-core` (pure Rust). `flow-v8` is off by default (`flow-v8 = ["v8"]`, compiles/links V8); `ody-cli` forwards it, so release builds ship `workflow.js` support only with `--features flow-v8`.
- Trigger-time degradation (locked decision 2): in a build without `flow-v8`, a `workflow.js` skill still loads fine and fails at trigger time with a clear "compiled without the `flow-v8` Cargo feature" error.
- Host-function semantics are identical across carriers (the module docs of `core/src/flow/mod.rs` are the single source of truth): `agent(prompt)`, `pipeline(items, each)`, `parallel(fns)` (each/fns are pure prompt renderers), `phase(msg)`/`log(msg)`; a single fan-out batch is hard-capped at `FLOW_BATCH_LIMIT = 4096`.
- Script-carrier result contract: a top-level `result` binding becomes `outputs["result"]` (`None` → empty outputs). For `workflow.js` any top-level declaration works — the engine appends a tail that mirrors the module-scope binding onto `globalThis`.
- Determinism (workflow.js): the global `Date` is deleted and `Math.random` is replaced with a throwing function.
- Checkpoint/replay (M2.1) applies uniformly to all three carriers (key = prompt SHA-256); run-before guardian approval uses `FlowPlanSummary::for_plan` (structured phases for yaml, bounded 40-line source preview for script carriers).
- Conformance guard: `conformance_v8_matches_yaml_and_starlark_calls_and_outputs` (and the yaml×star pair) assert the same logical plan produces the same prompt multiset and outputs across carriers.

## Flow M4: Claude Code alignment (concurrency / stagger / schema / `/flows`)

- Per-run caps (`core/src/flow/concurrency.rs` `RunConcurrency`): at most `FLOW_CONCURRENCY_LIMIT = 16` concurrent agents per run (semaphore) and `FLOW_MAX_AGENTS_PER_RUN = 1000` total spawns per run. Wired in `SessionFlowAgentHost`, so all three carriers (yaml/star/v8) inherit it; checkpoint replay hits do not spawn and take no permit (Claude "completed agents return saved results" semantics).
- Fan-out prefix stagger (`PrefixStagger`, same file): the first spawn of a run passes immediately; subsequent spawns inside the stagger window hold until the first agent leaves `PendingInit` (observable "response has begun" equivalent) or the window times out. Env `ODY_FLOW_PREFIX_STAGGER_MS` (default 5000, 0 disables), read once at host construction. Hold is abort-aware (`tokio::select!` on the abort channel).
- Structured-output schema across carriers: `core/src/flow/runtime.rs::run_agent_text` is the single source of truth (schema prompt injection + bounded retry now live inside it — do not re-inject schema instructions on the yaml side). Starlark API: `agent(prompt, schema=None)` (dict → JSON); v8 API: `agent(prompt, {schema})` with `WorkflowHost::run_agent(prompt, schema: Option<String>)` passthrough in code-mode.
- TUI `/flows` (`tui/src/chatwidget/flows.rs`): read-only view over a `FlowRuns` registry — yaml runs show structured phase progress; script carriers only have log lines, so their runs stay `"running"` on the protocol surface (in-turn execution is not observable as completion). Protocol: `ThreadItem::FlowLog` / `EventMsg::FlowLog` on the v2 surface. Control keys (pause/stop/restart), token totals and per-agent drill-down are deliberately out of scope (need protocol event + execution-control extensions).
- Verify with: `cargo nextest run -p ody-core -E 'test(flow_tests::)'` (default and `--no-default-features --features flow-starlark,flow-v8`), plus `cargo nextest run -p ody-code-mode --features v8` when touching the v8 host signature.
