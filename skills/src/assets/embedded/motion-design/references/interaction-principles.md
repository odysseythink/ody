# Interaction Principles — Fluid, Gesture-Driven UI

How Apple builds interfaces that stop feeling like a computer (distilled from WWDC's *Designing Fluid Interfaces*, translated to the web: CSS, Pointer Events, `requestAnimationFrame`, spring libraries like Motion).

**The through-line:** an interface feels alive when motion starts from the current on-screen value, inherits the user's velocity, projects momentum forward, and can be grabbed and reversed at any instant. Springs are the tool that makes all of this natural — inherently interruptible and velocity-aware.

## 1. Response — kill latency

The moment lag appears, directness "falls off a cliff." Response is the foundation.

- **Respond on pointer-down, not on release.** Highlight a button the instant it's pressed; waiting for `click`/touch-up feels dead.
- **Audit every latency on the input path:** debounces, artificial timers, transition waits, the ~300ms tap delay. Anything non-essential is a regression.
- **Feedback is continuous *during* the interaction, not just at the end.** For a drag, slider, or drawer, update the UI 1:1 with the pointer the whole way through.

```css
.button:active { transform: scale(0.97); transition: transform 100ms ease-out; }
```

## 2. Direct manipulation — 1:1 tracking

> "Touch and content should move together."

The dragged element must stay glued to the finger — respecting the offset from *where they grabbed it*. Snapping to center on grab breaks the illusion immediately.

```js
el.addEventListener('pointerdown', (e) => {
  el.setPointerCapture(e.pointerId);
  const grabOffset = e.clientY - el.getBoundingClientRect().top;
  // track a short position+timestamp history — you need velocity at release
});
```

## 3. Interruptibility — the single most important principle

> "The thought and the gesture happen in parallel."

Every animation must be interruptible and redirectable at any moment: grab a moving element mid-flight and reverse it without waiting. A closing modal grabbed again should follow the finger — not finish closing, then reopen.

- **Never lock out input during a transition.**
- **Always animate from the *presentation* (current on-screen) value, never the target value.** On interrupt, read the element's live transform and start from there; starting from the logical value causes a visible jump.
- **Avoid CSS transitions and `@keyframes` for gesture-driven motion** — they can't be grabbed mid-flight. Springs animate from the current value by default.
- **Blend velocity on reversal — don't hard-cut it.** Replacing one animation with another at a reversal creates a velocity discontinuity, a "brick wall." Choose a spring library that carries velocity through re-targeting (iOS's *additive animations* do this natively).
- **Decompose 2D motion into independent X and Y springs** — a single spring on distance desyncs when X and Y have different velocities.

## 4. Behavior over animation — springs

> "Think of animation as a conversation between you and the object, not something prescribed by the interface."

A fixed-duration animation can't respond to new input; a spring can — new input just changes the target and motion stays continuous. Reach for springs for anything a user can touch.

Think in Apple's two designer-friendly parameters:

- **Damping ratio** — controls overshoot. `1.0` = critically damped (no bounce, smooth settle). `< 1.0` = overshoots and oscillates; lower = bouncier.
- **Response** — how quickly the spring reacts (seconds to reach the target).

**Defaults:** start UI at damping `1.0` (graceful, non-distracting). Add bounce (~`0.8`) **only when the gesture itself carried momentum** — overshoot on a menu that faded in feels wrong; overshoot on a flicked card feels right.

| Interaction (shipped by Apple) | Damping | Response |
|---|---|---|
| Move / reposition (e.g. PiP) | `1.0` | `0.4` |
| Rotation | `0.8` | `0.4` |
| Drawer / sheet | `0.8` | `0.3` |

**Web mapping (Motion / Framer Motion):** `bounce` + `duration` ≈ Apple's damping + response.

```js
animate(el, { y: 0 }, { type: 'spring', bounce: 0, duration: 0.4 });        // default: no overshoot
animate(el, { y: target }, { type: 'spring', bounce: 0.2, duration: 0.4 }); // momentum interaction
```

## 5. Velocity handoff — the seam between drag and animation

When a gesture ends, the animation must continue at the finger's *exact* velocity — the detail that most separates "fluid" from "fine."

Pass release velocity as the spring's initial velocity. Some APIs want **relative** velocity:

```
relativeVelocity = gestureVelocity / (targetValue − currentValue)
```

Example: element at `y=50`, target `y=150` (100px to go), finger moving 50px/s → initial spring velocity `50 / 100 = 0.5`. Framer Motion / Motion take absolute px/s directly (`velocity` option).

## 6. Momentum projection — animate to where the gesture is *going*

> "Take a small input and make a big output."

Don't snap to the nearest boundary from the release point — project the resting position from velocity (exactly like scroll deceleration), then snap to the target nearest the projection. This is what makes a flick feel like a throw. Apple's exact projection function:

```js
// decelerationRate ≈ 0.998 for normal scroll feel; 0.99 for snappier
function project(initialVelocity /* px/s */, decelerationRate = 0.998) {
  return (initialVelocity / 1000) * decelerationRate / (1 - decelerationRate);
}
const projected = currentPosition + project(releaseVelocity);
const target = nearestSnapPoint(projected);
animateSpringTo(target, { velocity: releaseVelocity });
```

Note: the physics-textbook `v²/(2·decel)` is *not* what Apple ships — use the exponential-decay form. Standard behavior in good bottom-sheets and carousels (Vaul, Embla).

## 7. Spatial consistency — symmetric paths, anchored origins

> "If something disappears one way, we expect it to emerge from where it came."

- **Enter and exit along the same path.** A panel that slides in from the right must dismiss to the right; in-from-right / out-the-bottom feels disconnected.
- **Anchor interactions to their source** — menus, popovers, and sheets originate from the trigger (`transform-origin`), keeping the spatial relationship obvious.
- **Mirror easing on reversible transitions** — inverse cubic-bézier control points for the two directions, so the outbound path matches the return.

## 8. Hint in the direction of the gesture

Humans predict a final state from a trajectory. Intermediate motion should telegraph where things are going — Control Center modules "grow up and out toward your finger." Make in-between frames point at the outcome.

## 9. Rubber-banding — soft boundaries

At an edge, resist progressively instead of stopping hard. A hard stop reads as "frozen"; continuous resistance reads as "responsive, but there's nothing more here":

```js
function rubberband(overshoot, dimension, constant = 0.55) {
  return (overshoot * dimension * constant) / (dimension + constant * Math.abs(overshoot));
}
```

## 10. Multimodal feedback — motion + sound + haptics

1. **Causality** — trigger feedback on the actual causal event (the toggle flipping, the item snapping home), matched to the action's physicality.
2. **Harmony** — visual, sound, and haptic fire on the **same frame**. Latency between them destroys the illusion; don't let a CSS transition lag the audio/haptic (Vibration API).
3. **Utility** — reserve haptics/sound for meaningful moments (success, error, commit, snap). Over-feedback trains users to ignore all of it.

## 11. Reduced motion & accessibility

Reduced motion means a gentler, non-vestibular equivalent, not no feedback. Respond to three independent signals:

- **`prefers-reduced-motion: reduce`** — replace slides/springs/parallax with short opacity cross-fades; drop elastic/overshoot; keep opacity/color changes that aid comprehension.
- **`prefers-reduced-transparency: reduce`** — raise background opacity, drop the blur.
- **`prefers-contrast: more`** — near-solid backgrounds with a defined, contrasting border.

```css
@media (prefers-reduced-motion: reduce) {
  .sheet { transition: opacity 200ms ease; transform: none !important; }
}
```

## Quick reference

| Need | Technique | Concrete value |
|---|---|---|
| Default UI spring | Critically damped, no overshoot | `damping 1.0`, `response 0.3–0.4` |
| Momentum / flick spring | Under-damped, slight bounce | `damping ~0.8`, `response 0.3–0.4` |
| Gesture → spring velocity | Hand off release velocity | `gestureVelocity / (target − current)` if normalized |
| Flick landing point | Project momentum | `current + (v/1000)·d/(1−d)`, `d ≈ 0.998` |
| Interrupt cleanly | Start from presentation (live) value | read the on-screen transform |
| Avoid reversal "brick wall" | Carry velocity through re-target | spring that blends velocity |
| Reversible transition | Mirror the easing curve | inverse cubic-bézier |
| Decide reverse vs. commit | Use velocity **sign**, not position | at release |
| 1:1 drag | Pointer Events + capture | respect the grab offset |
| Feedback | On pointer-down, continuous | never only at the end |
| Boundary | Rubber-band, don't hard-stop | progressive resistance |
| Reduced motion | Cross-fade, not slide/spring | `@media (prefers-reduced-motion)` |

---

Distilled under MIT from [apple-design](https://github.com/emilkowalski/skills/tree/main/skills/apple-design) by Emil Kowalski. Materials/translucency, typography, and the eight general design foundations sections were omitted (materials/type belong to `frontend-design`; foundations are out of motion scope).
