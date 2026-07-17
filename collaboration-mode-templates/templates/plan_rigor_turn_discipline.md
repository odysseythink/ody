## Rigor tier addendum: Turn discipline (when to submit the plan)

Plan mode sessions must follow strict turn discipline to avoid loops and ensure clear approval gates.

### Turn ending rules (non-split plans)

Every turn in a non-split plan must end with exactly ONE of:

1. **`request_user_input`** — if you need clarification
   - Use when a material ambiguity remains that prevents you from writing the plan.
   - One question per turn; wait for the answer before proceeding.
   - Do NOT mention "the plan" (user cannot see it yet).

2. **submit_plan** — when the plan is decision-complete
   - Use when the plan is ready for user approval.
   - Call the `submit_plan` tool with the full plan markdown as the `plan` argument.
   - Never ask about approval via text — that is `submit_plan`'s job.

**Never mix them in one turn:** Do not call `request_user_input` and `submit_plan` in the same turn.

### Turn ending rules (split plans)

Split plans have different discipline because parts are written sequentially. `submit_plan` always writes to the single index file — it can never create a part file — so writing a part and updating the index's manifest are two different actions:

**While parts are pending:**
- Write ONE part file per turn, with a normal file-write tool (not `submit_plan`), at `<index-stem>/<part-name>.md`
- Then call `submit_plan` with the index's full markdown, that part's manifest row flipped to `done`
- As long as any row in the markdown you pass to `submit_plan` is still `pending`, that call saves the index and keeps Plan mode active — it does not end the turn
- Do NOT call `request_user_input` this turn
- Stop after the `submit_plan` call that flips the row; injection will direct you to the next pending part

**After all parts are done:**
- Do cross-file consistency review (no additional parts written)
- Call `submit_plan` with the index markdown showing every row `done` — this is the call that requests approval and ends Plan mode

### Specific rules

#### Rule 1: Do NOT ask about approval via text
- ❌ "Is this plan OK?" (in plain text)
- ❌ "Shall I proceed with this plan?" (in `request_user_input`)
- ✅ "Do you prefer Approach A or B?" (in `request_user_input`, to clarify spec)
- ✅ Call `submit_plan` with the final plan markdown (submit_plan's job)

#### Rule 2: Do NOT reference "the plan" in `request_user_input`
- ❌ "Does the plan cover enough detail?"
- ❌ "Should the plan include X?"
- ✅ "Should we include caching in the implementation?"

Why? The user cannot see the plan until you call `submit_plan` with no pending parts left. Asking about the plan confuses them.

#### Rule 3: `request_user_input` expects multiple choice
When using `request_user_input`, provide 2-4 meaningful options:
- ✅ Options are mutually exclusive (user picks one)
- ✅ Each option materially changes the spec/plan
- ❌ Options include filler ("Other: specify"); avoid generic catch-alls

#### Rule 4: submit_plan always carries the full index
- Pass the full index markdown as the `plan` argument to `submit_plan`, every time — including incremental row flips during a split
- Include everything the index should currently contain (not a delta against the previous call)
- Never call `submit_plan` more than once per turn

#### Rule 5: If the user rejects the plan
- User responds without selecting an option (stays in plan mode)
- Revise the plan based on their feedback
- Call `submit_plan` with a new complete plan markdown (complete replacement, not delta)

### Example turn sequence (non-split plan)

**Turn 1:**
```
Goal: Implement search redesign.
...
Should we use Elasticsearch or a database query builder for the search index?

- Option A: Elasticsearch (more powerful, external dependency)
- Option B: Database query builder (simpler, built-in)
```
→ Ends with `request_user_input`

**Turn 2:**
```
User selected Option B.
...
```
→ Calls `submit_plan` with:
```
# Search Redesign — Implementation Plan
... (complete plan)
```
→ No `## Parts` table, so this call ends Plan mode (approval requested)

**Turn 3 (if user has feedback):**
```
User: "This looks good but add a performance test."
```
→ Calls `submit_plan` again with:
```
# Search Redesign — Implementation Plan
... (plan WITH added performance test task)
```
→ Complete replacement, ends Plan mode again

### Example turn sequence (split plan)

**Turn 1:** Call `submit_plan` with:
```
# Design Mode — Implementation Plan (Index)

**Goal:** ...

## Parts
| # | File | Scope | Status |
|---|---|---|---|
| 1 | <stem>/protocol.md | Protocol types | pending |
| 2 | <stem>/config.md | Config + instructions | pending |
| 3 | <stem>/schema.md | Schema + tests | pending |
```
→ 3 rows are `pending`, so this call saves the index only — Plan mode stays active, no `request_user_input`. Injection will direct to Part 1.

**Turn 2:**
- Write `<stem>/protocol.md` with a normal file-write tool:
   - The `<stem>` directory is date-slug-prefixed and revealed in the `submit_plan` response — never guess it.
```
## Part 1 (from previous turn's injection)

### Task 1: Verify ModeKind::Design...
...
```
- Then call `submit_plan` with the index markdown, Part 1's row flipped to `done` (Parts 2-3 still `pending`)
→ Still 2 rows `pending`, so Plan mode stays active. Injection directs to Part 2.

**Turn 3:**
- Write `<stem>/config.md`
- Call `submit_plan` with the index markdown, Parts 1-2 `done` (Part 3 still `pending`)
→ Still 1 row `pending`, Plan mode stays active. Injection directs to Part 3.

**Turn 4:**
- Write `<stem>/schema.md` (final part)
- Cross-file consistency review: all dependencies valid ✓
- Call `submit_plan` with the index markdown, all 3 rows `done`
→ No rows `pending`, so this call ends Plan mode (approval requested)

### When to ignore these rules

**Rare exceptions (use sparingly):**
- If the user explicitly says "just execute this," you may skip calling `submit_plan` (but this is outside plan mode).
- If the user changes their mind mid-plan and asks to abort, stop and exit plan mode.

Otherwise, always follow the turn discipline.
