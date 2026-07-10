## Rigor tier addendum: Risks & Open Questions

In addition to the Dependency Overview, Spec-coverage table, and Self-review, every rigor-tier plan MUST identify and mitigate key risks before finalization. Never defer risks to "later phases" — surface them explicitly and include concrete mitigations in the plan.

### Risks & Open Questions table

Before finalizing, add a `## Risks & Open Questions` section containing a table with columns `#`, `Risk`, `Mitigation`.

| # | Risk | Mitigation |
|---|---|---|
| R1 | Concrete risk description | Concrete mitigation strategy or prerequisite task |
| R2 | Concrete risk description | Concrete mitigation strategy or prerequisite task |
| R3 | Concrete risk description | Concrete mitigation strategy or prerequisite task |

Every row must have:
- A **concrete, specific risk** (not vague like "things may break").
- A **concrete mitigation** (not "we'll handle it" or "TBD").

If a mitigation is "add a prerequisite task," name it: "Prerequisite Task 0: <name>".
If a mitigation is "covered by Task N," reference it explicitly: "Covered by Task 3 (schema fixture regeneration)".

### Common categories of risks

Consider risks in these areas (not exhaustive):

- **Implementation complexity / unknowns:**
  - Third-party library behavior or edge cases.
  - Intricate state machine or algorithm correctness.
  - Undocumented or ambiguous API semantics.
  
- **Performance / scalability:**
  - Query complexity on large datasets.
  - Memory usage under concurrent load.
  - Network latency in distributed operations.

- **Versioning / compatibility:**
  - Schema migration or data transformation.
  - Backward compat for existing clients or configs.
  - Third-party dependency version bumps.

- **Integration / deployment:**
  - Phased rollout windows or dependencies between services.
  - Database migrations that lock tables.
  - Breaking changes to public APIs.

- **Security / validation:**
  - Input validation or sanitization edge cases.
  - Permission checks or authorization gaps.
  - Sensitive data exposure in logs or caches.

- **Concurrency / race conditions:**
  - Concurrent mutations to shared state.
  - Lock ordering or deadlock potential.
  - Eventual consistency in distributed systems.

### Example

For a design-mode protocol rollout:

| # | Risk | Mitigation |
|---|---|---|
| R1 | `ModeKind::Design` added to Rust enum in D0, but TypeScript/JSON schema fixtures still reflect old `["plan", "default"]` — clients won't recognize the new mode. | Task 4: Regenerate schema fixtures using `write_schema_fixtures` bin; fixture consistency tests ensure TS and JSON stay in sync. |
| R2 | `PlanModeConfigToml` name suggests Plan-mode-only; future maintainers may think Design mode uses different config. | Task 2: Add doc comment to `PlanModeConfigToml` struct: "Settings scoped to Plan mode and Design mode." |
| R3 | `collaboration_mode_list` endpoint might be silently filtering Design mode via a whitelist added by future maintainers, breaking the feature without obvious cause. | Task 1: Add explicit test assertion `assert!(modes.contains(&Some(ModeKind::Design)))` to prevent regression. |

### Finalization rule

Do NOT submit the plan if any risk remains unmitigated. For each risk, either:
1. Include a concrete mitigation task in the plan, or
2. Identify it as a prerequisite (Task 0 or earlier), or
3. Explain why it is not a true risk for this context.

A risk deferred to "future work" or "later phases" is still an unmitigated risk and must be addressed before the plan is finalized.
