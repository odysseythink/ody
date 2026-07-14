## Rigor tier addendum: Structured workflow

In addition to the conversational planning phases above, rigor-tier plans MUST follow this explicit workflow to ensure completeness and traceability.

### Workflow (five steps)

1. **Understand** — explore the codebase with targeted, low-noise searches to discover existing functions, utilities, and patterns you can reuse. Prefer narrow queries that return file paths first, then read only the specific regions you need — a broad dump of matching lines burns the context you will need for the plan itself. Eliminate unknowns by active discovery before planning.

2. **File Structure** — list the files each task creates or modifies, with one clear responsibility per file. If a task touches multiple files, explain which file handles which concern.

3. **Dependency Overview** — order tasks as a directed acyclic graph (DAG). A task may only use symbols that an earlier task has already created. Group independent tasks into phases; tasks in the same phase with no mutual dependencies may run in parallel.

4. **Write the plan** — incrementally scaffold the plan document:
   - First, write the header (title, goal, architecture, tech stack, execution note).
   - Then append the File Structure and Dependency Overview.
   - Then append task detail, one section per turn (or one phase per turn for split plans).
   - Finally, append the Self-Review checklist and verify all seven items.

5. **Self-review** — run all seven verification items (see Self-review addendum) against the spec before finalizing.

### Plan document header (top of every plan)

Every plan must start with:

- **Title**: `# <Feature> Implementation Plan`
- **Goal**: one sentence describing success
- **Architecture**: 2-3 sentences explaining design decisions and key tradeoffs
- **Tech Stack**: technologies, languages, and frameworks involved
- **Execution note**: `> For executing workers: implement this plan task-by-task (prefer a fresh subagent/Task per task — a clean context per task avoids single-session degradation). Steps use - [ ] checkboxes for tracking.`

Example:
```
# Design Mode Collaboration Protocol — Implementation Plan

**Goal:** Integrate Design mode into app-server's collaboration mode protocol and ensure TypeScript/JSON schema fixtures reflect the new mode.

**Architecture:** Design mode shares configuration and enforcement with Plan mode (both read-only, both use `PlanModeConfigToml`). D0 added `ModeKind::Design` to the Rust enum; D8 syncs TypeScript bindings and JSON schema by regenerating fixtures, ensuring clients recognize the new mode.

**Tech Stack:** Rust (cargo, schemars, ts-rs), TypeScript, JSON schema.

> For executing workers: implement this plan task-by-task (prefer a fresh subagent/Task per task — a clean context per task avoids single-session degradation). Steps use - [ ] checkboxes for tracking.
```
