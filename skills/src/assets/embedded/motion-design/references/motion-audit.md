# Motion Audit — Exact Values, Rules, and Review Standards

The rule catalog for implementing and reviewing animation code. **Never approximate a value that appears here — copy it.** Severity: **HIGH** = feel-breaking (wrong easing on UI, animation on keyboard/high-frequency actions, dropped frames, `scale(0)`); **MEDIUM** = noticeably off (wrong origin, non-interruptible dynamic UI, missing reduced-motion); **LOW** = polish (stagger, blur-masked crossfades, token consolidation).

## 1. Frequency & purpose (should it animate?)

Valid purposes: spatial consistency, state indication, explanation, feedback, preventing jarring change. Frequency table:

| Frequency | Decision |
|---|---|
| 100+ times/day (keyboard shortcuts, command palette toggle) | No animation. Ever. |
| Tens of times/day (hover, list navigation) | Remove or drastically reduce |
| Occasional (modals, drawers, toasts) | Standard animation |
| Rare / first-time (onboarding, feedback, celebrations) | Can add delight |

**Never animate keyboard-initiated actions.** Raycast has no command-palette open/close animation — correct for something used hundreds of times a day. Don't re-litigate deliberate, documented motion tradeoffs.

## 2. Easing & duration (exact values)

Decision order: entering/exiting → `ease-out`; moving/morphing on screen → `ease-in-out`; hover/color → `ease`; constant motion → `linear`; default → `ease-out`. **`ease-in` on UI is always a finding.**

```css
--ease-out: cubic-bezier(0.23, 1, 0.32, 1);        /* strong ease-out for UI */
--ease-in-out: cubic-bezier(0.77, 0, 0.175, 1);    /* strong ease-in-out for on-screen movement */
--ease-drawer: cubic-bezier(0.32, 0.72, 0, 1);     /* iOS-like drawer curve (Ionic) */
```

Find stronger variants at [easing.dev](https://easing.dev/) or [easings.co](https://easings.co/) — don't hand-roll curves.

Duration budgets — **UI animations stay under 300ms**:

| Element | Duration |
|---|---|
| Button press feedback | 100–160ms |
| Tooltips, small popovers | 125–200ms |
| Dropdowns, selects | 150–250ms |
| Modals, drawers | 200–500ms |
| Marketing / explanatory | Can be longer |

Hunt for: `ease-in` anywhere, bare `ease`/`linear` on entrances, UI durations > 300ms, tooltip delay + animation on every hover in a toolbar (after the first, be instant).

## 3. Physicality & origin

- **Never `scale(0)`** — target `scale(0.9–0.97)` + `opacity: 0`.
- **Origin-aware popovers:** scale from the trigger, not center:
  ```css
  .popover { transform-origin: var(--transform-origin); } /* Base UI pattern */
  ```
  **Modals are exempt** — `transform-origin: center` is correct there. Do not report it.
- **Press feedback:** `transform: scale(0.97)` on `:active`, `transition: transform 160ms ease-out`, keep within 0.95–0.98.

Hunt for: `scale(0)`, pure-fade entrances with no initial transform, centered origin on trigger-anchored elements, pressable elements with no press feedback.

## 4. Interruptibility & timing

- CSS **transitions** retarget from the current state mid-animation; **keyframes** restart from zero. Toasts, toggles, drags, expand/collapse → transitions or springs.
- Entry without JS: `@starting-style` (legacy fallback: `data-mounted` attribute set in `useEffect`).
- Gesture-driven motion → springs (they carry velocity when interrupted).
- Apple-style spring config: `{ type: "spring", duration: 0.5, bounce: 0.2 }`; keep bounce subtle (0.1–0.3), reserve visible bounce for drag-to-dismiss and playful moments.
- **Asymmetric timing:** deliberate phases (press, hold, destructive confirm) animate slower; the system's response snaps. Symmetric timing on press-and-release is a finding.

Hunt for: `@keyframes` on toasts/toggles/rapidly-triggered UI, fixed-duration tweens on gestures, drags without velocity-based dismissal (see §gestures), hard stops at boundaries.

## 5. Performance (GPU-only)

- **Animate `transform` and `opacity` only.** `width`/`height`/`margin`/`padding`/`top`/`left` trigger layout + paint + composite.
- **`transition: all`** animates unintended properties off-GPU — always a finding.
- **Framer Motion `x`/`y`/`scale` shorthands are NOT hardware-accelerated** — main thread via rAF, drops frames under load. Use the full transform string: `animate={{ transform: "translateX(100px)" }}`.
- **Don't drive child transforms via a CSS variable on the parent** — recalcs styles for all children. Set `transform` directly on the element.
- CSS (and WAAPI) beat rAF-based JS under load: CSS for predetermined motion, JS/springs for dynamic and gesture-driven motion. WAAPI gives JS control with CSS performance.
- Keep transition-time `filter: blur()` under 20px — heavy blur is expensive, especially in Safari.

Hunt for: `transition: all`, animated layout properties, Framer shorthands on busy pages, `setProperty('--x', …)` driving children, rAF loops doing what CSS could.

## 6. Gestures & drag (exact values)

- **Momentum dismissal by velocity, not distance:** dismiss if `Math.abs(distance)/elapsedMs > ~0.11`. A flick should be enough.
- **Rubber-band at boundaries** — progressive resistance, not an invisible wall:
  ```js
  function rubberband(overshoot, dimension, constant = 0.55) {
    return (overshoot * dimension * constant) / (dimension + constant * Math.abs(overshoot));
  }
  ```
- **Pointer capture** once dragging starts (`setPointerCapture`), so tracking survives leaving bounds.
- **Multi-touch protection:** ignore extra touch points after drag begins (`if (isDragging) return`).
- Tap: highlight on touch-**down** (instant), commit on touch-up; ~10px hysteresis/hit padding; allow cancel-by-dragging-away.
- Drag: ~10px movement threshold before committing to a direction, then track 1:1; respect the grab offset (don't snap to center).
- Detect plausible gestures in parallel from the first move; avoid recognizers that only report a final state — they throw away continuous tracking. Minimize disambiguation delays.

## 7. Transforms & clip-path techniques

- **`translate` percentages** are relative to the element's own size — `translateY(100%)` = its own height (how Sonner/Vaul position toasts/drawers). Prefer over hardcoded px.
- **`scale()` scales children too** (font, icons) — a feature for press feedback.
- **3D:** `rotateX/Y` + `transform-style: preserve-3d` for depth/orbit/flip without JS. Lower `perspective` value = exaggerated depth.
- **`clip-path: inset(t r b l)`** eats in from each side: reveal-on-scroll (`inset(0 0 100% 0)` → `inset(0 0 0 0)`), hold-to-delete overlay, seamless tab color transitions (duplicate + clip the active copy), comparison sliders.

## 8. Cohesion, stagger, crossfades

- Motion matches product personality — playful can be bouncier, a dashboard stays crisp. Mismatched personality is a finding.
- Curves/durations live as shared tokens; five near-identical hand-typed cubic-beziers is a consolidation finding. Plans must extend repo conventions, not invent parallel ones.
- Everything-at-once group entrances → **30–80ms stagger**. Stagger is decorative — never block interaction.
- A jarring crossfade that double-exposes two states can be masked with `filter: blur(2px)` during the transition.

## 9. Accessibility

```css
@media (prefers-reduced-motion: reduce) {
  .element { animation: fade 0.2s ease; } /* keep opacity/color, drop movement */
}
@media (hover: hover) and (pointer: fine) {
  .element:hover { transform: scale(1.05); } /* touch fires false hovers on tap */
}
```

Reduced motion means fewer and gentler animations, **not zero** — keep transitions that aid comprehension. In JS branch with `useReducedMotion()`. Also avoid full-viewport moving backgrounds, slow looping oscillations (~0.2 Hz), and abrupt brightness jumps; ease dark↔light theme changes.

Hunt for: movement with no reduced-motion handling, ungated `:hover` motion, reduced-motion implementations that nuke all feedback.

## 10. The ten non-negotiables (review bar)

1. Justified motion (answers "why") 2. Frequency-appropriate 3. Responsive easing (no `ease-in` on UI, no weak built-ins on deliberate motion) 4. Sub-300ms UI 5. Origin & physical correctness (no `scale(0)`, trigger-anchored origin) 6. Interruptibility where triggered rapidly 7. GPU-only properties 8. Accessibility (reduced-motion, hover gating) 9. Asymmetric enter/exit for deliberate actions 10. Cohesion with product personality.

## 11. Flag on sight (hard escalation)

`transition: all` · `scale(0)` or pure-fade entrances · `ease-in` on any UI interaction · animation on keyboard shortcuts / command palette / 100+-per-day actions · UI duration > 300ms unstated · centered origin on trigger-anchored popover · keyframes on toasts/toggles · animating layout properties · Framer shorthand props on busy-page motion · parent CSS variable driving child transforms · missing `prefers-reduced-motion` on movement · ungated `:hover` motion · symmetric timing on press-and-release · group entrance with no stagger where one belongs.

## 12. Remedial preference hierarchy

Prefer earlier moves: 1. **Delete the animation** (high-frequency / no purpose) 2. Reduce it (shorter, smaller, fewer properties) 3. Fix the easing (`ease-in` → strong custom curve) 4. Fix origin/physicality 5. Make it interruptible (keyframes → transitions/springs) 6. Move it to the GPU 7. Asymmetric timing 8. Polish (blur-mask, stagger, `@starting-style`, springs for "alive" elements) 9. Accessibility & cohesion.

## 13. Review output format

**Part 1 — Findings table** (required): `| Before | After | Why |` with exact values from this file. **Part 2 — Verdict** grouped by tier (feel-breaking regressions → missed simplifications → performance → interruptibility & timing → origin/physicality/cohesion → accessibility), closing with an explicit **Block** or **Approve** decision. Cite `file:line`; when feel can't be judged from code alone, prescribe a feel-check (slow motion 2–5×, frame-by-frame in DevTools, real device for gestures, fresh eyes next day) instead of guessing.

---

Distilled under MIT from [improve-animations](https://github.com/emilkowalski/skills/tree/main/skills/improve-animations) (AUDIT.md), [review-animations](https://github.com/emilkowalski/skills/tree/main/skills/review-animations) (STANDARDS.md), and [emil-design-eng](https://github.com/emilkowalski/skills/tree/main/skills/emil-design-eng) by Emil Kowalski; overlapping content deduplicated.
