## Rigor tier addendum: Minimal experiments (spikes)

In addition to the Dependency Overview, Spec-coverage table, Self-review, and the `## Risks & Open Questions` table above, a rigor-tier plan must not rest a load-bearing decision on an **unvalidated** assumption when a cheap spike could have settled it. The base "Minimal experiments (spikes)" section defines the trigger gate, the sandbox rule, and the data/credential rules — this addendum makes the discipline mandatory for the decisions that matter most, and wires it into the risk table.

### Mandatory for the most-expensive-if-wrong decisions

Name the **1–3 decisions that are most expensive if wrong** (the same ones Self-review audits deepest). For each, its correctness must be backed by exactly one of:

1. a **spike result** — a conclusion + data from a demo run under `.ody-code/spikes/`, with its assumption boundary stated; or
2. an **explicit, user-visible assumption** — a row in `## Risks & Open Questions` (or an `## Assumptions` list) whose mitigation is a **named validation task** ("Prerequisite Task 0: spike <X> against a read-only staging snapshot"), used when the trigger gate is not met, no sandbox is available to run the spike, or the user has deferred it.

A rigor plan that silently assumes a load-bearing unknown will hold — with neither a spike nor a surfaced assumption — is not decision-complete. Do not finalize it.

### Tie spikes to the risk table

For any risk in `## Risks & Open Questions` whose real severity is unknown until measured, the mitigation column must be concrete: either "verified by spike: <one-line conclusion + number>" or a named validation task to run the spike. "We'll validate later" is an unmitigated risk and blocks finalization, exactly as the Risks addendum requires.

### Spikes never become tasks

A spike is throwaway. Its **conclusion and data** feed the plan (an assumption confirmed, a task's approach chosen, a risk retired); its **code is discarded**. Never turn spike code into an implementation task, and never cite "the spike already does it" as a reason to skip a task's own test-first loop.
