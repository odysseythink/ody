# Checks

Copy-paste `browser__evaluate` expressions for checks 1–6 in `SKILL.md`, plus the capture recipes that keep a round comparable.

Check 7 (accessibility) has its own expressions in `references/accessibility.md` — semantics, accessible names, the 24×24 floor, and reduced motion. They are kept apart because check 7 is the only one whose verdict is partly a reading of the markup rather than of the rendered frame.

§ Craft metrics supplies the six numbers behind the craft pass, and § One report runs checks 2–6 plus those metrics in a single call.

Each expression is an IIFE that returns JSON. The browser tool refuses a few patterns by policy — `document.cookie`, web storage, `eval(`, `setTimeout(`, and classic `function(` declarations — so these use arrow functions and read layout only. Keep that style when you adapt them.

## Frame facts

Ask this before writing a sentence that describes how a frame *looks*. It exists
because the same picture supports two opposite readings — "the page is shrunk"
and "one box is wider than the viewport" — and they lead to opposite fixes.

```js
(() => {
  const px = (value) => Number.parseFloat(value) || 0;
  const de = document.documentElement;
  const vw = window.innerWidth;
  const zoomOf = (value) => (!value || value === "normal" ? 1 : px(value));
  const scaled = [...document.querySelectorAll("body *")]
    .map((el) => {
      const style = getComputedStyle(el);
      return {
        tag: el.tagName.toLowerCase(),
        cls: String(el.className).slice(0, 40),
        transform: style.transform,
        zoom: zoomOf(style.zoom),
      };
    })
    .filter((item) => (item.transform !== "none" && item.transform !== "") || item.zoom !== 1)
    .slice(0, 8);
  const sample = document.querySelector("button,a[href],input,p");
  const box = sample ? sample.getBoundingClientRect() : null;
  return {
    viewport: { w: vw, h: window.innerHeight },
    visual_viewport: window.visualViewport
      ? {
          w: Math.round(window.visualViewport.width),
          scale: Number(window.visualViewport.scale.toFixed(3)),
        }
      : null,
    device_pixel_ratio: window.devicePixelRatio,
    root_zoom: zoomOf(getComputedStyle(de).zoom),
    root_font_size: px(getComputedStyle(de).fontSize),
    document_scroll: { w: de.scrollWidth, h: de.scrollHeight },
    content_wider_than_viewport: de.scrollWidth > vw + 1,
    sample: sample
      ? {
          tag: sample.tagName.toLowerCase(),
          font_size: px(getComputedStyle(sample).fontSize),
          w: Math.round(box.width),
          h: Math.round(box.height),
        }
      : null,
    scaled_elements: scaled,
  };
})()
```

How to read it, in the order the misreadings happen:

| The sentence you want to write | What has to be true |
| --- | --- |
| "the page is scaled down" / "shrunk" | `root_zoom` ≠ 1, a hit in `scaled_elements`, or `visual_viewport.scale` ≠ 1. The last one is the common cause on a phone: the browser widened the layout viewport to the content's minimum and scaled the page to fit. |
| "the text is too small" | `sample.font_size` says what it is. If `visual_viewport.scale` is below 1, the authored size is not what the reader sees — 14px at 0.893 renders as 12.5px, so the complaint is real and the fix is the scale, not the font. |
| "the card is cut off at the right" | `content_wider_than_viewport: true`, and the overflow check's `offenders` names the element and its `right`. When `content_wider_than_viewport` is false, nothing is cut — say the page is scaled instead. |
| "the card is cramped" | `sample.w` against `viewport.w`. A 420px card inside a 360px viewport is not cramped — it is too wide, and the fix is a `width`, not tighter padding. |

A 420-wide card on a 375px phone is the case that started this check. The
screenshot reads as "everything is smaller", the geometry agrees
(`scale: 0.893`) and supplies the reason (`innerWidth` grew to 420 while the
device is 375), and the overflow check on its own calls that frame clean. Read
from the picture, the fix is a smaller type scale; read from `innerWidth` versus
the device width, it is one `width` declaration on one card.

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

The expression has one blind spot, and on phones it is the common case: when the
content is wider than the viewport, the browser can widen the *layout* viewport
and shrink the page to fit instead of overflowing. `horizontal_scroll` stays
`false`, `offenders` stays `[]`, and the page is rendered at, say, 0.893 scale.
Read the scale in the same call:

```js
(() => ({
  inner_width: window.innerWidth,
  scroll_width: document.documentElement.scrollWidth,
  scale: window.visualViewport ? Number(window.visualViewport.scale.toFixed(3)) : null,
}))()
```

Compare `inner_width` with the width you passed to `set_viewport`.

`scale` must be 1 and `inner_width` must match the width you asked for. When
they do not, the page is shrink-to-fit: nothing overflows and everything is too
small, which is precisely the state that passes the overflow check and fails the
person holding the phone. A fixed-width child (`width: 420px` on a 375px device)
is the usual cause; `min(420px, 100% - 32px)` is the usual fix.

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

## Craft metrics

Six numbers behind the craft pass in `SKILL.md`. They exist because "hierarchy is weak" and "type feels small" are not checkable, while "the headline is 3 lines and 21 words" is.

```js
(() => {
  const px = (value) => Number.parseFloat(value) || 0;
  const shown = (el) => {
    const style = getComputedStyle(el);
    const r = el.getBoundingClientRect();
    return r.width > 0 && r.height > 0 && style.visibility !== "hidden"
      && style.display !== "none" && Number(style.opacity) > 0;
  };
  const textOf = (el) => (el.textContent || "").trim();
  const shownAll = [...document.querySelectorAll("body *")].filter(shown);

  const headline = document.querySelector("h1")
    || document.querySelector('[role="heading"][aria-level="1"]');
  const headStyle = headline ? getComputedStyle(headline) : null;
  const headBox = headline ? headline.getBoundingClientRect() : null;
  const lineHeight = headStyle ? px(headStyle.lineHeight) || px(headStyle.fontSize) * 1.2 : 0;
  const headlineText = headline ? textOf(headline) : "";

  const chOf = (el) => {
    const style = getComputedStyle(el);
    const probe = document.createElement("span");
    probe.textContent = "0000000000";
    probe.style.position = "absolute";
    probe.style.left = "-9999px";
    probe.style.whiteSpace = "pre";
    probe.style.fontFamily = style.fontFamily;
    probe.style.fontSize = style.fontSize;
    probe.style.fontWeight = style.fontWeight;
    probe.style.fontStyle = style.fontStyle;
    document.body.appendChild(probe);
    const one = probe.getBoundingClientRect().width / 10;
    probe.remove();
    return one || px(style.fontSize) * 0.5;
  };
  const paragraphs = [...document.querySelectorAll("p")].filter(shown);
  const body = paragraphs.sort((a, b) => textOf(b).length - textOf(a).length)[0] || null;
  const measure = body ? Math.round(body.getBoundingClientRect().width / chOf(body)) : null;

  const colourCount = new Map();
  const note = (value) => {
    if (!value || value === "rgba(0, 0, 0, 0)" || value === "transparent") return;
    colourCount.set(value, (colourCount.get(value) || 0) + 1);
  };
  shownAll.forEach((el) => {
    const style = getComputedStyle(el);
    note(style.color);
    note(style.backgroundColor);
    if (px(style.borderTopWidth) > 0 && style.borderTopStyle !== "none") note(style.borderTopColor);
  });
  const colours = [...colourCount.entries()].sort((a, b) => b[1] - a[1]);

  const images = [...document.querySelectorAll("img")].filter(shown);
  const bitmaps = images.length + [...document.querySelectorAll("video")].filter(shown).length;
  const broken = images.filter((img) => img.complete && img.naturalWidth === 0).length;
  const cssImages = shownAll
    .filter((el) => /url\(/.test(getComputedStyle(el).backgroundImage)).length;

  const boxes = (el) => {
    const style = getComputedStyle(el);
    if (style.display === "inline") return false;
    const sides = ["Top", "Right", "Bottom", "Left"].filter((side) =>
      px(style[`border${side}Width`]) > 0 && style[`border${side}Style`] !== "none").length;
    return style.backgroundColor !== "rgba(0, 0, 0, 0)" || style.boxShadow !== "none" || sides >= 2;
  };
  const boxAncestor = (el) => {
    let node = el.parentElement;
    let depth = 0;
    while (node && node !== document.body && depth < 8) {
      if (boxes(node)) return node;
      node = node.parentElement;
      depth += 1;
    }
    return null;
  };
  const insets = [...document.querySelectorAll("p,li,h1,h2,h3,h4,span,a,button,label,td,th,figcaption")]
    .filter((el) => el.children.length === 0 && textOf(el).length > 1 && shown(el))
    .map((el) => {
      const box = boxAncestor(el);
      if (!box) return null;
      const r = el.getBoundingClientRect();
      const pr = box.getBoundingClientRect();
      const ps = getComputedStyle(box);
      if (pr.width >= window.innerWidth * 0.9) return null;
      return {
        tag: el.tagName.toLowerCase(),
        text: textOf(el).slice(0, 28),
        inset: Math.round(r.left - pr.left - px(ps.borderLeftWidth)),
      };
    })
    .filter(Boolean)
    .sort((a, b) => a.inset - b.inset);

  const distinct = (pick) => [...new Set(shownAll.map(pick).filter(Boolean))];
  const families = distinct((el) => {
    const value = getComputedStyle(el).fontFamily;
    return value ? value.split(",")[0].trim().replace(/^["']|["']$/g, "") : null;
  });
  const sizes = distinct((el) =>
    textOf(el).length > 0 ? Math.round(px(getComputedStyle(el).fontSize)) : null).filter((n) => n > 0);
  const radii = distinct((el) => getComputedStyle(el).borderRadius).filter((v) => v !== "0px");
  const shadows = distinct((el) => getComputedStyle(el).boxShadow).filter((v) => v !== "none");
  const borders = distinct((el) => {
    const style = getComputedStyle(el);
    return style.borderTopStyle === "none" ? null : `${style.borderTopWidth} ${style.borderTopStyle}`;
  }).filter((v) => !v.startsWith("0px"));

  return {
    viewport: { w: window.innerWidth, h: window.innerHeight },
    scale: window.visualViewport ? Number(window.visualViewport.scale.toFixed(3)) : null,
    page_height_screens: Number((document.documentElement.scrollHeight / window.innerHeight).toFixed(1)),
    headline_lines: headBox && lineHeight ? Math.max(1, Math.round(headBox.height / lineHeight)) : null,
    headline_words: headlineText ? headlineText.split(/\s+/).filter(Boolean).length : null,
    headline_chars: headlineText ? headlineText.replace(/\s+/g, "").length : null,
    text_measure_ch: measure,
    colour_distinct: colours.length,
    colour_used_once: colours.filter(([, n]) => n === 1).map(([value]) => value).slice(0, 8),
    colour_top: colours.slice(0, 6),
    bitmap_count: bitmaps,
    bitmap_broken: broken,
    css_background_images: cssImages,
    inline_svg: document.querySelectorAll("svg").length,
    edge_inset_min: insets.length ? insets[0].inset : null,
    edge_inset_worst: insets.slice(0, 4),
    font_families: families.length,
    font_sizes: sizes.length,
    radii: radii.length,
    shadows: shadows.length,
    border_styles: borders.slice(0, 6),
  };
})()
```

What each number can settle, and what it cannot:

| Number | Read it as | Do not use it for | Applies to |
| --- | --- | --- | --- |
| `headline_lines` | 1–2 at 1280 is a headline; 3+ means it is doing a paragraph's job — cut it, do not shrink it. `headline_words` / `headline_chars` corroborate; CJK has few spaces, so read `chars` for Chinese copy and `words` for English | Judging whether the words are good | Any page with an `h1` |
| `text_measure_ch` | 45–75 ch is readable; under 40 the column is a sidebar, over 90 the eye loses the line on the return | Setting a `max-width` without checking the font — one page can carry two measures on purpose | Pages with prose; never on a nav, table or dashboard |
| `colour_distinct` | Context only. The signal is `colour_used_once`: a value used exactly once is either a mistake or an accent that still needs a job | "Too many colours" — a data page legitimately uses 12 | Every page, as context |
| `bitmap_count` | Whether a page that needs imagery has any. `bitmap_broken` > 0 is always a defect | A minimum. Auth, settings and dashboards correctly have 0; `css_background_images` counts décor, not content | Landing / marketing / portfolio |
| `edge_inset_min` | ≥ 8. `0` means the text starts at the very edge of the card it sits on; negative means it already overflows it. `edge_inset_worst` names the element | Measuring page gutters, vertical rhythm, or anything on the right side. It reads one number: the leading inset of text inside the nearest ancestor with a background, border or shadow — and only when that box is narrower than 90% of the viewport, because a full-bleed band's left edge is the page edge and 0 there is correct. `null` means the page has no such box | Pages with cards, panels or bordered rows |
| `font_families` / `font_sizes` / `radii` / `shadows` | Consistency, counted. One screen with 2 families, ≤ 6 sizes, 1–2 radii and 1–2 shadows reads deliberate; 4 radii and 5 shadows reads assembled | Proving a design is consistent — a token system can still be applied badly | Every page |

Two warnings, both earned:

- **The number is the input to a finding, never the finding.** "`colour_distinct: 9`" is not a defect; "`colour_used_once` contains three near-identical greys at `#6B6B6B`, `#6C6C6C`, `#6E6E6E`" is.
- **`page_height_screens` is context, not a target.** It is what you quote when a claim depends on "the fold" — 6.4 screens means the fold is at 14% of the page.

## One report

Checks 2–6 plus the craft metrics are six `browser__evaluate` calls if run one by one. This returns them together, so one round costs one call and the report lines come from one snapshot of the page instead of six.

```js
(() => {
  const px = (value) => Number.parseFloat(value) || 0;
  const safe = (name, run) => {
    try { return [name, run()]; }
    catch (error) { return [name, { error: String((error && error.message) || error) }]; }
  };
  const vw = window.innerWidth;
  const doc = document.documentElement;
  const shown = (el) => {
    const style = getComputedStyle(el);
    const r = el.getBoundingClientRect();
    return r.width > 0 && r.height > 0 && style.visibility !== "hidden" && style.display !== "none";
  };
  const all = [...document.querySelectorAll("body *")].filter(shown);
  const textOf = (el) => (el.textContent || "").trim();

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

  return Object.fromEntries([
    safe("frame", () => ({
      inner_width: vw,
      scroll_width: doc.scrollWidth,
      scale: window.visualViewport ? Number(window.visualViewport.scale.toFixed(3)) : null,
      shrunk: vw < window.screen.width && (window.visualViewport ? window.visualViewport.scale < 1 : false),
    })),
    safe("overflow", () => {
      const offenders = all
        .filter((el) => el.getBoundingClientRect().right > vw + 1)
        .slice(0, 6)
        .map((el) => ({ tag: el.tagName.toLowerCase(), right: Math.round(el.getBoundingClientRect().right) }));
      return { horizontal_scroll: doc.scrollWidth > vw + 1, offenders };
    }),
    safe("clip", () => {
      const hits = [...document.querySelectorAll("h1,h2,h3,h4,p,span,a,li,td,th,label,button")]
        .filter((el) => {
          if (el.children.length > 0) return false;
          const style = getComputedStyle(el);
          return (style.overflow !== "visible" || style.textOverflow === "ellipsis")
            && (el.scrollWidth > el.clientWidth + 1 || el.scrollHeight > el.clientHeight + 1);
        });
      return { count: hits.length, first: hits.slice(0, 4).map((el) => textOf(el).slice(0, 30)) };
    }),
    safe("hit_area", () => {
      const targets = [...document.querySelectorAll('a[href],button,[role="button"],input,select,textarea,summary')]
        .map((el) => el.getBoundingClientRect())
        .filter((r) => r.width > 0 && r.height > 0);
      const small = targets.filter((r) => r.width < 40 || r.height < 40);
      const min = targets.length
        ? targets.reduce(
            (acc, r) => ({ w: Math.min(acc.w, Math.round(r.width)), h: Math.min(acc.h, Math.round(r.height)) }),
            { w: 9999, h: 9999 },
          )
        : null;
      return { total: targets.length, under_40: small.length, min };
    }),
    safe("contrast", () => {
      const failures = [...document.querySelectorAll("h1,h2,h3,h4,h5,p,span,a,li,td,th,label,button,figcaption")]
        .filter((el) => el.children.length === 0 && textOf(el).length > 1)
        .map((el) => {
          const style = getComputedStyle(el);
          if (style.visibility === "hidden" || style.display === "none" || Number(style.opacity) === 0) return null;
          const size = px(style.fontSize);
          const large = size >= 24 || (size >= 18.66 && Number(style.fontWeight) >= 700);
          const fg = channels(style.color);
          if (!fg) return null;
          return { text: textOf(el).slice(0, 24), ratio: Number(ratio(fg, backgroundOf(el)).toFixed(2)), required: large ? 3 : 4.5 };
        })
        .filter((item) => item && item.ratio < item.required);
      return { failures: failures.length, worst: failures.sort((a, b) => a.ratio - b.ratio).slice(0, 4) };
    }),
    safe("focus", () => {
      const focusable = [...document.querySelectorAll('a[href],button,input,select,textarea,[tabindex]:not([tabindex="-1"])')]
        .filter((el) => !el.disabled && el.getBoundingClientRect().width > 0);
      const missing = focusable.slice(0, 8).filter((el) => {
        el.focus();
        const style = getComputedStyle(el);
        const ring = style.outlineStyle !== "none" && px(style.outlineWidth) > 0
          && style.outlineColor !== "rgba(0, 0, 0, 0)";
        return !ring && style.boxShadow === "none";
      });
      return { focusable: focusable.length, sampled: Math.min(8, focusable.length), no_ring: missing.length };
    }),
  ]);
})()
```

Three things this does and does not do:

- It **runs `focus()` on up to eight controls**, which moves the visible focus ring. Run it last in the round, or expect the frame after it to look focused.
- It reports **counts and the worst case**, not the full lists. When a count is non-zero, open the matching section above and run the detailed expression — that one names elements.
- It **agrees with the sections by construction, and must be re-checked when a threshold moves**. If you change the 40 in § Hit area or the 4.5 in § Contrast, change it here in the same commit and re-run both on one fixture; a composite that disagrees with its own detail is worse than six calls.

`shrunk` uses `window.screen.width` as a second opinion on § Frame facts. On a real phone browser `screen.width` is the device width and a shrink-to-fit shows up immediately; under emulation it can equal the emulated width, in which case trust `scale` and `inner_width` from § Frame facts instead.

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
| Everything looks shrunk at one width | § Frame facts before touching type. `visual_viewport.scale` < 1 is shrink-to-fit from a fixed-width child; `root_zoom` / `scaled_elements` is a CSS zoom; all at their defaults with `offenders` non-empty is real overflow |
| Text clipped after a copy change | fixed `height`, `max-height`, or `line-clamp` without a fallback |
| Contrast fails only in dark mode | a hard-coded hex instead of a token, or an opacity applied to text |
| Focus ring missing on one control | a component that resets `outline` without a replacement ring |
| Layout shifts between widths | a grid breakpoint that changes the column count without re-deriving spacing |
| Looks flat compared to before | the direction was not carried through — revisit `frontend-design`'s token plan before adding effects |

Two rounds of the same finding means the diagnosis is wrong. Re-read the frame and the computed value instead of adjusting the value again.
