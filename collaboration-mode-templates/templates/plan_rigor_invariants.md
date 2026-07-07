## Rigor tier addendum: Build-green invariant + No-placeholders

In addition to the Dependency Overview, Spec-coverage table, and Self-review above, every rigor-tier plan MUST enforce the following two invariants.

## Shared-signature build-green invariant

If a task changes a shared signature / type / interface / struct field / trait method that other code already uses, that SAME task must:

1. Find and update EVERY caller — including test files.
2. End with a whole-tree typecheck, not a single-package build.

For this Rust workspace, the whole-tree typecheck command is:

```bash
cargo check --workspace --all-targets
```

A compile-clean change whose runtime consumer keys off a different value (for example, a file written under one identifier but authorized by a guard matching another) is a HARD failure. Verify the consumer with Read/Grep — never assume it continues to work.

## No-placeholders rule

Every task must contain the real content an engineer needs. The following are plan failures and must not appear:

- `TODO`, `TBD`, `FIXME`, or "implement later".
- "Add appropriate error handling / validation" without the actual validation.
- "Write tests for the above" without the test code.
- "Similar to Task N" — repeat the code instead.
- References to types, functions, or files that no task defines.
- Author deliberation left in the body.

A dependency on unfinished upstream work must be a prerequisite task or a typed shim — never a placeholder or dead code. A spec item that genuinely needs no code must be marked `no-op` in the Spec-coverage table with an explicit justification, not manufactured into a phantom task.
