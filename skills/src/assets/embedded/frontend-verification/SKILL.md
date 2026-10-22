---
name: frontend-verification
description: Use when building, changing, or reviewing UI. Defines the workflow that turns a rendered page into judgement — capture, inspect, fix, re-capture — plus the coverage tiers, the checks that must pass, and how to report what was verified and what was not. Load it before the first screenshot of a task, and again before claiming a screen is done.
---

# Frontend Verification

A screen is not the code you wrote. It is what the browser paints. Everything here exists so that "it looks good" becomes something you actually saw, at a size the user will see it, with the states the user will hit — and so that the difference between *checked* and *assumed* stays visible in your report.

Craft rules live elsewhere: `frontend-design` owns direction and composition, `make-interfaces-feel-better` owns interaction detail, `motion-design` owns timing. This skill only governs how you look, judge, and close.

## The loop

Every task that changes pixels runs this loop, in order. Skipping a step is not a shortcut; it moves the failure to the user.

1. **Baseline before the edit.** Capture the current state at the widths you will compare against. A redesign with no "before" cannot answer "is this better", only "is this different".
2. **Look at the baseline.** Read the images. Name what is wrong with the current screen in one sentence per problem. If you cannot name a problem, you are not ready to change it.
3. **Build toward the stated direction.** Only the part under change; do not restyle adjacent surfaces to match your new idea.
4. **Re-capture the same frames.** Same route, same widths, same states, same file naming. Different camera, no comparison.
5. **Judge against the checks below, then fix and re-capture.** One hypothesis per iteration. A fix that is not re-captured is unverified.
6. **Stop when the exit condition holds** (below), then report with evidence.

## State layout

Keep the loop's artifacts in the repository, not in your head:

```
.ody-code/frontend/
  design-system.md      # durable visual contract: tokens, type scale, spacing, motion
  shots/<route>@<width>@<state>.png
  audit.md              # appended per round: what was checked, what was found, verdict
```

`<route>` is a short slug (`home`, `login`, `settings-billing`). `<width>` is a CSS pixel number. `<state>` is `default`, `empty`, `error`, `loading`, `hover`, `focus`, or `open`.

Never overwrite a baseline with a re-capture: write `...@1280@after.png` next to `...@1280@before.png`. Two files with the same name across rounds cannot be compared, and the audit loses its evidence.

## Coverage tiers

Pick the tier from the size of the change, and say which one you ran. This is what keeps a one-button change from turning into a full design review.

| Tier | When | Frames | Checks |
| --- | --- | --- | --- |
| `quick` | one component, copy, spacing, a single state | 1 primary path × 2 widths (mobile + desktop) × the state you touched, ≤ 6 frames | console clean, overflow, clipping, hit area |
| `full` | a new screen, a redesign, layout or type-scale change, a flow with several steps | every affected route × 5 widths (360 / 768 / 1024 / 1280 / 1440) × 3 states (default / empty / error, or default / hover / focus for interactive parts) | all six checks, plus the craft pass |

If the project already has breakpoints, use the project's widths instead of the default five and say so.

## Capture

Use the browser tools — not a mental model of the page.

```
browser__navigate        { url, wait_until: "networkidle" }
browser__set_viewport    { width: 1280, height: 900 }        # repeat per width
browser__screenshot      { path: ".ody-code/frontend/shots/home@1280@before.png" }
view_image               { path: ".ody-code/frontend/shots/home@1280@before.png" }
browser__read_logs       { kind: "console", level: "error" }
```

`browser__screenshot` with `path` writes the PNG into the workspace; the call is only useful once `view_image` reads it back. A screenshot you did not open is not evidence. The `path` form never truncates — the base64 form does, and a truncated PNG cannot be decoded at all.

`browser__set_viewport` changes layout for real (it is not a resize of the screenshot). Clear it with `{ reset: true }` when the capture pass ends so later work is not stuck at 360px.

For a state that only appears on interaction, drive it: `browser__click` at a point, `browser__type` into a selector, then capture. For key events there is no first-class tool — `browser__execute_raw_cdp` with `Input.dispatchKeyEvent` works in local browser mode only, and is refused for an external browser. If you cannot reach the state, capture the closest reachable one and label the rest **Not verified**.

## The six checks

Run these on every frame in scope. They are cheap, objective, and they catch the failures that survive a visual skim.

| # | Check | Pass condition | How |
| --- | --- | --- | --- |
| 1 | Console | zero `error` entries produced by the actions of this round | `browser__read_logs { kind: "console", level: "error" }` |
| 2 | Horizontal overflow | no page-level horizontal scroll, no element past the right edge | `references/checks.md` § Overflow |
| 3 | Clipping | no visible text cut off inside a fixed-height or hidden-overflow box | `references/checks.md` § Clipping |
| 4 | Hit area | interactive targets ≥ 40×40 CSS px (≥ 44 on touch-first layouts) | `references/checks.md` § Hit area |
| 5 | Contrast | body text ≥ 4.5:1, large text and UI boundaries ≥ 3:1 | `references/checks.md` § Contrast |
| 6 | Focus | keyboard focus is visible and reachable in a sensible order | `references/checks.md` § Focus |

A console `error` is a failed check even when the frame looks perfect: a component that threw during hydration will break on the next interaction.

`references/checks.md` holds the exact `browser__evaluate` expressions. Use them instead of re-deriving the arithmetic — hand-computed contrast and off-by-one overflow judgements are how false passes get written.

## Judging what the numbers cannot see

The six checks are the floor. Above it, judge with the vocabulary the craft skills already define, and do not re-invent it:

- **Hierarchy** — one primary action; what matters reads first within two seconds (`frontend.md` quality bar).
- **Detail** — one spacing scale, all states present, composition holds between the captured widths.
- **Motion** — a purpose per animation, timings from `motion-design`'s budget table, `prefers-reduced-motion` respected.
- **Consistency** — repeated elements share tokens from `design-system.md`; nothing is one-off "just this time".
- **Comparison** — with before and after side by side, name what improved in one line per improvement. If you cannot say what got better, the change is churn.

Severity uses the same scale as `make-interfaces-feel-better`: `HIGH` blocks use or hides information, `MEDIUM` is a noticeable usability or consistency problem, `LOW` is isolated polish.

## Exit condition

Close the loop only when all three hold:

1. Every check in the chosen tier passes, or its gap is written down as **Not verified** with the reason.
2. The last round produced no `HIGH` finding.
3. Before and after differ only where the task intended. An unexplained visual diff is a regression until proven otherwise.

Then write one paragraph into `.ody-code/frontend/audit.md`: tier, widths, states, checks run, result, verdict.

## Reporting

The report is the evidence, not a summary of your effort. Include:

- the frame paths you captured, grouped before / after;
- the console result for this round;
- the coverage actually walked (widths × states), naming anything skipped;
- findings as a table with **Severity**, **Location**, **Before**, **After**, **Why** — reuse `make-interfaces-feel-better`'s format rather than inventing a second one;
- a verdict: `Block` if any `HIGH` remains, `Needs changes` if only `MEDIUM` or `LOW`, `Approve` only when nothing actionable is left;
- an explicit **Not verified** list.

Never describe a screen as verified when you did not open its image at that width. "I checked the desktop view" is a claim about a specific file, not a general reassurance.

## When the loop cannot run

No dev server, no browser, or a build that will not start: say so in the first reply, not the last. Fall back to reading the code and state clearly that rendering, overflow, contrast, and focus are **Not verified**. Do not simulate a verification narrative around a page you never rendered — that is the failure this skill exists to prevent.
