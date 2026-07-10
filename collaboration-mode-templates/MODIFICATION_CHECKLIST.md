# Rigor Fragment Modification Checklist

A practical checklist for anyone editing `collaboration-mode-templates/templates/plan_rigor_*.md`
or the composition code in `core/src/context/collaboration_mode_instructions.rs`. Read
`FRAGMENT_CATALOG.md` first if you haven't touched these fragments before — this checklist
assumes you know which fragment you're editing and why.

## Before you edit

- [ ] Find the fragment's entry in `FRAGMENT_CATALOG.md`. Note its **change frequency** and
      **hard dependents** (fragments whose own text says "... above" referring to this one).
- [ ] If change frequency is "very low", double-check you have a concrete reason (a real
      incident, a real ambiguity a model hit in practice) — these fragments were kept minimal
      on purpose (e.g. SELFREVIEW's 7-item count, RENAME's "no silent survivors" rule).
- [ ] If you're changing what the fragment *asserts about itself* (adding/removing an "In
      addition to ... above" reference), check whether that changes its position in the
      dependency graph — you may need to move its `.with_rigor_*()` call in
      `collaboration_mode_instructions.rs` and update `rigor_fragment_graph.rs`.

## While editing content (`.md` files)

- [ ] Keep the existing heading style (`## Rigor tier addendum: <Name>` for most fragments;
      a few like GROUNDING/SCOPE/RENAME use a bare `## <topic>` heading — match the sibling
      fragments, don't invent a third style).
- [ ] If you reference another fragment's content ("see X above/below"), make sure that phrase
      stays true after your edit and after any composition-order change.
- [ ] Don't duplicate rules that already live in another fragment (e.g. the no-placeholders rule
      appears in both TASK_SKELETON and INVARIANTS — check both before adding a third copy
      elsewhere).
- [ ] If you introduce a new `{{ variable }}` placeholder, it must be wired through
      `render_plan_instructions` in `collaboration_mode_instructions.rs` the same way
      `split_threshold` is — a placeholder with no renderer will leak `{{ ... }}` literally into
      the model prompt.
- [ ] Concrete over abstract: every rule in these fragments exists to stop a real failure mode.
      If you're tightening a rule, prefer adding a worked example (see RISKS' or TASK_SKELETON's
      "Example" sections) over adding more prose.

## While editing composition code (`collaboration_mode_instructions.rs`)

- [ ] Adding a fragment: create the `.md` file, add the `pub const PLAN_RIGOR_<NAME>` in
      `collaboration-mode-templates/src/lib.rs`, add a `with_rigor_<name>()` method (copy the
      three-line shape of any existing one), and insert the call into the chain in
      `from_collaboration_mode` at the position its dependencies require.
- [ ] Reordering the chain: re-derive which HARD edges (see FRAGMENT_CATALOG.md) the new order
      violates, if any. Update `RIGOR_COMPOSITION_ORDER` and the `actual_chain` array in
      `core/src/context/rigor_fragment_graph.rs`'s test module to match.
- [ ] Never add a fragment call inside the `if tier == PlanModeTier::Rigor` block without also
      adding it to `RIGOR_FRAGMENT_GRAPH` — the drift test only catches fragments it knows about.

## Verify

- [ ] `cargo check -p ody-collaboration-mode-templates` — confirms the `.md` file compiles as an
      `include_str!` (i.e. exists at the expected path, valid UTF-8).
- [ ] `cargo test -p ody-core context::rigor_fragment_graph` — DAG validity + composition-order
      drift check (5 tests; see `rigor_fragment_graph.rs`).
- [ ] `cargo test -p ody-core collaboration_mode_instructions` — composition tests assert marker
      strings from each fragment are present in the rendered Rigor tier output (17 tests as of
      this writing). A silently-deleted fragment body will fail one of these loudly.
- [ ] `cargo check --workspace --all-targets` — this crate is included via `include_str!` in
      `core`, so a broken path or missing file is a compile error, not a runtime surprise.

## After merging

- [ ] Update `FRAGMENT_CATALOG.md` if you changed a fragment's dependencies, purpose, or change
      frequency — the catalog is only useful if it stays accurate.
- [ ] If the edit was prompted by a real conversation going wrong (a model looping, skipping a
      step, or misreading an ambiguous instruction), consider whether `RENAME`'s pattern is worth
      following: a one-paragraph "Historical reference" noting what happened, so the rule doesn't
      get quietly relaxed later by someone who doesn't know why it's there.

## Anti-patterns to avoid

- **Silently deleting a fragment's marker phrase while editing prose around it.** The
  `collaboration_mode_instructions.rs` tests assert on specific strings (e.g.
  `"## Rigor tier addendum: Structured workflow"`) — if you reword the heading, update the test,
  don't just let it start failing and then loosen the assertion.
- **Adding a 12th fragment without touching `RIGOR_FRAGMENT_GRAPH`.** The whole point of that
  module is that composition-order mistakes fail a fast unit test instead of shipping a plan
  where a fragment says "the table above" and there is no table above.
- **"Just shrinking" SELFREVIEW's 7 items.** The fragment says to "reproduce all seven items
  exactly" — the count and order are load-bearing for INVARIANTS/RISKS, which both cite
  "Self-review above" assuming it already ran. Don't drop or merge items without checking what
  else references them.
- **Copy-pasting TASK_SKELETON's no-placeholders section instead of referencing INVARIANTS.**
  They already overlap; a third near-duplicate makes future edits need three synced updates
  instead of two.
