# Checks

Copy-paste `browser__evaluate` expressions for the six checks in `SKILL.md`, plus the capture recipes that keep a round comparable.

Each expression is an IIFE that returns JSON. The browser tool refuses a few patterns by policy — `document.cookie`, web storage, `eval(`, `setTimeout(`, and classic `function(` declarations — so these use arrow functions and read layout only. Keep that style when you adapt them.

## Overflow

```js
(() => {
  const vw = window.innerWidth;
  const offenders = [...document.querySelectorAll("body *")]
    .map((el) => ({ el, r: el.getBoundingClientRect() }))
    .filter(({ r }) => r.width > 0 && r.right > vw + 1)
    .slice(0, 10)
    .map(({ el, r }) => ({
      tag: el.tagName.toLowerCase(),
      cls: String(el.className).slice(0, 60),
      right: Math.round(r.right),
    }));
  return {
    viewport: vw,
    document_scroll_width: document.documentElement.scrollWidth,
    horizontal_scroll: document.documentElement.scrollWidth > vw + 1,
    offenders,
  };
})()
```

`offenders` usually points at the cause directly. If it is empty but `horizontal_scroll` is true, the culprit is a fixed-position or absolutely positioned element clipped by an ancestor — search `position: fixed` / `100vw` next.

## Clipping

```js
(() => [...document.querySelectorAll("h1,h2,h3,h4,p,span,a,li,td,th,label,button")]
  .filter((el) => {
    if (el.children.length > 0) return false;
    const style = getComputedStyle(el);
    const constrained = style.overflow !== "visible" || style.textOverflow === "ellipsis";
    return constrained
      && (el.scrollWidth > el.clientWidth + 1 || el.scrollHeight > el.clientHeight + 1);
  })
  .slice(0, 10)
  .map((el) => ({
    tag: el.tagName.toLowerCase(),
    text: (el.textContent || "").trim().slice(0, 40),
    scroll_width: el.scrollWidth,
    client_width: el.clientWidth,
    scroll_height: el.scrollHeight,
    client_height: el.clientHeight,
  })))()
```

An ellipsis is legitimate for a table cell or a badge, and a defect for a button label or an error message. Decide by role, not by the number.

## Hit area

```js
(() => {
  const targets = [...document.querySelectorAll('a[href],button,[role="button"],input,select,textarea,summary')]
    .map((el) => {
      const r = el.getBoundingClientRect();
      return {
        tag: el.tagName.toLowerCase(),
        label: (el.getAttribute("aria-label") || el.textContent || "").trim().slice(0, 40),
        w: Math.round(r.width),
        h: Math.round(r.height),
      };
    })
    .filter((item) => item.w > 0 && item.h > 0);
  const violations = targets.filter((item) => item.w < 40 || item.h < 40);
  return { total: targets.length, violations, targets: targets.slice(0, 40) };
})()
```

The pass condition is `violations.length === 0`. `targets` is there so a passing
round still has a number to quote: "the primary button is 279×44 at 375px" is
evidence, "the check returned `[]`" is only the absence of evidence — and an
absent number is what invites a made-up one in the report. Read the size of the
element that actually receives the pointer, not the one that looks like a button:
a padded icon button is fine visually and tight in hit area only if the padding
sits on a child.

## Contrast

```js
(() => {
  const channels = (value) => {
    const parts = (value.match(/[\d.]+/g) || []).slice(0, 3).map(Number);
    return parts.length === 3 ? parts : null;
  };
  const alphaOf = (value) => {
    const parts = value.match(/[\d.]+/g) || [];
    return parts.length > 3 ? Number(parts[3]) : 1;
  };
  const luminance = (rgb) => {
    const linear = rgb.map((channel) => {
      const c = channel / 255;
      return c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
    });
    return 0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2];
  };
  const ratio = (a, b) => {
    const la = luminance(a);
    const lb = luminance(b);
    return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
  };
  const backgroundOf = (el) => {
    let node = el;
    while (node) {
      const value = getComputedStyle(node).backgroundColor;
      const rgb = channels(value);
      if (rgb && alphaOf(value) > 0.5) return rgb;
      node = node.parentElement;
    }
    return [255, 255, 255];
  };
  return [...document.querySelectorAll("h1,h2,h3,h4,h5,p,span,a,li,td,th,label,button,dt,dd,figcaption")]
    .filter((el) => el.children.length === 0 && (el.textContent || "").trim().length > 1)
    .map((el) => {
      const style = getComputedStyle(el);
      if (style.visibility === "hidden" || style.display === "none" || Number(style.opacity) === 0) return null;
      const size = parseFloat(style.fontSize);
      const bold = Number(style.fontWeight) >= 700;
      const large = size >= 24 || (size >= 18.66 && bold);
      const fg = channels(style.color);
      if (!fg) return null;
      return {
        text: (el.textContent || "").trim().slice(0, 30),
        size: Math.round(size),
        ratio: Number(ratio(fg, backgroundOf(el)).toFixed(2)),
        required: large ? 3 : 4.5,
      };
    })
    .filter((item) => item && item.ratio < item.required)
    .slice(0, 15);
})()
```

Gradients and background images resolve to the nearest solid ancestor colour, so treat a hit over a gradient or photo as "check by eye" rather than as proof. Placeholder text is not body text but still needs to be legible: 4.5:1 is the safe target.

## Focus

```js
(() => {
  const focusable = [...document.querySelectorAll('a[href],button,input,select,textarea,[tabindex]:not([tabindex="-1"])')]
    .filter((el) => !el.disabled && el.getBoundingClientRect().width > 0);
  const samples = focusable.slice(0, 6).map((el) => {
    el.focus();
    const style = getComputedStyle(el);
    const hasRing = style.outlineStyle !== "none"
      && parseFloat(style.outlineWidth) > 0
      && style.outlineColor !== "rgba(0, 0, 0, 0)";
    return {
      tag: el.tagName.toLowerCase(),
      label: (el.getAttribute("aria-label") || el.textContent || "").trim().slice(0, 30),
      outline: hasRing ? `${style.outlineWidth} ${style.outlineStyle} ${style.outlineColor}` : "none",
      box_shadow: style.boxShadow === "none" ? null : "set",
    };
  });
  return { focusable_count: focusable.length, samples };
})()
```

Two limits, state them when you report:

- Programmatic `focus()` does not always match `:focus-visible`, so a ring that shows here can still be absent after a mouse click and present after Tab. The check catches *missing* rings reliably; it cannot prove the ring only appears for keyboard users.
- Tab *order* needs real key events. `browser__execute_raw_cdp` with `Input.dispatchKeyEvent` can send them in local browser mode, but the call is refused for an external browser. If neither path is available, label order **Not verified** instead of guessing.

## Capture recipes

### Breakpoint matrix

```
for width in 360 768 1024 1280 1440:
    browser__set_viewport  { width, height: 900, mobile: width <= 768 }
    browser__screenshot    { path: ".ody-code/frontend/shots/<route>@<width>@<round>.png" }
    view_image             { path: "..." }
    browser__evaluate      { expression: "<overflow check>" }
```

Set `mobile: true` only when the project actually ships a mobile layout; it also switches meta-viewport and touch emulation, which can change what you are testing.

Clear the override with `{ reset: true }` before the round ends. A leftover 360px viewport silently ruins the next screenshot in the session.

### Interaction states

```
browser__click  { x, y }                 # open the menu / hover equivalent
browser__screenshot { path: "...@<width>@open.png" }
```

`browser__click` takes coordinates, not a selector — read them from the overflow/clipping output or from `browser__get_dom` first. A click that lands on the wrong element changes the state you think you captured, so verify with `browser__evaluate` (`document.activeElement`, or an `aria-expanded` attribute) before screenshotting.

### Console

The buffer is cumulative for the page's lifetime, so a plain read mixes this
round's errors with everything that happened before it. Mark the start of the
round with a draining read, then read again after the actions:

```
browser__read_logs { kind: "console", level: "error", drain: true }   # baseline: read it, then forget it
# ... the actions under test ...
browser__read_logs { kind: "console", level: "error" }                # only entries from this round
```

The second call is the check: it must be empty. An `error` from a resource that
404s is still a defect in the screen you are about to call finished.

Without `drain` the only honest move is to record the starting count and compare,
which turns a measurement into an inference — and an inference is what gets
mis-stated in the report. Take the baseline, then let the buffer forget it.

## Fast triage

When a finding appears, the cause is usually structural rather than cosmetic:

| Symptom | Look at first |
| --- | --- |
| Horizontal scroll at one width only | a fixed width or `min-width` on a child, or `100vw` plus a scrollbar |
| Text clipped after a copy change | fixed `height`, `max-height`, or `line-clamp` without a fallback |
| Contrast fails only in dark mode | a hard-coded hex instead of a token, or an opacity applied to text |
| Focus ring missing on one control | a component that resets `outline` without a replacement ring |
| Layout shifts between widths | a grid breakpoint that changes the column count without re-deriving spacing |
| Looks flat compared to before | the direction was not carried through — revisit `frontend-design`'s token plan before adding effects |

Two rounds of the same finding means the diagnosis is wrong. Re-read the frame and the computed value instead of adjusting the value again.
