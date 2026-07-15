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

## Design Mode

- Design Mode is a collaboration mode entered via `/design`. It is read-only except for the current design file under `.ody-code/designs/` and its `<stem>/` split parts.
- When switching from Design to Plan mode, the session injects a handoff reminder that references the approved design file. The design must pass the C1–C8 completeness gate; with `enforcement = "Strict"` an incomplete design blocks the switch.
- Design Mode intentionally shares `PlanModeConfigToml` configuration (`enforcement`, `split_threshold`, `split_plan_compaction_ratio`) with Plan Mode.
