---
type: inline
name: roadmap-architect
description: >
  Turn a large multi-phase roadmap into an execution-ready plan by splitting oversized goals
  (so an AI executor never silently drops scope), ordering real source-level dependencies, judging
  what can run in parallel, and labelling each sub-task with the execution mode it should run in
  (normal / plan / design). Use this skill whenever the user wants to "拆分 roadmap", "细化路线图",
  "标注每个阶段用什么模式跑", "排依赖 / 看能不能并行", "防止 AI 执行时遗漏功能", or asks which
  sub-tasks are safe in normal mode vs need plan mode vs need design mode. The defining feature is
  ROI discipline: it does the high-leverage work once (one shared rubric, applied in a single batch
  pass over every phase) instead of re-deriving the same judgement phase-by-phase across many turns.
  Do NOT use this skill to actually execute a roadmap task — it only produces and refines the plan.
---

# SKILL: Roadmap Architect (路线图架构师) v1

This skill answers one question: **"I have a big roadmap — how do I make it safe and cheap for an
AI to execute, without burning tokens re-deciding the same thing for every phase?"**

It produces four annotations on an existing roadmap:
1. **Split** oversized goals into bite-sized sub-phases (so scope can't be silently dropped).
2. **Dependencies** — a real, source-grounded graph; what's serial, what's parallel.
3. **Execution mode** per sub-task — `normal` / `plan` / `design`.
4. A short **rubric** at the top of the doc, so future edits stay consistent.

---

## The ROI Rule (read first — this is the whole point)

The expensive failure mode is **re-deriving the same judgement phase-by-phase across separate turns**
(audit 4.1, then 4.2, then 4.3, … each turn re-reading the roadmap and re-inventing the mode criteria).
That is mechanical repetition dressed up as analysis, and it scales token cost linearly with no extra insight.

Do the opposite:

- **Decide the rules ONCE.** Produce a single rubric (split-granularity principles + the three mode-decision
  criteria) and get the user to approve it. This is the one high-leverage human checkpoint.
- **Apply the rules in ONE batch pass** over every phase, not one conversation per phase.
- **Spend real effort only where AI beats a human:** source-level dependency analysis and decomposition.
  Grep the actual code; never invent dependencies from the phase titles.

If the user asks you to process phases one at a time, offer to batch them: "I can define the mode rubric
once and apply it to all of 4.1–4.5 in a single pass — cheaper and more consistent than going section by
section. Want that?"

---

## Step 1 — Establish the rubric (one turn, then a checkpoint)

Read the **whole** roadmap once, plus enough source to ground the dependency calls (use Read / Grep / Glob,
or `Agent(subagent_type="explore")` for wide fan-out). Then write a `## Execution Rubric` block at the top
of the roadmap containing:

**A. Split-granularity principles** — when one roadmap item is too big and must be split. Default triggers:
a sub-phase touches >~8 distinct files/modules, mixes shared-infra changes with leaf work, or bundles
independently-shippable pieces under one heading ("migrate all builtin tools"). Each split sub-phase must be
independently testable.

**B. The three mode-decision criteria** (this software's real mode semantics):

| Mode | When a sub-task belongs here | Why |
| --- | --- | --- |
| **normal** | Mechanical, low-risk, one obviously-correct answer; isolated change with no shared-signature/architecture decision. | normal mode can edit code directly — no planning overhead needed. |
| **plan** | Multi-step implementation with real dependencies, shared-signature/caller fan-out, or anything that benefits from a task-by-task TDD plan before touching code. | plan mode forbids edits except the plan file and forces a dependency graph + test-first task list. |
| **design** | Genuine unknowns in architecture, data model, public interface/contract, or migration semantics — where guessing wrong wastes large work. | design mode hard-gates all implementation until the user approves a spec, so unexamined assumptions get caught first. |

Tie-break: if a sub-task could be two modes, pick the **more cautious** one (design > plan > normal) only
when there's a real unknown; otherwise prefer the cheaper one. Don't escalate routine work to design.

**Then STOP and present the rubric for approval** (use AskUserQuestion if available). The user's domain
knowledge corrects the criteria here, cheaply, before you apply them 40 times. This is the single checkpoint.

---

## Step 2 — Batch-apply across every phase (one turn)

Once the rubric is approved, sweep **all** phases in a single pass. For each phase:

1. **Split** per principle A. Number sub-phases (e.g. 4.4.0–4.4.8). 4.4.0 is usually the shared-infra
   sub-phase that later siblings depend on.
2. **Dependencies** — for each sub-phase, grep the actual code it touches and record `Depends on:` only
   where a real symbol/file/contract dependency exists. State explicitly which sub-phases can run in parallel.
   Render a small ASCII dependency graph per phase.
3. **Mode** — tag each sub-phase `[normal]` / `[plan]` / `[design]` per criteria B, with a one-line reason
   grounded in what the code actually requires (not a generic justification).

Update the roadmap's overview table, dependency section, and `Last Updated` line in the same pass.

---

## Step 3 — Self-review (reproduce as checkboxes in your reply)

- [ ] Every oversized phase split; no sub-phase bundles independently-shippable work.
- [ ] Every `Depends on:` points at an EARLIER sub-phase and is backed by a real grep/Read, not a guess.
- [ ] Parallelism stated explicitly per phase (don't leave "can these run together?" unanswered).
- [ ] Every sub-task has exactly one mode tag with a code-grounded one-line reason.
- [ ] The rubric block is present at the top so future edits stay consistent.
- [ ] You did this as ONE batch pass after ONE approved rubric — not phase-by-phase across many turns.

---

## Anti-patterns (the low-ROI traps this skill exists to prevent)

- **Phase-by-phase re-litigation.** Running the same "which mode?" prompt as a fresh conversation for 4.1,
  4.2, 4.3… Re-reads the roadmap every time, re-invents the criteria every time, costs tokens linearly.
- **Title-only dependencies.** Declaring dependencies from phase names without grepping the code. The
  dependency graph is the highest-value output; if it isn't source-grounded it's worse than nothing.
- **Mode inflation.** Tagging routine leaf work `design` "to be safe". Design mode is for real unknowns;
  over-using it turns the plan into a wall of approval gates and trains the user to ignore them.
- **Over-splitting.** Decomposing below the level the executor's certainty supports. Plans more precise
  than the work is predictable get rewritten after the first real execution — wasted up-front tokens.

> Remember the test of whether this was worth doing: **the roadmap actually gets executed sub-phase by
> sub-phase.** If it's destined to be a document nobody drives execution from, say so and stop early —
> a beautifully annotated roadmap that's never executed is pure sunk cost.
