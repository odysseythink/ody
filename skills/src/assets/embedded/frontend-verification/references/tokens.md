# Visual contract and reference extraction

Two things live here: how to write `.ody-code/frontend/design-system.md` once so later tasks read it instead of re-deriving it, and the `browser__evaluate` expression that harvests a token palette from a reference site.

## Why the file exists

A token set inferred from component code is a guess about intent. The same `14px` appears because the scale says so, or because someone nudged it in a review — the code cannot tell you which. After two tasks those guesses drift, and the third screen uses a fourth shade of grey.

`design-system.md` is the place the decision is recorded once. The test for whether it is written well: **a new teammate reads only this file and can build a screen that belongs in the app.** If they would still need to open three components to pick a colour, the file is a list of observations, not a contract.

## What goes in it

```md
# Design system

## Direction
One paragraph: who uses this, what it should feel like, the one decision
that carries that feeling. Written once; changed only when the direction
changes.

## Type
| Role | Size | Line height | Weight | Tracking | Family |
| --- | --- | --- | --- | --- | --- |
| display | 40px | 1.05 | 600 | -0.02em | Inter |
| body | 15px | 1.6 | 400 | 0 | Inter |

## Colour
| Token | Value | Use |
| --- | --- | --- |
| --surface | #0b0b0c | page background |
| --text | #ededef | primary copy |

## Space
Base unit and the multiplier ladder actually in use (e.g. 4 → 4/8/12/20/32/48).
Note the exceptions by name; an unnamed exception becomes a habit.

## Radius, border, elevation
The finite set, with the one-line rule for choosing between them.

## Motion
Durations and easings from `motion-design`'s budget table, as named tokens.

## Reference sources
One entry per site consulted: URL, date, what was taken, what was not.
```

Keep it short enough to read in one sitting. A forty-row colour table nobody consults is worse than none.

## Writing rules

1. **Write it on the first task that establishes direction**, not at project start — before there is a screen, the table would be filled with intent rather than fact.
2. **Read it before touching component code.** If a value in the file and the value in the code disagree, the code is the bug or the file is stale; decide which, then fix one. Do not silently follow the code.
3. **Amend incrementally.** A new surface gets a new row, not a new file. If a change makes a row wrong, rewrite that row — leaving both is how the file stops being trusted.
4. **Record tokens, not implementations.** `--space-3: 12px`, not `.card__body { padding: 12px 16px 14px }`. The second is a component detail that will be refactored; the first is what the next component needs.

## Extracting from a reference site

Use this when the task says "make it feel like X", or when the project has no tokens and you need a starting point that is not your own taste.

Scope limits, non-negotiable:

- **Public pages only.** Never a logged-in view, an admin panel, or anything behind a paywall.
- **Visual properties only.** No copy, no logos, no images, no brand names. You are reading a scale, not a design.
- **Two or three sites maximum.** Past that you are averaging, and an average has no direction.

```js
(() => {
  const top = (map, n) => [...map.entries()]
    .sort((a, b) => b[1] - a[1])
    .slice(0, n)
    .map(([value, count]) => ({ value, count }));
  const bump = (map, key) => map.set(key, (map.get(key) || 0) + 1);
  const els = [...document.querySelectorAll("body *")].filter((el) => {
    const r = el.getBoundingClientRect();
    return r.width > 0 && r.height > 0;
  });
  const sizes = new Map(), families = new Map(), colors = new Map();
  const surfaces = new Map(), radii = new Map(), spaces = new Map(), shadows = new Map();
  for (const el of els) {
    const s = getComputedStyle(el);
    if ((el.textContent || "").trim().length > 0 && el.children.length === 0) {
      bump(sizes, Math.round(parseFloat(s.fontSize) * 10) / 10 + "/" + s.lineHeight);
      bump(families, s.fontFamily.split(",")[0].replace(/["']/g, "").trim());
      bump(colors, s.color);
    }
    if (s.backgroundColor !== "rgba(0, 0, 0, 0)") bump(surfaces, s.backgroundColor);
    if (s.borderRadius !== "0px") bump(radii, s.borderRadius);
    for (const side of ["paddingTop", "paddingLeft", "marginTop", "rowGap"]) {
      const v = parseFloat(s[side]);
      if (v > 0) bump(spaces, v);
    }
    if (s.boxShadow !== "none") bump(shadows, s.boxShadow.slice(0, 60));
  }
  return {
    url: location.href,
    viewport: window.innerWidth,
    font_sizes: top(sizes, 12),
    families: top(families, 4),
    text_colors: top(colors, 8),
    surfaces: top(surfaces, 8),
    radii: top(radii, 6),
    spacing: top(spaces, 12),
    shadows: top(shadows, 4),
  };
})()
```

Read the output as frequency, not as truth: the top three sizes are the scale, the tail is one-offs. Record the head, mention the tail in one line, and do not promote a count of one into a token.

Capture at the width you care about — a `1440` pass and a `390` pass produce different spacing heads, and the file needs both if the app is responsive.

## Recording what you took

Every extracted set gets an entry under `## Reference sources`, because a token whose origin is lost gets "cleaned up" by the next person:

```md
## Reference sources

### linear.app — 2026-10-22
- Took: type ladder (13/15/17/21), 6-step spacing, 8px radius on controls.
- Did not take: their palette (this app's direction is warmer), their motion.
- Viewport: 1280.
```

The "did not take" line matters as much as the "took" line: it is the record that you looked at something and chose, rather than copied.
