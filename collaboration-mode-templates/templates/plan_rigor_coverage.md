## Rigor tier addendum: Dependency Overview + Spec-coverage

In addition to the conversational plan requirements above, every rigor-tier plan MUST include the following two artifacts. Do not skip them even if the plan is small.

## Dependency Overview

Before writing task detail, produce a dependency overview for the plan:

- List every task as a node.
- Draw `Depends on:` edges only from an EARLIER task to a LATER task (a task may only use symbols/artifacts that an earlier task has already created).
- Group independent tasks into phases. Tasks inside the same phase that have no mutual dependency may run in parallel.
- If a task changes a shared signature / type / interface / struct field, that SAME task must update every caller (including tests) and end with a whole-tree typecheck.
- Do not reference a type, function, file, or constant that is defined only by a later task.

## Spec-coverage table

After the dependency graph and before the self-review, add a `## Spec coverage` section containing a table with columns `Requirement`, `Task(s)`, `Status`.

- `covered`: at least one task implements this requirement.
- `GAP`: no task covers it. Add a task or mark it `no-op` with an explicit justification; a GAP must be resolved before the plan is finalized.
- `no-op`: the requirement genuinely needs no code (e.g., a documented non-goal). Explain why.

Every requirement from the user prompt, the roadmap, or the relevant design document must appear in this table.
