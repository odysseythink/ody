---
name: motion-design
description: "Motion and interaction craft for UI that feels right — easing curves, durations, springs, gesture physics, and animation review. Use when building, tuning, or reviewing UI motion of any kind (games, web apps, tools): micro-interactions, transitions, hover/press feedback, drag/swipe gestures, staggered lists, loaders, or when the user says an interaction feels sluggish, janky, bouncy, or lifeless. Also for 'what is this animation called' naming questions (动效/动画/手感/交互反馈/缓动)."
metadata:
  short-description: Easing, springs, and gesture feel for UI motion
---

# Motion Design

Senior motion guidance for web UI. The model already knows how to animate; this skill supplies the values and judgment it doesn't carry around: exact curves, durations, spring configs, and the decision framework for when motion helps or hurts.

**Boundary:** visual identity (palette, typography, layout direction) belongs to `frontend-design`. This skill owns motion and interaction *feel*.

## The Decision Framework (answer in order)

**1. Should this animate at all?** Frequency decides:

| Frequency | Decision |
|---|---|
| 100+ times/day (keyboard shortcuts, command palette) | No animation. Ever. |
| Tens/day (hover, list navigation) | Remove or drastically reduce |
| Occasional (modals, drawers, toasts) | Standard animation |
| Rare / first-time (onboarding, celebrations) | Can add delight |

Never animate keyboard-initiated actions — they repeat hundreds of times daily and animation makes them feel slow. The strongest fix for a bad animation is often deleting it.

**2. Purpose.** Every animation answers "why does this animate?": spatial consistency, state indication, explanation, feedback, or preventing a jarring change. "It looks cool" on a frequently-seen element is not a purpose.

**3. Easing** (decision order):

- Entering / exiting → **`ease-out`** (starts fast, feels responsive)
- Moving / morphing on screen → **`ease-in-out`**
- Hover / color change → **`ease`**
- Constant motion (marquee, progress) → **`linear`**
- Default → **`ease-out`**

Never `ease-in` on UI — it delays the exact moment the user watches most. Built-in CSS easings are too weak for deliberate motion; use strong custom curves as tokens:

```css
--ease-out: cubic-bezier(0.23, 1, 0.32, 1);      /* strong ease-out for UI */
--ease-in-out: cubic-bezier(0.77, 0, 0.175, 1);  /* on-screen movement */
--ease-drawer: cubic-bezier(0.32, 0.72, 0, 1);   /* iOS-like drawer */
```

**4. Duration.** UI animations stay under 300ms:

| Element | Duration |
|---|---|
| Button press feedback | 100–160ms |
| Tooltips, small popovers | 125–200ms |
| Dropdowns, selects | 150–250ms |
| Modals, drawers | 200–500ms |
| Marketing / explanatory | Can be longer |

Perceived performance matters as much as real speed: a 180ms dropdown *feels* faster than a 400ms one; a fast-spinning spinner makes identical load time feel shorter. After the first tooltip, skip delay + animation entirely — instant tooltips make a toolbar feel fast.

## Defaults worth memorizing

- Entrances start from `scale(0.9–0.97)` + `opacity: 0`. **Never `scale(0)`** — nothing in the real world appears from nothing.
- Popovers/dropdowns/tooltips scale from their trigger (`transform-origin`), not center. **Modals are exempt** — centered origin is correct there.
- Press feedback: `transform: scale(0.97)` on `:active`, `transition: transform 100–160ms ease-out` (subtle: 0.95–0.98).
- Animate only `transform` and `opacity`. **`transition: all` is always a finding.**
- Rapidly-triggered or reversible motion (toasts stacking, toggles, drags) must be interruptible: CSS transitions or springs retarget from the current state; `@keyframes` restart from zero — avoid for dynamic UI.
- Group entrances stagger 30–80ms between items; stagger is decorative and must never block interaction.
- Springs for anything the user can touch. Default Apple-style config `{ type: "spring", duration: 0.5, bounce: 0 }`; add bounce (0.1–0.3) only when the gesture itself carried momentum (a flick, a throw).
- Respect `prefers-reduced-motion`: fewer and gentler animations, **not zero** — keep opacity/color feedback, drop movement. Gate `:hover` motion behind `@media (hover: hover) and (pointer: fine)`.

## When to load which reference

- **Naming or identifying an effect** ("the bouncy thing when a popover opens", "the iOS rubber-band scroll") → read `references/motion-vocabulary.md`. A reverse-lookup glossary: describe the feel, get the exact term to prompt with.
- **Implementing or reviewing animation code** → read `references/motion-audit.md`. The exact value catalog (curves, durations, spring configs), physicality rules, performance constraints, the ten review standards, and the flag-on-sight escalation list.
- **Building gesture-driven or Apple-style fluid UI** (drag, swipe, bottom sheets, momentum, interruptible motion) → read `references/interaction-principles.md`. Response latency, direct manipulation, interruptibility, velocity handoff, momentum projection, rubber-banding.

When citing a precise value in findings or plans, copy it from the reference — never approximate.

---

Sources: distilled under MIT from [emilkowalski/skills](https://github.com/emilkowalski/skills) @ `d23d7f88`. See `THIRD_PARTY.md` in this repository's skills crate.
