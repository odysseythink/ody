## Rigor tier addendum: task parts and the Parts manifest

Rigor values complete, executable detail over brevity. A Rigor plan with more than one executable task must use a task manifest and one part file per task, even when the generic split threshold would otherwise allow a single file. A one-task plan may remain in one file. The generic split threshold still applies to non-Rigor plans and can require splitting for other reasons.

Do not compress, merge away, or replace concrete implementation steps, source evidence, failure cases, or behavioral tests with a summary. There is no default byte target for a task part. If an installation explicitly configures a size limit, split only that task into separately named, explicitly limited tasks before writing it; never make the task less complete to fit.

### File structure

A split Rigor plan consists of:

1. **Index file** — the entry point. It contains the goal, architecture, file structure, dependency overview, risks, spec-coverage table, and the `## Parts` manifest. It contains no `### Task` sections.
2. **Task part files** — stored in the subdirectory named after the index file's stem. If the index is `2026-07-10-search.md`, its task files live in `2026-07-10-search/`. The `submit_plan` response supplies the real directory name; use it verbatim.

### Task manifest

For a split Rigor plan, the index must contain this six-column table. Every row is exactly one independently executable task.

```markdown
## Parts
| ID | File | Task | Scope | Depends on | Status |
|---|---|---|---|---|---|
| T01 | `2026-07-10-search/models.md` | Add persisted search preferences | data model and migration | — | pending |
| T02 | `2026-07-10-search/command.md` | Add preferences slash command | parser, command routing, and UI action | T01 | pending |
| T03 | `2026-07-10-search/tests.md` | Add behavioral coverage | command and persistence tests | T01, T02 | pending |
```

- **ID** is a stable unique task ID such as `T01`. Do not renumber it when a task completes.
- **File** is the exact, real path a reader can open from the plans directory. It must use the real `<index-stem>/name.md` directory printed by the host, not a placeholder or a bare filename.
- **Task** is the full task title and must match the sole Task heading in that file.
- **Scope** states the change surface without replacing the task details in the task file.
- **Depends on** lists earlier task IDs separated by commas, or `—` when there is no dependency. A task cannot become `done` until each dependency is a verified `done` task.
- **Status** is `pending` until its file passes the completion contract, then `done`.

### Required task-part contract

Each task part contains exactly one Task heading and every section below. It is a complete implementation plan for that task, not a hand-off summary.

```markdown
# <area> — <task title>

**Manifest task:** T01

### Task T01: Add persisted search preferences

**Files**
- `path/to/file.rs:42` — exact change and why.

**Source evidence**
- `path/to/file.rs:42` — current behavior, API, or invariant being changed.

**Implementation**
1. Concrete edit with symbols, control flow, data shape, and compatibility behavior.
2. Concrete integration edit and every caller or serialization boundary affected.

**Failure and edge cases**
- Error path, migration/default behavior, and rollback or compatibility handling.

**Tests**
- Exact test target and behavioral assertion that proves the changed risk.

## Self-review

- [x] 1. Spec-coverage: evidence recorded.
- [x] 2. Placeholder scan: no TODO/TBD/deferred plan placeholders.
- [x] 3. No phantom task: this task produces a verifiable change.
- [x] 4. Dependency soundness: every listed dependency is complete.
- [x] 5. Caller and build soundness: affected callers and build checks are named.
- [x] 6. Test-the-risk: behavioral coverage proves the changed behavior.
- [x] 7. Type consistency: types and signatures agree with dependent tasks.
```

The source-evidence section must include at least one backticked `path:line` anchor. All seven Self-review items must be checked before the row can be marked `done`.

### Writing protocol

1. Write the complete index first with all task rows `pending`, then call `submit_plan` with the full index.
2. The host names the only pending task that may be written. Write that exact task file with a normal file-write tool; `submit_plan` only writes the index.
3. Call `submit_plan` again with the complete index and only the verified task row changed to `done`.
4. The host automatically continues to the next pending task. Do not stop with a plain-text progress report, do not ask for approval, and do not create or edit a later task file first.
5. After every row is `done`, perform the cross-file consistency review and call `submit_plan` once more with the complete index. Only that final submission requests approval and ends Plan mode.

If context is compacted at a task boundary, re-read the index and the current task's completed dependencies before writing. Continue from the first `pending` manifest row; never rewrite a verified `done` task.

### Cross-task review

Before the final submission, verify that every dependency ID exists and points to an earlier completed task, all files and symbols referenced across task parts agree, the index's spec-coverage table remains accurate, and no two task parts prescribe conflicting changes to the same behavior.
