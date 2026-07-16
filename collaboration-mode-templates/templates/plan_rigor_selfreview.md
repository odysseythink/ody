## Rigor tier addendum: Self-review checklist

In addition to the Dependency Overview and Spec-coverage table above, every rigor-tier plan MUST end with a `## Self-review` section reproducing all seven items below, as checkboxes, in this order.

These are yours to run, not the implementer's. Check an item off (`- [x]`) once you have actually verified it, and say what you verified against — name the file, symbol, or command that settled it, not "done". An item you cannot substantiate stays `- [ ]`, and a plan with an unchecked item is not ready to finalize: fix what the item exposed, then check it. A checked item with nothing behind it is worse than an unchecked one — it spends the reader's trust and buys nothing.

- [ ] 1. Spec-coverage table: map every spec section/requirement → Task(s), marked covered / GAP / no-op (GAP means add the task).
- [ ] 2. Placeholder scan: no TODO/TBD, no deferred-by-dependency excuses, no dead-code placeholders.
- [ ] 3. No phantom tasks: every task produces a verifiable change; zero `--allow-empty` / "already done in Task N".
- [ ] 4. Dependency soundness: every `Depends on:` is satisfied by an earlier task; nothing references a symbol only a later task creates.
- [ ] 5. Caller & build soundness: every shared-signature task updated all callers (including test files) and ends with a whole-tree typecheck, not a single-package build; the same signature is not changed across multiple tasks. Beyond the type level — for any identifier, path, or filename a task changes, open the runtime consumer that reads or validates it and trace one concrete value end-to-end. A compile-clean change whose consumer keys off a different value is a HARD failure.
- [ ] 6. Test-the-risk: every state-mutating task has a behavioral test asserting the mutation, not just a compile check. For each test assertion, trace the expected value through the implementation constants it depends on — a test that expects a "must-survive" input to pass a filter that would actually reject it is a HARD failure; fix the constant or the assertion before proceeding.
- [ ] 7. Type consistency: types, signatures and property names used in later tasks match what earlier tasks defined.

The Self-review section must immediately follow the Spec-coverage table and precede any final summary.
