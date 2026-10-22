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
2. **Read the contract.** Open `.ody-code/frontend/design-system.md` if it exists and take tokens from it. Re-deriving the palette from component files invents a second source of truth; by the third task they disagree. If it does not exist and the task establishes a direction, write it now (see "The durable contract").
3. **Look at the baseline.** Read the images. Name what is wrong with the current screen in one sentence per problem. If you cannot name a problem, you are not ready to change it.
4. **Build toward the stated direction.** Only the part under change; do not restyle adjacent surfaces to match your new idea.
5. **Re-capture the same frames.** Same route, same widths, same states, same file naming. Different camera, no comparison.
6. **Judge against the checks below, then fix and re-capture.** One hypothesis per iteration. A fix that is not re-captured is unverified.
7. **Stop when the exit condition holds** (below), then report with evidence.

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

## The durable contract

`design-system.md` is the one artifact that outlives a task. Everything else here is per-round.

- **Write it on the first task that fixes a direction.** Before there is a screen, the tables would record intent rather than fact.
- **Read it before editing components, amend it in place.** A value in the file and a value in the code that disagree is a bug in one of them — pick which and fix it. Two sources of truth is the failure this file prevents.
- **Tokens, not implementations.** `--space-3: 12px`, not the padding of one card.
- **The bar:** a new teammate reads only this file and builds a screen that belongs in the app.

When the direction has to come from somewhere other than your own taste — "make it feel like X" — extract a scale from a public reference page with `references/tokens.md`. That file also has the non-negotiables: public pages only, visual properties only, two or three sources at most, and every extraction recorded with its URL, date, and what you deliberately did not take.

## Coverage tiers

Pick the tier from the size of the change, and say which one you ran. This is what keeps a one-button change from turning into a full design review.

| Tier | When | Frames | Checks |
| --- | --- | --- | --- |
| `quick` | one component, copy, spacing, a single state | 1 primary path × 2 widths (mobile + desktop) × the state you touched, ≤ 6 frames | console clean, overflow, clipping, hit area, accessible names |
| `full` | a new screen, a redesign, layout or type-scale change, a flow with several steps | every affected route × 5 widths (360 / 768 / 1024 / 1280 / 1440) × 3 states (default / empty / error, or default / hover / focus for interactive parts) | all seven checks, plus the craft pass |

If the project already has breakpoints, use the project's widths instead of the default five and say so.

## Capture

Use the browser tools — not a mental model of the page.

```
browser__navigate        { url, wait_until: "network_idle" }
browser__set_viewport    { width: 1280, height: 900 }        # repeat per width
browser__screenshot      { path: ".ody-code/frontend/shots/home@1280@before.png" }
view_image               { path: ".ody-code/frontend/shots/home@1280@before.png" }
browser__read_logs       { kind: "console", level: "error", drain: true }   # marking the start
```

The `drain: true` read is the round's zero point: it reports what the page had
already logged, then removes those entries so the read after your actions shows
only what your actions produced. Without it the console check can only be
inferred by comparing counts, and an inferred check is the one that gets
overstated in the report.

`browser__screenshot` with `path` writes the PNG into the workspace; the call is only useful once `view_image` reads it back. A screenshot you did not open is not evidence. The `path` form never truncates — the base64 form does, and a truncated PNG cannot be decoded at all.

`browser__set_viewport` changes layout for real (it is not a resize of the screenshot). Clear it with `{ reset: true }` when the capture pass ends so later work is not stuck at 360px.

For a state that only appears on interaction, drive it: `browser__click` at a point, `browser__type` into a selector, then capture. For key events there is no first-class tool — `browser__execute_raw_cdp` with `Input.dispatchKeyEvent` works in local browser mode only, and is refused for an external browser. If you cannot reach the state, capture the closest reachable one and label the rest **Not verified**.

## The seven checks

Run these on every frame in scope. They are cheap, objective, and they catch the failures that survive a visual skim.

| # | Check | Pass condition | How |
| --- | --- | --- | --- |
| 1 | Console | zero `error` entries produced by the actions of this round | `browser__read_logs { kind: "console", level: "error" }` after a `drain: true` baseline |
| 2 | Overflow | no page-level horizontal scroll, no element past the right edge, and `visualViewport.scale` = 1 | `references/checks.md` § Overflow |
| 3 | Clipping | no visible text cut off inside a fixed-height or hidden-overflow box | `references/checks.md` § Clipping |
| 4 | Hit area | interactive targets ≥ 40×40 CSS px (≥ 44 on touch-first layouts) | `references/checks.md` § Hit area |
| 5 | Contrast | body text ≥ 4.5:1, large text and UI boundaries ≥ 3:1 | `references/checks.md` § Contrast |
| 6 | Focus | keyboard focus is visible and reachable in a sensible order | `references/checks.md` § Focus |
| 7 | Accessibility | no missing label, no unnamed control, heading levels intact, targets ≥ 24×24, motion respects the setting | `references/accessibility.md` |

A console `error` is a failed check even when the frame looks perfect: a component that threw during hydration will break on the next interaction.

Check 7 is the one that cannot be guessed from a picture. A `<div role="button">`, a placeholder standing in for a label, and three skipped heading levels all render beautifully — checks 1–6 pass, and the page is unusable with a keyboard and silent to a screen reader. `references/accessibility.md` carries four `browser__evaluate` expressions for the measurable half and separates out the four sentences that require actually pressing Tab or running VoiceOver. Those go in **Not verified** if you did not do them; they do not get written as "keyboard accessible" on the strength of an expression.

Check 2 has a blind spot worth naming, because it hides the most common mobile defect: when content is wider than a phone viewport, the browser may widen the *layout* viewport and scale the page to fit instead of overflowing. Nothing crosses the right edge, `offenders` is empty, and the page is silently rendered at 89% — real type becomes smaller and spacings compress, with no scrollbar to show for it. `visualViewport.scale` below 1, or an `innerWidth` larger than the width you passed to `set_viewport`, is that state. A fixed-width child is the usual cause.

`references/checks.md` holds the exact `browser__evaluate` expressions. Use them instead of re-deriving the arithmetic — hand-computed contrast and off-by-one overflow judgements are how false passes get written. § One report returns checks 2–6 plus the craft metrics in a single call; when a count in it is non-zero, drop to the matching section for the element names.

## Judging what the numbers cannot see

The seven checks are the floor. Above it sit three passes. The craft skills already own their vocabulary — use theirs, do not re-invent it.

### The craft pass — what a frame can show

- **Hierarchy** — one primary action; what matters reads first within two seconds (`frontend.md` quality bar).
- **Detail** — one spacing scale, all states present, composition holds between the captured widths.
- **Motion** — a purpose per animation, timings from `motion-design`'s budget table.
- **Comparison** — with before and after side by side, name what improved in one line per improvement. If you cannot say what got better, the change is churn.

Where a claim here can be turned into a number, turn it into one with `references/checks.md` § Craft metrics — headline lines, measure in `ch`, distinct colours, bitmap count, the text inset inside a card, variant counts. "The headline is 3 lines and 21 words" is arguable; "hierarchy is weak" is not. A number is the input to a finding, never the finding itself, and the six numbers are only meaningful per page kind: a login page with `bitmap_count: 0` is correct, and judging it against a landing page's imagery is the mistake this rule exists to prevent.

Consistency is not here. It is heuristic 4 of the next pass, with the other nine, because "the same action looks the same everywhere" is a usability claim rather than a visual one.

### The usability pass — Nielsen's ten, applied

Ten questions, one line each. `references/usability.md` carries the failure condition for each, and the state that has to be on screen before you may answer it.

1. **System status** — every action answers within a second: pressed, loading, done, failed. A control that looks inert reads as broken.
2. **The user's language** — copy in the audience's words, not the org chart's. "Provision an instance" on a consumer page is a fail.
3. **Control and freedom** — an exit from every state, an undo for every destructive act. A modal whose only button is "OK" is a trap.
4. **Consistency** — one action, one appearance; one destination, one route; tokens from `design-system.md`, nothing one-off.
5. **Error prevention** — confirm the destructive, constrain the invalid, autocomplete the tedious.
6. **Recognition over recall** — names say what they open; nothing requires carrying a value from a screen that is gone.
7. **Flexibility** — a shortcut for the tenth visit that does not crowd the first.
8. **Minimalism** — every element earns its place against the primary action. Decoration that competes is noise.
9. **Error recovery** — what happened, why, and what to do next; never a bare "Error", never colour alone.
10. **Help where it is needed** — beside the field that confuses, not in a help centre nobody opens.

Four of these are judged from a state most rounds never capture: #1 needs a loading or failed frame, #3 an open overlay, #5 an invalid submission, #9 an actual error. **A heuristic whose required state was not captured is Not verified, not pass.** "Usability 7/10, three not verified — no error frame" is honest and tells the next round what to capture; "10/10" from one default frame is the same failure as claiming a check you never ran, one level up and much easier to believe, because it is prose.

### The conversion pass — only on a page that asks for something

Landing, pricing, auth, signup, upgrade. Not settings, dashboards, documentation or internal tools: those have no conversion, and demanding a CTA on a page whose job is to be read is how this pass gets switched off entirely. Four things:

- **One primary action above the fold.** Two buttons of equal weight are zero buttons — the visitor picks neither.
- **The proposition lands from the headline alone**, in eight seconds, without the sub-sub-line.
- **Friction inventory** — every field, step and click between "interested" and "started", each one justified in the list or deleted.
- **Trust where the decision happens** — beside the CTA, inside the pricing table, not on a page they will never open.

One of these is countable: the first screen's controls, their positions, and the form's field count. `references/usability.md` has the expression. It settles that there are two equal CTAs above the fold; it cannot settle which one should go. Counting hands you the list, the judgement reads it. A number is the input to this pass, never its verdict.

### Anchor the adjective

A sentence about how a frame looks is a claim like any other, and the eye is the
least reliable instrument in the loop. Two readings of the same picture send the
next iteration to opposite files: *"the page is scaled down"* means the type
scale and spacing are wrong, *"the card is wider than the viewport"* means a
`width` is wrong. Both are sayable about one screenshot, and only one is true.

So ask the page before describing it — `references/checks.md` § Frame facts
returns `visual_viewport.scale`, `root_zoom`, `scaled_elements`,
`sample.font_size`, and `document_scroll.w` against `viewport.w`. The rule is one
line: **every qualitative sentence in the report carries a frame-fact number or
a check result beside it.** "The card is 420 wide inside a 375 viewport, so the
browser scaled the page to 0.893" is evidence. "It looks shrunk" is a guess with
a confident tone, and it costs an iteration to disprove.

The failure this catches is not cosmetic, and it cuts both ways. One round
described a mobile frame as "the whole page is scaled down, the text is too
small"; the review of that round answered "no, the card is clipped at the right
edge and the type never moved". Both sentences were about the same screenshot
and neither was checked. The geometry settled it: `visual_viewport.scale 0.893`,
`innerWidth == scrollWidth == 420`, `offenders: []` — the layout viewport had
grown past the 375px device, the browser had scaled the page to fit, and nothing
was cut off. The first sentence was right. Two confident readings, one number.

The general shape: a sentence about appearance is cheap to write and expensive
to retract, because the next change is built on it. Spend one `browser__evaluate`
instead.

Severity uses the same scale as `make-interfaces-feel-better`: `HIGH` blocks use or hides information, `MEDIUM` is a noticeable usability or consistency problem, `LOW` is isolated polish.

## Exit condition

Close the loop only when all four hold:

1. Every check in the chosen tier passes, or its gap is written down as **Not verified** with the reason.
2. The last round produced no `HIGH` finding.
3. Before and after differ only where the task intended. An unexplained visual diff is a regression until proven otherwise.
4. Every state sentence in the report is backed by the close-out command (below).

Then write one paragraph into `.ody-code/frontend/audit.md`: tier, widths, states, checks run, result, verdict.

## Reporting

The report is the evidence, not a summary of your effort. Include:

- the frame paths you captured, grouped before / after;
- all seven checks by number — `pass`, the failing count, or **not run** — because a
  check you forgot reads as a check you passed, and only one of those is
  survivable;
- the usability pass by count, naming any heuristic you could not answer and the
  state you would have needed — a pass count without that list claims coverage
  the frames do not have;
- the craft numbers you measured — `headline_lines`, `text_measure_ch`,
  `colour_distinct`, `bitmap_count`, `edge_inset_min`, and the variant counts —
  or `not run`. A craft pass reported as a verdict with no numbers behind it is
  the same claim as a check you skipped, one sentence softer;
- the coverage actually walked (widths × states), naming anything skipped;
- findings as a table with **Severity**, **Location**, **Before**, **After**, **Why** — reuse `make-interfaces-feel-better`'s format rather than inventing a second one. **Why** opens with its source (`check 5`, `Nielsen 9`, `conversion`, `craft`) so a reader can tell a measured defect from a judged one;
- a verdict: `Block` if any `HIGH` remains, `Needs changes` if only `MEDIUM` or `LOW`, `Approve` only when nothing actionable is left;
- an explicit **Not verified** list.

Every dimension, ratio, and count in that report must be traceable to a tool
result from this round. A number you cannot point at is worse than no number: it
reads as measured and is not. If a check returned only pass/fail, say "no targets
under 40×40" instead of inventing the size that passed it. The same rule applies
to state: do not report a file as staged, saved, or committed unless a command
in this round said so — § Close-out is that command.

Never describe a screen as verified when you did not open its image at that width. "I checked the desktop view" is a claim about a specific file, not a general reassurance.

## Close-out

The last lines of a round are where an unverified claim does the most damage:
nobody re-opens the work to disprove a sentence that reads like a status line.
So the round ends with one command, run once, before the final message:

```
git status --porcelain=v1; git log --oneline -1
```

Read the columns. A letter in the **first** column (`M `, `A `, `R `) means
staged. A letter in the second (` M`) means changed but not staged. `??` means
untracked. A path printed nowhere is unchanged — or ignored, and an ignored path
can never be staged, so that is not a maybe.

| About to write | It is true only when |
| --- | --- |
| "staged" / "已暂存" | the path shows a first-column letter in `git status` |
| "modified" / "改好了" | any `git status` line names the path |
| "committed" | `git log --oneline -1` names the commit you mean |
| "saved" / "written" for a frame or `audit.md` | `ls -l <path>` in this round, with an mtime from this round |
| "clean" / "no changes" | `git status --porcelain` printed nothing |
| a check passed | that check's own tool result, containing the pass condition |

If the output does not support the sentence, delete the sentence. Do not soften
it to "should be staged" or "I believe it saved" — a hedge still implies you
looked, and it is the hedge that survives into notes someone else acts on.

This is the same rule as the numbers, applied to state instead of geometry: both
are claims that a command in this round can settle, and neither is settled by
being plausible.

## When the loop cannot run

No dev server, no browser, or a build that will not start: say so in the first reply, not the last. Fall back to reading the code and state clearly that rendering, overflow, contrast, and focus are **Not verified**. Do not simulate a verification narrative around a page you never rendered — that is the failure this skill exists to prevent.

Check 7 is the partial exception, and the only one. Label association, heading levels, `div[onclick]`, `aria-hidden` on focusable content, and target sizes declared in the stylesheet are all readable in the source — run that half and say you read it rather than measured it. The tab walk, the screen-reader pass, and alt-text quality stay **Not verified**: they are properties of the rendered page and of a reader, and neither exists yet.
