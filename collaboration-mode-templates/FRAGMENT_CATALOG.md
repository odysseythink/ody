# Plan Mode Rigor Tier — Fragment Catalog

The Rigor tier of Plan mode is composed from 11 independent Markdown fragments in
`collaboration-mode-templates/templates/plan_rigor_*.md`, registered as constants in
`collaboration-mode-templates/src/lib.rs`, and chained onto the base `plan.md` instructions in
`core/src/context/collaboration_mode_instructions.rs::from_collaboration_mode`.

This document is the map: what each fragment does, what it assumes is already in context, and
the safe order to add or edit them in. It exists so that changing one fragment doesn't silently
break the sentence that follows it (several fragments literally start with "In addition to ... above").

## The tier contract (read this before adding tier-specific text anywhere)

Base `plan.md` renders for **every** tier. Fragments render for **one** tier. So any rule that only
one tier obeys MUST live in that tier's fragment — never in the base. Putting it in the base ships
it to the other tier too, where it either contradicts that tier's own addenda or dangles with no
definition. Both failure modes have already happened; see WORKFLOW and CONCISE below.

Each Plan-mode prompt is therefore exactly: `tier declaration` + `base plan.md` + `that tier's
fragments`, and nothing else:

```rust
this = match tier {
    PlanModeTier::Concise => this.with_concise_contract(),
    PlanModeTier::Rigor | PlanModeTier::Auto => this.with_rigor_contract(),
};
this = this.with_plan_tier_declaration(tier);
```

`with_plan_tier_declaration` prepends the host-resolved tier at the top of the prompt. It is the
single authority on which tier is in force — fragments must never ask the model to infer its own
tier (from request length, task size, or which sections rendered). `Auto` is not a renderable tier;
`resolve_plan_mode_tier` normalizes it away before this match.

## Composition order (source of truth: the code)

The Rigor chain, inside `with_rigor_contract()`:

```rust
this.with_rigor_workflow()
    .with_rigor_coverage()
    .with_rigor_task_skeleton()
    .with_rigor_selfreview()
    .with_rigor_invariants()
    .with_rigor_grounding()
    .with_rigor_scope()
    .with_rigor_rename()
    .with_rigor_risks()
    .with_rigor_split()
    .with_rigor_turn_discipline();
```

If you change this order, re-check every "in addition to ... above" reference below — those
sentences are only true if the referenced fragment actually rendered earlier in the string.

The Concise tier is a single fragment (CONCISE), so it has no order to maintain and is **not** part
of `RIGOR_FRAGMENT_GRAPH` — that graph and its drift test cover the rigor chain only.

## Fragment reference

### 0. CONCISE — `plan_concise.md` (the only non-rigor fragment)

- **Tier:** Concise. Everything else in this catalog is Rigor.
- **Says:** the autonomy rules (resolve unknowns by exploration, choose labelled defaults, never
  block finalization) and the compact writing style (3-5 short sections; grouped bullets over
  file-by-file inventories), plus an explicit list of what the concise tier does *not* ask for
  (`### Task N` sections, step checkboxes, Dependency Overview, Spec-coverage, Self-review).
- **Depends on:** nothing. It is the whole concise contract in one file.
- **Why it exists:** both halves used to live in base `plan.md` — the autonomy rules as
  "## Concise-tier autonomy" and the writing style inside "Finalization rule". Because base renders
  for every tier, rigor prompts received both. The writing style told them to compress to 3-5
  sections while their own addenda demanded per-task detail with real code. Worse, the autonomy
  section opened with "When the user's request is brief ... (the **concise** plan tier)", telling
  the model to self-classify by request length — a heuristic the host had already abandoned
  (`select_tier` ignores the prompt entirely). A terse request could therefore talk a rigor-tier
  session into writing a concise plan. It also referenced a "standard" tier that never existed in
  `PlanModeTier`.
- **Change frequency:** low.
- **Guarded by:** `rigor_tier_does_not_receive_concise_rules`, `base_template_is_tier_neutral`, and
  `concise_tier_has_no_dangling_rigor_references` in
  `core/src/context/collaboration_mode_instructions.rs`.

### 1. WORKFLOW — `plan_rigor_workflow.md` (44 lines)

- **Says:** the 5-step method (Understand → File Structure → Dependency Overview → Write the
  plan incrementally → Self-review), plus the mandatory plan document header (Title/Goal/
  Architecture/Tech Stack/Execution note).
- **References forward:** step 3 says "Dependency Overview" and step 5 says "Self-review" —
  both are only defined later, in COVERAGE and SELFREVIEW. This fragment is a forward-looking
  table of contents, not a self-contained spec.
- **Depends on:** nothing (first fragment; only assumes the base `plan.md` instructions).
- **Referenced by:** nothing explicitly, but COVERAGE and TASK_SKELETON assume the reader has
  already seen "the workflow" as framing.
- **Change frequency:** very low. This is the spine; changing the 5 steps ripples everywhere.
- **Shares step 1 with base `plan.md`, deliberately.** Step 1 (**Understand**) is exploration
  technique, which is tier-neutral — every plan-mode session needs it, so base carries an identical
  copy under "Exploration technique (every tier)". Two tests in
  `collaboration-mode-templates/src/lib.rs` pin this: `plan_templates_only_name_tools_that_exist`
  (both copies must steer at `grep`/`glob`/`read_file`, or the model shells out to `rg`/`cat` and
  burns context) and `plan_and_rigor_workflow_share_the_same_understand_step` (the two copies must
  not drift, because `plan_mode_injector::render_full_reminder()` re-injects this fragment
  mid-session). Steps 2-5 and the document header are rigor-only and live here alone.
- **Do not inline the rest into `plan.md`.** It was, once, as "PHASE 3A" — a verbatim copy minus
  the `(see Self-review addendum)` cross-reference. Rigor prompts rendered the workflow twice;
  concise prompts rendered it *without* the addenda that define its steps, leaving "verify all
  seven items" pointing at nothing, and models invented a seven-item checklist to fill the gap.
  Guarded by `rigor_workflow_is_not_duplicated_in_base_template` in
  `core/src/context/collaboration_mode_instructions.rs`.

### 2. COVERAGE — `plan_rigor_coverage.md` (23 lines)

- **Says:** Dependency Overview rules (DAG, `Depends on:` edges only point backward, shared-
  signature tasks must update all callers) + the Spec-coverage table format
  (`Requirement | Task(s) | Status`, with `covered`/`GAP`/`no-op`).
- **Explicit dependency phrase:** "In addition to the conversational plan requirements above" —
  i.e. the base `plan.md`, not a specific rigor fragment.
- **Depends on:** WORKFLOW (conceptually — this is the detailed version of workflow steps 2-3).
- **Referenced by (explicit "above" text in the child):** SELFREVIEW, INVARIANTS, RISKS all say
  "In addition to the Dependency Overview, Spec-coverage table[, and Self-review] above" — so
  COVERAGE must render before all three.
- **Change frequency:** low. The DAG/spec-coverage contract is load-bearing for SELFREVIEW item 1
  and item 4.

### 3. TASK_SKELETON — `plan_rigor_task_skeleton.md` (171 lines, largest)

- **Says:** the `### Task N` header format (Depends on / Files: Create/Modify/Test), the
  test-first loop for testable code (write failing test → run → fail → implement → run → pass →
  commit, with a full worked example), the parallel loop for non-testable code (complete code →
  build/typecheck → manual verification → commit), the shared-signature caller-update protocol,
  and the no-placeholders rule (duplicated in INVARIANTS — see below).
- **Depends on:** WORKFLOW (task structure) and COVERAGE (the `Depends on:` field mirrors the
  Dependency Overview edges). No explicit "above" phrase in the text itself.
- **Referenced by:** SELFREVIEW items 5-6 (caller/build soundness, test-the-risk) assume tasks
  were written in this shape.
- **Change frequency:** medium. This is the fragment most likely to need new worked examples or
  tightened test-first language.
- **Note:** its "no placeholders or pseudocode" section (lines 161-171) is a near-duplicate of
  INVARIANTS' "No-placeholders rule" — if you tighten one, check the other for drift.

### 4. SELFREVIEW — `plan_rigor_selfreview.md` (13 lines, smallest)

- **Says:** the 7-item self-review checklist (spec-coverage, placeholder scan, no phantom tasks,
  dependency soundness, caller & build soundness, test-the-risk, type consistency), to be
  reproduced verbatim as `- [ ]` checkboxes.
- **Explicit dependency phrase:** "In addition to the Dependency Overview and Spec-coverage table
  above" → requires COVERAGE to have rendered first.
- **Depends on:** COVERAGE (hard requirement, textually referenced).
- **Referenced by:** INVARIANTS and RISKS both say "...and Self-review above" → both require
  SELFREVIEW to render before them.
- **Change frequency:** very low. The instruction is to "reproduce all seven items exactly" —
  the count is load-bearing (INVARIANTS and RISKS both cite "Self-review above" assuming these
  seven ran); do not add/remove items casually.

### 5. INVARIANTS — `plan_rigor_invariants.md` (31 lines)

- **Says:** the shared-signature build-green invariant (same task updates every caller +
  `cargo check --workspace --all-targets`) and the no-placeholders rule (no TODO/TBD, no "similar
  to Task N", no manufactured phantom tasks).
- **Explicit dependency phrase:** "In addition to the Dependency Overview, Spec-coverage table,
  and Self-review above" → requires COVERAGE and SELFREVIEW to have rendered first.
- **Depends on:** COVERAGE, SELFREVIEW (both hard requirements, textually referenced).
- **Referenced by:** nothing explicit, but this is the canonical maps to ody-code's
  SHARED_SIGNATURE + NO_PLACEHOLDERS.
- **Change frequency:** medium — high risk. Changing the caller-update rule affects every task in
  every generated plan.

### 6. GROUNDING — `plan_rigor_grounding.md` (27 lines)

- **Says:** every claim in the plan must be grounded in Read/Grep evidence before being added;
  ungrounded items get a `[C:INFERRED]` label; names must not be trusted as concept-matches
  (worked examples: OS "account" vs app "account"); no invented Depends-on edges.
- **Depends on:** nothing explicit (no "above" phrase) — it's addressed at the whole plan, not
  chained off a specific earlier fragment.
- **Referenced by:** SCOPE explicitly says "In addition to the source-grounding mandate above" →
  GROUNDING must render before SCOPE.
- **Change frequency:** low. The `[C:INFERRED]` labeling convention is referenced nowhere else,
  so it's safe to tune independently.

### 7. SCOPE — `plan_rigor_scope.md` (23 lines)

- **Says:** every plan needs an explicit `## Out-of-scope` section listing name-matches that
  looked relevant but are a different concept, each with symbol/path + one-line reason.
- **Explicit dependency phrase:** "In addition to the source-grounding mandate above" → requires
  GROUNDING to have rendered first.
- **Depends on:** GROUNDING (hard requirement, textually referenced).
- **Referenced by:** RENAME's "carve out" option points hits into this section, but RENAME has no
  textual "above" dependency on it — treat as a soft/conceptual link.
- **Change frequency:** low.

### 8. RENAME — `plan_rigor_rename.md` (22 lines)

- **Says:** for any "remove concept X" task, every matching symbol/field/file must get an
  explicit delete/rename/carve-out decision with a one-line evidence-based reason; no hit may be
  silently left unaddressed. Includes a historical incident (`auth.rs::PlanType`) as justification.
- **Depends on:** conceptually SCOPE (its "carve out" action feeds SCOPE's Out-of-scope section)
  and GROUNDING (evidence requirement), but no explicit "above" phrase.
- **Referenced by:** nothing explicit.
- **Change frequency:** very low — this fragment exists because of one specific incident; don't
  water down the "no silent survivors" rule without a comparably concrete reason.

### 9. RISKS — `plan_rigor_risks.md` (73 lines)

- **Says:** every plan needs a `## Risks & Open Questions` table (`# | Risk | Mitigation`) with
  concrete (not vague) entries; lists 6 common risk categories (complexity, performance,
  versioning, integration, security, concurrency) with a worked example; forbids deferring risks
  to "later phases".
- **Explicit dependency phrase:** "In addition to the Dependency Overview, Spec-coverage table,
  and Self-review" → requires COVERAGE and SELFREVIEW to have rendered first.
- **Depends on:** COVERAGE, SELFREVIEW (hard requirements, textually referenced).
- **Referenced by:** nothing explicit.
- **Change frequency:** medium — the risk-category list may grow as new failure classes are
  discovered in review.

### 10. SPLIT — `plan_rigor_split.md` (178 lines, largest)

- **Says:** when to split a plan (>`{{ split_threshold }}` tasks or multi-subsystem), the
  index-file vs part-file structure (`<id>/` subdirectory, files-next-to-index rejected by the
  write guard), the Parts manifest table format, and the turn-by-turn writing protocol (write
  index → one part per turn → flip manifest row to `done` → final cross-file review).
- **Uses the `{{ split_threshold }}` template variable**, rendered by
  `render_plan_instructions` in `collaboration_mode_instructions.rs` (default `8` if unset).
- **Depends on:** conceptually all prior fragments (a split plan still needs Coverage, Task
  Skeleton, Risks etc. — split only changes *file layout*, not content requirements). No explicit
  "above" phrase.
- **Referenced by:** TURN_DISCIPLINE has a dedicated "Turn ending rules (split plans)" section
  that only makes sense once SPLIT's manifest concept has been introduced.
- **Change frequency:** low — the protocol is intentionally rigid to survive context compaction
  mid-generation; treat changes here as high-blast-radius.

### 11. TURN_DISCIPLINE — `plan_rigor_turn_discipline.md` (163 lines)

- **Says:** every turn in a non-split plan ends with exactly one of AskUserQuestion (never
  mentioning "the plan", since the user can't see it yet) or ExitPlanMode (full plan in
  `<proposed_plan>`); never both in one turn. Split plans get separate rules: one part per turn,
  no AskUserQuestion/ExitPlanMode until all parts are `done`, then a final cross-file review before
  ExitPlanMode.
- **Depends on:** SPLIT (its "split plans" branch is meaningless without SPLIT's manifest
  vocabulary already established).
- **Referenced by:** nothing (last fragment).
- **Change frequency:** medium — this is the fragment most likely to need adjustment as we
  observe real conversations looping or asking premature approval questions.

## Dependency graph (textually verified, not inferred)

```
WORKFLOW (no deps)
  │
  ├─▶ COVERAGE (soft: elaborates workflow steps 2-3)
  │     │
  │     ├─▶ TASK_SKELETON (soft: needs workflow + coverage framing)
  │     │
  │     ├─▶ SELFREVIEW (HARD: "Dependency Overview and Spec-coverage table above")
  │     │     │
  │     │     ├─▶ INVARIANTS (HARD: "...and Self-review above")
  │     │     │
  │     │     └─▶ RISKS (HARD: "...and Self-review")
  │     │
  │     └─▶ RISKS (HARD, same as above — RISKS needs both COVERAGE and SELFREVIEW)
  │
  ├─▶ GROUNDING (no deps)
  │     │
  │     └─▶ SCOPE (HARD: "source-grounding mandate above")
  │           │
  │           └─▶ RENAME (soft: shares "carve out" vocabulary)
  │
  └─▶ SPLIT (no deps, gated by task count)
        │
        └─▶ TURN_DISCIPLINE (soft: split-plan branch needs SPLIT's manifest concept)
```

**HARD** = the fragment's own text says "above" and will read as a non-sequitur if reordered.
**soft** = conceptual dependency; reordering won't produce a grammatically broken sentence, but
will read worse or repeat context.

The current composition order in `collaboration_mode_instructions.rs` satisfies every HARD edge
(verified above). `fragment_metadata.rs` (see Phase 2 of the optimization roadmap) encodes this
same graph as data so a test can catch future violations automatically.

## How to add a new fragment

1. Decide its content and where it sits in the dependency graph above (what must render before
   it, what — if anything — reads oddly without it).
2. Create `collaboration-mode-templates/templates/plan_rigor_<name>.md`.
3. Register the constant in `collaboration-mode-templates/src/lib.rs`:
   ```rust
   pub const PLAN_RIGOR_<NAME>: &str = include_str!("../templates/plan_rigor_<name>.md");
   ```
4. Add a `with_rigor_<name>()` method in `core/src/context/collaboration_mode_instructions.rs`
   (copy an existing one — they're all the same three-line shape).
5. Insert the call into the chain in `from_collaboration_mode` at the position dictated by its
   dependencies.
6. Add/update a test in `core/src/context/collaboration_mode_instructions.rs`'s test module
   asserting the new fragment's marker text appears in the composed output.
7. Update this catalog: add a numbered section, and add the new node/edges to the dependency
   graph above.

## How to edit an existing fragment

1. Check this catalog for the fragment's **change frequency** and **hard dependents** before
   editing — a fragment with hard dependents can't have its "In addition to ... above" framing
   removed without updating the dependent's opening sentence too.
2. Edit the `.md` file directly; no code changes needed for content-only edits.
3. If you add a new `{{ variable }}` placeholder, wire it through
   `render_plan_instructions` (and its call site) the same way `split_threshold` is handled.
4. Run `cargo test -p ody-core collaboration_mode_instructions` — the composition tests assert
   marker strings are present, so most accidental deletions will fail loudly.
5. If the edit changes the fragment's *dependencies* (not just wording), update this catalog's
   entry and the dependency graph.

## Do not reorder the chain without re-reading this file

The 11-call chain in `from_collaboration_mode` is not arbitrary — it is one valid topological
sort of the HARD-edge graph above. There are other valid orderings, but changing the order
without checking the HARD edges can silently produce a rigor-tier plan where SELFREVIEW says
"the Dependency Overview and Spec-coverage table above" and there is no such table above.
