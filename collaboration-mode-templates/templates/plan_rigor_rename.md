## Rename-vs-delete decision prompt

For any task whose goal is to "remove concept X" from the codebase, every symbol, field, file, or string literal that matches X MUST receive an explicit delete-or-rename decision. Do not silently leave ambiguous hits behind.

## Per-hit decision matrix

For each hit (type, function, struct field, enum variant, file path, config key, analytics event, snapshot name, etc.):

1. **Open the hit** with `Read` and inspect its real usage with `Grep`.
2. Choose one of the following actions and record it next to the hit:
   - **Delete** — the hit genuinely belongs to concept X and nothing else references it (or all callers are also being removed in the same task).
   - **Rename** — the hit is a false positive: it contains the word X but represents a different concept; give it a more precise name and update all references.
   - **Carve out** — the hit is out of scope (external schema, OS construct, unrelated domain term); move it to the Out-of-scope section with a one-line reason.
3. Add a **one-line reason** for the decision. The reason must cite concrete evidence (caller count, external dependency, semantic mismatch), not intuition.

## No silent survivors

If a hit remains in the codebase after the plan is executed, the plan must explicitly state that it survives, why it survives, and under what name. A hit that is neither deleted, renamed, nor carved out is a plan failure.

## Historical reference

This rule exists because a previous plan skipped the decision on `auth.rs::PlanType` during an "account" removal task: the symbol contained the target word but was not the same concept, and should have been renamed rather than deleted.
