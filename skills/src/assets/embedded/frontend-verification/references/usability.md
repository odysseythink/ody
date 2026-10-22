# Usability and conversion

Adapted from design-taste `skills/taste-design-review` (MIT, © 2026 Mustafa) — see `skills/THIRD_PARTY.md`.

Companion to `SKILL.md` § Judging what the numbers cannot see. Two passes live here:

- **usability** — Nielsen's ten heuristics, applied, run on every tier that judges a screen;
- **conversion** — run only when the page asks the visitor for something.

Neither is check 8 or check 9. Checks 1–7 return a number and a pass condition; these return
findings and a judgement. Adding them to the numbered table would let a round write "8: pass" for
something no expression can settle — which is the exact failure `SKILL.md` spends its length
preventing. They appear in the report as findings with a source tag, not as rows of pass/fail.

## Why bother, given checks 1–7

A page can be pixel-correct and still fail the person using it. Checks 1–7 measure rendering:
nothing overflows, nothing clips, contrast holds, focus is visible, labels exist. None of them
notices that the submit button gives no feedback, that the delete action has no undo, or that the
error says "Error". Those are properties of the *interaction*, and the only instrument that reads
them is a person driving the page.

The one thing this pass is not allowed to become: a taste argument. "I would have used a
different layout" is not a finding. Every entry below has a stated failure condition, so a
finding can quote it and a reader can disagree with the condition rather than with the taste.

## The ten, applied

| # | Heuristic | Fails when | Needs a state |
| --- | --- | --- | --- |
| 1 | Visibility of system status | An action produces no visible change within ~1s — no pressed state, no spinner, no disabled control, no result. A control that looks inert is read as broken. | `loading`, `error` |
| 2 | Match with the real world | Copy uses the organisation's vocabulary, not the visitor's. "Provision an instance", "identity provider", "ledger entry" on a page whose reader says "account", "log in", "payment". | `default` |
| 3 | User control and freedom | A state has no exit; a destructive act has no undo or confirmation; a modal offers only "OK"; the browser back button undoes work instead of navigating. | `open` |
| 4 | Consistency and standards | The same action looks different in two places; one destination has two routes; a control type means two things. Tokens from `design-system.md`, nothing one-off "just this time". | `default` |
| 5 | Error prevention | The destructive is unconfirmed, the invalid is unconstrained (free-text country, unpasted card number), the tedious is un-autocompleted. | `error` |
| 6 | Recognition rather than recall | A name does not say what it opens; finishing the task requires carrying a value from a screen that is no longer visible; abbreviations are invented at the point of use. | `default` |
| 7 | Flexibility and efficiency | No shortcut for the repeat user (search, keyboard, saved view), or the shortcut crowds the first-visit path. Both failures are real; say which one you found. | `default` |
| 8 | Aesthetic and minimalist design | An element competes with the primary action instead of serving it. Three nested cards, a decorative chart above the value proposition, a full-width gradient behind the CTA. | `default` |
| 9 | Help users recognise, diagnose, recover | An error states neither what happened nor what to do; it names a code instead of a field; it appears somewhere other than beside the cause; it is only a colour. | `error` |
| 10 | Help and documentation | The answer to a likely question exists only in a help centre nobody opens, when the question is asked by the field itself. | `default` |

Row 4 is where `SKILL.md`'s old "Consistency" bullet went. It is one heuristic, not two ideas.

## Four of the ten need a state you may not have captured

The `Needs a state` column is the useful half of that table. A round that captured only
`default` frames has no evidence for heuristics 1, 3, 5 and 9 — and those four are exactly the
ones a screenshot of a happy page flatters.

So the rule:

> A heuristic whose required state was not captured is **Not verified**, not pass.

Write it that way in the report. "Usability 7/10 pass, 3 not verified (1, 5, 9 — no loading,
invalid or error frame)" is honest and tells the next round what to capture. "Usability 10/10"
from one default frame is the same failure as reporting a check you did not run, one level up in
abstraction — and much easier to believe, because it is prose.

This is why `SKILL.md`'s `full` tier asks for `default / empty / error` on every affected route:
the error frame is not decoration, it is the only evidence for heuristic 9.

## Conversion — only on a page that asks for something

Run this pass when the page has a conversion goal: landing, pricing, auth, signup, upgrade,
onboarding. Do **not** run it on a settings page, a dashboard, a documentation page, or an
internal tool. Those pages have no conversion, and inventing one produces findings nobody asked
for — the loudest way this pass goes wrong is demanding a CTA on a page whose whole job is to be
read.

Four things, in order of how often they are wrong:

1. **One primary action above the fold.** Everything else is visibly quieter or deferred below.
   Two buttons of equal weight is zero buttons: the visitor picks neither.
2. **The proposition lands from the headline alone.** Eight seconds, no scrolling, no reading the
   sub-sub-line. If the headline names the category and the sub-line names the mechanism, that is
   enough. If the headline is a slogan and the sub-line is a haiku, nothing landed.
3. **Friction inventory.** List every field, step, and click between "interested" and "started".
   Each one is either justified in the list or deleted. The inventory is the finding; the deletion
   is the fix.
4. **Trust where the decision happens.** Social proof, guarantee, and price transparency belong
   beside the CTA and inside the pricing table — where the visitor hesitates — not on a separate
   page they will never open.

### The one countable part

The friction inventory and the position of the first screen's controls can be measured. Do that
first, so the discussion is about the list rather than about memory:

```js
(() => {
  const fold = window.innerHeight;
  const visible = (el) => {
    const style = getComputedStyle(el);
    const rect = el.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0
      && style.visibility !== "hidden"
      && Number(style.opacity) > 0.05;
  };
  const controls = [...document.querySelectorAll('a[href],button,[role="button"],input[type="submit"]')]
    .filter(visible)
    .map((el) => {
      const rect = el.getBoundingClientRect();
      const style = getComputedStyle(el);
      return {
        label: (el.getAttribute("aria-label") || el.textContent || el.value || "").trim().slice(0, 40),
        tag: el.tagName.toLowerCase(),
        top: Math.round(rect.top),
        above_fold: rect.top < fold && rect.bottom > 0,
        w: Math.round(rect.width),
        h: Math.round(rect.height),
        background: style.backgroundColor,
        font_size: Math.round(Number.parseFloat(style.fontSize) || 0),
        full_width: rect.width > window.innerWidth * 0.6,
      };
    });
  const fields = [...document.querySelectorAll("form input:not([type=hidden]),form select,form textarea")]
    .filter((el) => visible(el) && el.type !== "hidden")
    .map((el) => el.getAttribute("name") || el.id || el.type);
  return {
    viewport_height: fold,
    controls_total: controls.length,
    controls_above_fold: controls.filter((item) => item.above_fold),
    form_field_count: fields.length,
    form_fields: fields.slice(0, 30),
    steps_declared: document.querySelectorAll("[data-step],[aria-current=\"step\"],.step").length,
  };
})()
```

What it settles: how many controls the first screen offers, where they sit, how many fields the
form demands, and whether the flow declares steps. What it does **not** settle: which control is
primary. Visual weight is a composite of size, fill, contrast, position, and isolation, and no
single computed value ranks them — `background` and `full_width` are hints, not a verdict.

So the expression hands you a list and the judgement reads it. Measured at 1280×900 on
design-taste's `examples/corporate-trust.html`, the first screen carries eight controls, and two of
them are the same action in the same colour:

```
Start free trial            a  128x35  rgb(31, 79, 224)   nav
Start a 14-day free trial   a  210x42  rgb(31, 79, 224)   hero
```

That is a finding you can defend without a taste argument: one action, two doors, both painted the
primary colour, both above the fold. Say it with the numbers beside it. Note also what the numbers
say that this pass may not claim — the 35px height is check 4's failure, not heuristic 4's, and it
is reported once. A number is the input to this pass, never its verdict.

## Fail the round if

- [ ] A heuristic was marked pass while its required state was never captured.
- [ ] A usability finding has no failure condition quoted from the table above.
- [ ] Conversion findings were raised on a page with no conversion goal.
- [ ] The friction inventory is missing — "the form felt long" is not an inventory, "six fields,
      four of them optional at signup" is.
- [ ] "I would have designed it differently" appears anywhere without a heuristic number and a
      failure condition behind it.

## Boundary with checks 1–7

Do not re-report a measured failure as a usability finding. If contrast failed check 5, that is a
check-5 finding; heuristic 8 may mention that a low-contrast decorative element competes with the
CTA, but it does not get to claim the contrast failure as its own. One defect, one finding, one
source tag — otherwise the report's counts inflate and the reader stops trusting the severities.
