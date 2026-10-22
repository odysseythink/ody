# Accessibility

Adapted from design-taste `skills/taste-accessibility` (MIT, © 2026 Mustafa) — see `skills/THIRD_PARTY.md`.

This is check 7 of `SKILL.md`, not a style pack. Two consequences:

- It is not in `frontend-style/references/` and never gets nominated in a popup. A visual direction is chosen; this is owed.
- It runs on every tier. `quick` runs the name-and-label pass (§1, §2); `full` runs all four
  sections plus the manual pass (§5).

The reason it needs its own check is narrow and worth stating: a `<div role="button">` with a
placeholder-only label, three skipped heading levels, and an icon button with no accessible name
all render beautifully. Checks 1–6 pass on that page — console clean, nothing overflows, nothing
clips, 40×40 hit area, 4.5:1 contrast, focus ring present. The page is unusable with a keyboard
and silent to a screen reader, and every number said it was fine.

## What these expressions can and cannot answer

Run these; they return JSON, same policy as `checks.md` (no `function(`, no `eval(`, layout and
attributes only).

| Question | Measurable? |
| --- | --- |
| Is any input missing a programmatically associated label? | yes — §2 |
| Are there empty buttons or links (no accessible name)? | yes — §2 |
| Do heading levels skip? | yes — §1 |
| Does a control sit under 24×24 with no spacing exception? | yes — §3 |
| Is `aria-hidden` on something focusable? | yes — §2 |
| Is there a duplicate `id` that breaks every `aria-labelledby` pointing at it? | yes — §2 |
| Is reduced motion actually handled? | partly — §4, only when the stylesheet is same-origin |
| Is the tab *order* sane? | no — needs real key events, §5 |
| Does a screen reader say something sensible? | no — needs a screen reader, §5 |
| Is the copy plain enough? | no — judgement, §5 |

The second half of that table is the point of §5: those four sentences may not appear in a report
unless you did the thing. "Keyboard accessible" written after running an expression is a claim
about a `tabindex` attribute, not about a person pressing Tab.

## 1. Semantics

```js
(() => {
  const headings = [...document.querySelectorAll("h1,h2,h3,h4,h5,h6,[role=heading]")]
    .map((el) => ({
      level: el.tagName === "DIV" || el.getAttribute("role") === "heading"
        ? Number(el.getAttribute("aria-level")) || 2
        : Number(el.tagName.slice(1)),
      text: (el.textContent || "").trim().slice(0, 50),
    }));
  const skips = [];
  for (let i = 1; i < headings.length; i += 1) {
    if (headings[i].level - headings[i - 1].level > 1) {
      skips.push({ from: headings[i - 1], to: headings[i] });
    }
  }
  const landmarks = {};
  ["header", "nav", "main", "footer", "aside"].forEach((tag) => {
    landmarks[tag] = document.querySelectorAll(tag).length;
  });
  const emptyAlt = [...document.querySelectorAll("img")].filter((img) => !img.hasAttribute("alt")).length;
  const decorativeSvg = [...document.querySelectorAll("svg")]
    .filter((svg) => !svg.hasAttribute("aria-hidden") && !svg.getAttribute("aria-label"))
    .length;
  return {
    h1_count: headings.filter((h) => h.level === 1).length,
    heading_count: headings.length,
    first_heading: headings[0] || null,
    level_skips: skips.slice(0, 8),
    landmarks,
    images_missing_alt_attr: emptyAlt,
    svg_without_aria_hidden_or_label: decorativeSvg,
    heading_outline: headings.slice(0, 30),
  };
})()
```

Read it as an outline, not a score:

- `h1_count` must be 1 on a page. Two means the outline has two roots — usually a section title marked up as the page title.
- `level_skips` must be `[]`. `h1 → h3` is the failure that looks fine and reads as a flat list to a screen reader; the fix is an `h2` that might be visually hidden.
- `landmarks.main` must be ≥ 1. Screen-reader users jump by landmark; a page with no `<main>` has no "skip to content".
- `images_missing_alt_attr` counts images with **no `alt` attribute at all**. `alt=""` is correct for decoration and is not counted. The failure mode is not "forgot alt" but "wrote `alt="image"`", which no expression can catch — that one is §5.

`heading_outline` is the cheapest review tool here: read only those lines. If they do not describe the page, the page has no structure regardless of how it looks.

## 2. Names, labels, and the things that break them

```js
(() => {
  const idCount = {};
  [...document.querySelectorAll("[id]")].forEach((el) => {
    idCount[el.id] = (idCount[el.id] || 0) + 1;
  });
  const duplicateIds = Object.keys(idCount).filter((id) => idCount[id] > 1).slice(0, 10);
  const textOf = (el) => (el ? (el.textContent || "").trim() : "");
  const nameOf = (el) => {
    const by = el.getAttribute("aria-labelledby");
    if (by) {
      const joined = by.split(/\s+/).map((id) => textOf(document.getElementById(id))).join(" ");
      if (joined.trim()) return joined.trim();
    }
    const label = el.getAttribute("aria-label");
    if (label && label.trim()) return label.trim();
    if (el.labels && el.labels.length) return [...el.labels].map(textOf).join(" ").trim();
    const own = textOf(el);
    if (own) return own;
    const img = el.querySelector("img[alt]");
    if (img) return img.getAttribute("alt").trim();
    return (el.getAttribute("title") || "").trim();
  };
  const fields = [...document.querySelectorAll("input,select,textarea")]
    .filter((el) => !["hidden", "submit", "button", "reset"].includes((el.type || "").toLowerCase()))
    .filter((el) => el.getBoundingClientRect().width > 0);
  const unlabeled = fields
    .filter((el) => !nameOf(el))
    .map((el) => ({
      tag: el.tagName.toLowerCase(),
      type: el.type || "",
      name: el.getAttribute("name") || "",
      placeholder: el.getAttribute("placeholder") || "",
      id: el.id || "",
    }))
    .slice(0, 10);
  const noAutocomplete = fields
    .filter((el) => el.tagName === "INPUT" && !el.getAttribute("autocomplete"))
    .map((el) => el.type + ":" + (el.getAttribute("name") || el.id || "?"))
    .slice(0, 10);
  const interactive = [...document.querySelectorAll(
    'a[href],button,[role="button"],[role="link"],[role="tab"],[role="menuitem"],input[type="submit"],summary'
  )];
  const unnamed = interactive
    .filter((el) => el.getBoundingClientRect().width > 0)
    .filter((el) => !nameOf(el))
    .map((el) => ({ tag: el.tagName.toLowerCase(), cls: String(el.className).slice(0, 40) }))
    .slice(0, 10);
  const hiddenFocusable = [...document.querySelectorAll('[aria-hidden="true"]')]
    .filter((el) => el.querySelector('a[href],button,input,select,textarea,[tabindex]:not([tabindex="-1"])'))
    .length;
  const clickableNonButton = [...document.querySelectorAll("div[onclick],span[onclick]")].length;
  const placeholderOnly = fields
    .filter((el) => nameOf(el) === "" && el.getAttribute("placeholder"))
    .length;
  return {
    fields: fields.length,
    unlabeled_fields: unlabeled,
    fields_without_autocomplete: noAutocomplete,
    interactive_count: interactive.length,
    unnamed_interactive: unnamed,
    aria_hidden_containing_focusable: hiddenFocusable,
    click_handlers_on_div_or_span: clickableNonButton,
    placeholder_as_only_label: placeholderOnly,
    duplicate_ids: duplicateIds,
  };
})()
```

Pass conditions, and what each one actually catches:

| Field | Pass | The failure behind it |
| --- | --- | --- |
| `unlabeled_fields` | `[]` | Placeholder used as the label. It vanishes on typing, is not announced as a name by most screen readers, and fails 4.1.2. Fix with `<label for>`, not a second placeholder. |
| `unnamed_interactive` | `[]` | The icon-only button. `nameOf` falls back through `aria-label`, descendant text, and descendant `img[alt]`; empty means a pointer-only control. |
| `placeholder_as_only_label` | `0` | The specific, most common instance of the row above — split out because it is the one that gets defended as "the placeholder says Email". |
| `click_handlers_on_div_or_span` | `0` | A `div` with `onclick` is not focusable, not in the tab order, and does not respond to Enter. Use `<button>`. |
| `aria_hidden_containing_focusable` | `0` | Focus lands inside an `aria-hidden` subtree: the user tabs into a control the screen reader refuses to name. |
| `duplicate_ids` | `[]` | Every `aria-labelledby` / `<label for>` pointing at a duplicated id now names the wrong element, silently. |
| `fields_without_autocomplete` | report, do not block | Not a failure. `autocomplete="email"` / `"one-time-code"` is the difference between a form and a chore on a phone; say the count. |

`nameOf` is an approximation of the accessible-name algorithm, not an implementation of it. It
covers labelledby → aria-label → `<label>` → text → `img[alt]` → title. It will miss CSS
`::before` content and `aria-describedby`-only naming; both are rare enough that a hit here is
worth fixing rather than disputing.

## 3. Target size (WCAG 2.2 AA, 24×24)

`checks.md` § Hit area enforces 40×40, which is stricter than the standard and correct for
touch-first layouts. The AA floor is separate and lower, and the spacing exception matters:

```js
(() => {
  const box = [...document.querySelectorAll(
    'a[href],button,[role="button"],input:not([type="hidden"]),select,textarea,summary,[tabindex]:not([tabindex="-1"])'
  )]
    .map((el) => ({ el, r: el.getBoundingClientRect() }))
    .filter(({ r }) => r.width > 0 && r.height > 0)
    .map(({ el, r }) => ({
      el,
      left: r.left,
      right: r.right,
      top: r.top,
      bottom: r.bottom,
      w: Math.round(r.width),
      h: Math.round(r.height),
    }));
  const centre = (b) => ({ x: (b.left + b.right) / 2, y: (b.top + b.bottom) / 2 });
  const centreGap = (a, b) => {
    const p = centre(a);
    const q = centre(b);
    return Math.hypot(p.x - q.x, p.y - q.y);
  };
  const labelOf = (el) => {
    const aria = el.getAttribute("aria-label");
    if (aria && aria.trim()) return aria.trim().slice(0, 30);
    const own = (el.textContent || "").trim();
    if (own) return own.slice(0, 30);
    const hint = el.getAttribute("placeholder") || el.getAttribute("name") || el.id || String(el.className);
    return (hint || el.tagName.toLowerCase()).slice(0, 30);
  };
  // 2.5.8 inline exception: a link inside a sentence is exempt from the size floor.
  const inlineInText = (el) => {
    if (el.tagName !== "A") return false;
    const display = getComputedStyle(el).display;
    if (display !== "inline" && display !== "inline-block") return false;
    const parent = el.parentElement;
    if (!parent) return false;
    return (parent.textContent || "").trim().length > (el.textContent || "").trim().length + 2;
  };
  const small = box.filter((b) => b.w < 24 || b.h < 24);
  const evaluated = small.map((b) => {
    const nearest = box
      .filter((other) => other.el !== b.el)
      .reduce((min, other) => Math.min(min, centreGap(b, other)), Number.POSITIVE_INFINITY);
    return {
      tag: b.el.tagName.toLowerCase(),
      label: labelOf(b.el),
      w: b.w,
      h: b.h,
      nearest_centre_gap: Number.isFinite(nearest) ? Math.round(nearest) : null,
      inline_text_link: inlineInText(b.el),
      spacing_exception: nearest >= 24,
    };
  });
  return {
    total: box.length,
    under_24: evaluated.length,
    passing_by_spacing: evaluated.filter((item) => item.spacing_exception && !item.inline_text_link).length,
    passing_as_inline_text: evaluated.filter((item) => item.inline_text_link).length,
    violations: evaluated.filter((item) => !item.spacing_exception && !item.inline_text_link),
  };
})()
```

Pass is `violations.length === 0`. Two details decide whether this check is real or
decorative:

- **The exception is a nearest-neighbour test, not an "is anything far away" test.** A 20×20 icon
  with another icon 2px to its right fails even though the page footer is 800px from both. Compare
  the centre distance against 24, the way 2.5.8 words it ("a 24 CSS pixel diameter circle centred
  on each"), so a 20px target needs ≥24px of air to the *closest* other target.
- **Inline links in a sentence are exempt.** Without that clause every prose link in a footer
  reports a failure, the list becomes noise, and a check that always complains is a check that
  gets skimmed. `passing_as_inline_text` is reported so a failure count of zero still shows how
  many items relied on the exemption.

Report both numbers when the tier is `full`: `under_24` is WCAG AA, `checks.md` § Hit area's
40×40 is the project bar. They are allowed to disagree, and the report says which one it means.

## 4. Motion and zoom

```js
(() => {
  const reduce = window.matchMedia("(prefers-reduced-motion: reduce)");
  let guardedRules = 0;
  let unreadableSheets = 0;
  const walk = (rules) => {
    [...rules].forEach((rule) => {
      if (rule.media && String(rule.media.mediaText).includes("prefers-reduced-motion")) {
        guardedRules += 1;
      }
      if (rule.cssRules) walk(rule.cssRules);
    });
  };
  [...document.styleSheets].forEach((sheet) => {
    try {
      if (sheet.cssRules) walk(sheet.cssRules);
      else unreadableSheets += 1;
    } catch (error) {
      unreadableSheets += 1;
    }
  });
  const animated = [...document.querySelectorAll("body *")].filter((el) => {
    const style = getComputedStyle(el);
    const duration = style.transitionDuration.split(",").map(Number.parseFloat);
    const delay = style.animationDuration.split(",").map(Number.parseFloat);
    return duration.some((d) => d > 1) || delay.some((d) => d > 1 && style.animationName !== "none");
  }).length;
  const autoplay = [...document.querySelectorAll("video[autoplay],audio[autoplay]")]
    .filter((el) => !el.muted).length;
  return {
    user_prefers_reduced_motion: reduce.matches,
    prefers_reduced_motion_rules: guardedRules,
    unreadable_stylesheets: unreadableSheets,
    elements_animating_over_1s: animated,
    unmuted_autoplay_media: autoplay,
    document_scroll_width: document.documentElement.scrollWidth,
    viewport_width: window.innerWidth,
  };
})()
```

- `prefers_reduced_motion_rules === 0` with `elements_animating_over_1s > 0` is a failure: the
  page moves and has no way not to. Paste this expression after
  `browser__execute_raw_cdp { Emulation.setEmulatedMedia }` if you want to see the reduced state,
  but reading the rules is enough to know whether one exists.
- `unreadable_stylesheets > 0` means cross-origin CSS — the count is a floor, not a total. Say
  "not verified" for motion rather than reporting `0` as a pass.
- `unmuted_autoplay_media` must be `0`. Autoplaying sound is a 1.4.2 failure with no visual tell.

**Zoom.** 200% text zoom on a 1280px layout is the same box as a 640px viewport: run the
§ Overflow expression at 640 and call it the zoom test, saying so. Reflow (1.4.10) is a different
number — **320 CSS px** — and `SKILL.md`'s default five widths start at 360, so add 320 once on
the primary route.

Neither test is passed by "it looks fine at 360": `content_wider_than_viewport` and the scale in
§ Frame facts are the answer, and text clipping at that width is § Clipping's job.

## 5. What no expression answers

Four sentences that require actually doing the thing. If you did not, they go in the report's
**Not verified** list — they do not get softened into "should be keyboard accessible".

1. **Tab walk.** Press Tab from the address bar to the last control. Order follows visual order,
   every stop shows a ring, nothing traps, Esc closes what opened. A focus-ring expression proves
   a ring exists; it does not prove the order makes sense.
2. **Screen reader pass.** Page title, heading outline, each control announced with a name and
   role, errors announced when they appear. One minute with VoiceOver (`Cmd+F5`) beats any
   inference here.
3. **Dialog behaviour.** Focus moves in on open, stays in until close, returns to the trigger.
   `role="dialog"` + `aria-modal="true"` and a label are the markup half; the focus half is a
   keyboard test.
4. **Plain language and alt text quality.** `alt="a blue line chart"` passes every expression in
   this file and tells a blind reader nothing. The test is whether the alt carries the *meaning*:
   "conversion fell from 18% to 9% in May".

## Checklist — fail the page if

- [ ] `h1_count` ≠ 1, or `level_skips` is non-empty.
- [ ] `landmarks.main` is 0.
- [ ] Any `unlabeled_fields` entry.
- [ ] Any `unnamed_interactive` entry.
- [ ] `click_handlers_on_div_or_span` > 0.
- [ ] `aria_hidden_containing_focusable` > 0.
- [ ] `duplicate_ids` non-empty.
- [ ] `under_24_without_spacing` non-empty.
- [ ] `unmuted_autoplay_media` > 0.
- [ ] Motion present with `prefers_reduced_motion_rules` 0.
- [ ] Any error state communicated by colour alone (contrast check cannot see this — read the
      markup for a text message and an `aria-live` region).
- [ ] `input` with `type="text"` where `email` / `tel` / `password` / `number` / `search` was meant.

The last two are markup reads, not measurements. They stay on the list because colour-only error
states and `type="text"` on an email field are the two a11y defects that survive every automated
pass and are found by a human in the first ten seconds.
