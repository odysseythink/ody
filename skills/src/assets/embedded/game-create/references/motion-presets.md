# Motion Presets for Game Feel

Copy-paste starting values for the moments games animate most. General motion rules (frequency budgets, GPU-only properties, reduced-motion) live in the `motion-design` skill — these presets assume them. Never `scale(0)`; entrances start from `scale(0.92)` + `opacity: 0`.

## Presets

**Press feedback** (button/tap, 100+/session → keep tiny)
```css
.pressable:active { transform: scale(0.95); transition: transform 100ms cubic-bezier(0.23,1,0.32,1); }
```

**Spawn / entrance** (enemy appears, card dealt, power-up drops)
```css
@keyframes spawn { from { transform: scale(0.92); opacity: 0; } to { transform: scale(1); opacity: 1; } }
.spawn { animation: spawn 160ms cubic-bezier(0.23,1,0.32,1); }
```
Group spawns: stagger 30–50ms; never block input during the stagger.

**Hit impact** (projectile lands, collision, damage number)
```js
// scale pulse + brief pause reads as impact; add particles for juice
target.scale.set(0.9); // recover to 1 over ~120ms
// DOM: hitEl.animate([{transform:'scale(0.9)'},{transform:'scale(1)'}],{duration:120,easing:'cubic-bezier(0.23,1,0.32,1)'});
```
Interruptible: on repeat hits, retarget from the current scale (springs/transitions), don't restart keyframes.

**Pickup / collect** (coin, heal, combo up) — rare-to-occasional, allowed a little delight
```js
// overshoot is fine here: the gesture (grab) carried intent
animate(el, { scale: 1, y: 0 }, { type: 'spring', bounce: 0.25, duration: 0.4 });
// plus: number ticker toward the new score (tabular numbers, ~300ms)
```

**Throw / drag release** (angry-birds flick, swipe, drag-to-dismiss)
```js
// project where the gesture is going, then spring there carrying velocity
const projected = current + project(releaseVelocity); // project(): (v/1000)*d/(1-d), d≈0.998
animate(el, { x: nearestSnap(projected) }, { type: 'spring', bounce: 0.2, duration: 0.4, velocity: releaseVelocity });
```

**Game over / round end** (rare, high-emotion → allowed the longest, most deliberate motion)
```css
/* slow the world before the verdict; 400-600ms is fine here */
.board { transition: filter 400ms ease; filter: grayscale(0.8) brightness(0.7); }
```

**Damage flash / danger** — color over movement (keep under reduced-motion)
```css
@keyframes flash { 0%,100% { opacity: 1; } 50% { opacity: 0.4; } }
.damaged { animation: flash 200ms linear 2; } /* stepped repetition, not a slide */
```

## Feel-check loop

Tune by playing, not reading: serve locally → `browser-control` navigate + screenshot → simulate input → adjust one parameter at a time. Suspect motion? Slow it 2–5× (DevTools animation inspector) to see easing/origin defects, then check on a real device if gestures are involved.

---

Values distilled under MIT from [emilkowalski/skills](https://github.com/emilkowalski/skills) @ `d23d7f88` (via the `motion-design` skill's references) and adapted from game-feel practice.
