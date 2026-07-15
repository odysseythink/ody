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
