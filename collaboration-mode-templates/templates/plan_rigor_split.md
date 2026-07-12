## Rigor tier addendum: Large plan splitting & Parts manifest

Plans >{{ split_threshold }} tasks, or spanning multiple subsystems, must split into an index file and multiple part files. This ensures large plans remain manageable and can survive context compaction mid-generation.

### When to split

Split a plan when:
1. The task count exceeds {{ split_threshold }} (default: 8 tasks).
2. The work spans multiple subsystems and some tasks are independently shippable as phases.

If neither condition holds, keep all tasks in one file.

### File structure for split plans

A split plan consists of:

1. **Index file** (`<id>.md`) — the entry point
   - Title, Goal, Architecture, Tech Stack
   - Execution note
   - File Structure table (listing all files touched across all parts)
   - Dependency Overview (DAG spanning all tasks across all parts)
   - Risks & Open Questions
   - Spec-coverage table
   - **Parts manifest** (all rows start as `pending`)
   - **NO task sections** (no `### Task` headers, no step lists)

2. **Part files** (inside `<id>/` subdirectory)
   - Each part lives in a subdirectory named exactly after the index file's stem
   - If index is `2026-07-10-design-mode.md`, parts live in `2026-07-10-design-mode/`
   - Example: `2026-07-10-design-mode/core.md`, `2026-07-10-design-mode/api.md`
   - Each part file contains: part header → tasks for that phase → local Self-Review
   - **A file written next to the index (e.g., `2026-07-10-design-mode-core.md`) will be rejected by the write guard**

### Parts manifest

The index file must include a `## Parts` table listing all part files and their status:

```
## Parts
| # | File | Scope | Status |
|---|---|---|---|
| 1 | `<id>/core.md` | models + persistence | pending |
| 2 | `<id>/api.md` | endpoints + wiring | pending |
| 3 | `<id>/ui.md` | rendering | pending |
```

- **File column:** Relative path from index, using exact directory name (always `<id>/`)
- **Scope column:** Brief description of what this part handles
- **Status column:** `pending` (not yet written) or `done` (written + finalized)

### Writing protocol for split plans

1. **Write the index first, via `submit_plan`:**
   - Title, Goal, Architecture, Tech Stack, Execution note
   - File Structure (all files mentioned across all parts)
   - Dependency Overview (full DAG across all tasks)
   - Risks & Open Questions
   - Spec-coverage table
   - Parts manifest (all rows `pending`)
   - Call `submit_plan` with this markdown as the `plan` argument. `submit_plan` always writes to the index file; while any row is `pending`, this call saves the index and keeps Plan mode active — it does not end the turn, so you do not need to do anything else to "end" this step.

2. **Each subsequent turn: write ONE part file with a normal file-write tool (not `submit_plan`)**
   - Create the part file at exactly `<index-stem>/<part-name>.md`
   - Include: part header → its tasks → its local Self-Review (7 items)
   - `submit_plan` cannot create this file — it only ever overwrites the index — so use your normal file-write tool. Writing under the plan's own `<index-stem>/` directory is allowed in Plan mode.
   - After finishing the part, call `submit_plan` again with the index's full markdown, this time with that part's row flipped from `pending` to `done`. This re-submission is what advances the tracker — a direct edit to the index's `## Parts` table alone will not be seen.
   - Do NOT write any other part file this turn
   - Do NOT call `submit_plan` with a plan that has zero pending rows yet — that would end Plan mode before the remaining parts are written
   - Stop after the `submit_plan` call that flips the row

3. **Turn discipline during split:**
   - Injection will direct you to the next `pending` part
   - If context is compacted mid-generation: re-read the index, find the first `pending` row, and write that part (never re-write a `done` part)
   - Continue until all rows are `done`

4. **After all parts are done:**
   - Do a final cross-file consistency review (check cross-file dependencies, confirm no symbols are used before definition)
   - Call `submit_plan` with the complete index (all rows `done`) to request approval — only this final call, once no rows are `pending`, ends Plan mode

### Cross-file dependencies

Tasks in different parts may depend on each other. Use this format:

```
**Depends on:** <id>/core.md: Task 2
```

Example: `Depends on: design-mode/core.md: Task 3` means "this task uses a symbol/artifact that Task 3 in the core.md part created."

### Local Self-Review in each part

Each part file must end with its own Self-Review section:

```
## Self-review (Part 1)

- [ ] 1. Spec-coverage: all spec items handled in this part are marked covered / GAP / no-op.
- [ ] 2. Placeholder scan: no TODO/TBD/deferred placeholders.
- [ ] 3. No phantom tasks: every task in this part produces verifiable change.
- [ ] 4. Dependency soundness: all `Depends on:` within this part (and cross-file refs) are satisfied.
- [ ] 5. Caller & build soundness: (if applicable) shared-signature tasks updated all callers and ended with typecheck.
- [ ] 6. Test-the-risk: state-mutating tasks have behavioral tests.
- [ ] 7. Type consistency: types/signatures match earlier tasks (within this part and cross-file).
```

### Cross-file final review (in index file, before the final `submit_plan` call)

Once all parts are done, review:
- Do all cross-file dependencies reference valid earlier parts/tasks?
- Does the Spec-coverage table (in index) still map every spec item?
- Are there any conflicts between tasks in different parts (e.g., two parts modifying the same file)?
- Do the File Structure and Dependency Overview (in index) remain accurate?

### Example: Three-part design-mode rollout

**Index file:** `2026-07-10-design-mode.md`
```markdown
# Design Mode Collaboration Protocol — Implementation Plan

**Goal:** ...
**Architecture:** ...
**Tech Stack:** ...

## File Structure
| Responsibility | File |
|---|---|
| Protocol types | `app-server-protocol/src/protocol/v2/collaboration_mode.rs` |
| Server list endpoint | `app-server/src/request_processors/catalog_processor.rs` |
| Config types | `config/src/config_toml.rs` |
| Mode instructions | `core/src/context/collaboration_mode_instructions.rs` |
| Mode presets | `models-manager/src/collaboration_mode_presets.rs` |
| Schema fixtures | `app-server-protocol/schema/typescript/ModeKind.ts` etc |

## Dependency Overview
```
Part 1: Protocol-core (Protocol types + presets)
  ├─ Task 1: Verify ModeKind::Design in enum (depends on nothing)
  └─ Task 2: design_preset() in presets.rs (depends on Task 1)

Part 2: Configuration (Config types + instructions)
  ├─ Task 3: Extend config docs (depends on Part 1: Task 2)
  └─ Task 4: Design split_threshold rendering (depends on Task 3)

Part 3: Schema + Verification (Fixtures + tests)
  ├─ Task 5: Regenerate fixtures (depends on Part 1: Task 1, Part 2: Task 4)
  ├─ Task 6: app-server list endpoint assertion (depends on Part 1: Task 2)
  └─ Task 7: Full workspace typecheck (depends on all above)
```

## Parts
| # | File | Scope | Status |
|---|---|---|---|
| 1 | `design-mode/protocol-core.md` | Protocol types + presets | pending |
| 2 | `design-mode/config.md` | Config + instructions | pending |
| 3 | `design-mode/schema.md` | Schema fixtures + verification | pending |

...rest of index...
```

**Part file 1:** `design-mode/protocol-core.md`
```markdown
# Design Mode — Protocol & Presets (Part 1)

**Scope:** Verify `ModeKind::Design` enum exists, add design_preset() function.

### Task 1: Verify ModeKind::Design exists in protocol enum
...

### Task 2: Add design_preset() to builtin_collaboration_mode_presets()
...

## Self-review (Part 1)
- [ ] 1. Spec-coverage table: both spec items (enum + preset) are covered.
...
```

**After Part 1 is done:** call `submit_plan` again with the index markdown, Part 1's row flipped to `done`, then stop.

**Next turn:** Injection directs to Part 2, which may reference Part 1's tasks.
