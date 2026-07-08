## Rigor tier addendum: Source-grounding mandate

Every rigor-tier plan MUST ground every listed change, dependency, and out-of-scope carve-out in actual source evidence. Do not infer scope from names, paths, or phase titles.

## Verify with Read/Grep before writing

Before adding any file, dependency, or change to the plan:

1. Use `Read` to open the file and confirm the relevant lines exist.
2. Use `Grep` to find callers, usages, and related symbols.
3. Record the concrete evidence (file path, line range, or search result) that justifies the entry.

If you cannot find source evidence, mark the item as an explicit assumption with a `[C:INFERRED]` label and a one-sentence justification. Do not present inferred scope as fact.

## Never infer scope from names

A name that contains a target word is NOT automatically in scope. For example:

- An OS-level "account" is not the same as an application-level "account".
- A field named `creator_account_user_id` in an external schema is not the same as an internal account model.
- A crate named `accounting` that tracks token balances is not the same as a user account system.

For every hit, open the file and decide whether it refers to the same concept. Add hits that are a different concept to the Out-of-scope section with a one-line reason.

## No invented dependencies

Dependencies between tasks must come from actual symbol/artifact usage, not from phase titles or logical grouping. If Task N uses a constant, function, or file created by Task M, the dependency is real; otherwise, do not add the edge.
