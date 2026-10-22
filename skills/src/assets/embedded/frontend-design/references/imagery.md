# Imagery

What to do when a page needs a real photograph, and what to do when you cannot get one.

Applies to landing pages, marketing pages, portfolios, product pages with a hero object, and editorial layouts. Does **not** apply to auth, settings, tables and dashboards, where `bitmap_count: 0` is the correct answer and adding a stock photo makes the page worse. Decide by page kind before sourcing anything.

## The rule that outranks the rest

An image you cannot license is not an image you can ship. Every bitmap in the repo needs a source, a license, and an author on record — in `.ody-code/frontend/assets.md`, next to the file it describes:

```md
| File | Source | License | Author | Retrieved |
| --- | --- | --- | --- | --- |
| `public/hero-counter.jpg` | https://commons.wikimedia.org/wiki/File:… | CC BY-SA 4.0 | K. Tanaka | 2026-10-22 |
```

A missing row is the same defect as a missing file. "From Unsplash" is not a license: Unsplash's terms are not the same as public domain, and the attribution requirement differs by source. Record what the source's API said, not what you remember about the site.

## Finding one

Four sources have open APIs that return license metadata as JSON, which is the only kind of response this environment can read:

```
https://api.openverse.org/v1/images/?q=<terms>&license_type=commercial
https://commons.wikimedia.org/w/api.php?action=query&generator=search&gsrsearch=<terms>&prop=imageinfo&iiprop=url|extmetadata&format=json
https://api.artic.edu/api/v1/artworks/search?q=<terms>&fields=id,title,image_id,artist_display
https://collectionapi.metmuseum.org/public/collection/v1/search?q=<terms>
```

`WebFetch` returns their JSON. It will not return the bytes — `ody-web-search/src/fetch.rs` rejects any non-textual content type outright, so a direct image URL fails with `only reads textual sources`. Downloading is a shell job:

```
curl -L --fail --max-time 20 -o public/hero-counter.jpg "<image url from the JSON>"
```

That distinction decides your failure path: metadata is a `WebFetch` call away, the file is not. Check the file landed (`file public/hero-counter.jpg` should say JPEG or PNG, and `bitmap_broken` in `checks.md` § Craft metrics should be 0 after a reload) before writing a caption that assumes it did.

Search the subject's own world, not the category. "Noodle counter, Osaka, 1974" finds a photograph; "restaurant" finds the same stock dining room every other page uses. If the brief names an object, an instrument, a place or a trade, search that noun and prefer the least polished result that is still legible at the size you need.

## Marking it up

```html
<img src="hero-counter.jpg"
     srcset="hero-counter@2x.jpg 2x"
     width="1200" height="800"
     alt="Six stools at a wooden counter, steam rising from a single pot"
     loading="eager" decoding="async">
```

- `width` and `height` are not optional: without them the layout shifts when the image arrives, and a shift of 200px is a worse defect than a slow image. CSS `aspect-ratio` achieves the same thing when the file's dimensions are unknown.
- `loading="eager"` for the hero, `loading="lazy"` for everything below the fold. A lazy hero is a blank first screen on a slow connection.
- `srcset` only with a real second file. A `2x` entry pointing at the same URL is a lie the browser pays for.
- `<picture>` with `<source media>` when the composition must change per width — a wide hero cropped to a phone loses its subject.
- `object-fit: cover` with an explicit `aspect-ratio` on the container, so cropping is decided by CSS rather than by the viewport.

**Alt text is content, not compliance.** Decorative image: `alt=""` — present and empty, never omitted, because check 7 counts a missing attribute differently from an empty one. Content image: describe what it shows in the sentence you would say to someone on the phone, not "image of". If the image carries text, that text must be in the DOM somewhere; a headline baked into a JPEG is invisible to search and to a screen reader.

`background-image` is for texture and tint. A content photograph set as a background cannot be alt-texted, cannot be right-clicked, and disappears if images are blocked — the three failures users actually report.

## When you cannot get one

Three honest outcomes, in order of preference:

1. **No image, by design.** A typographic page with a strong composition is a real direction, not a fallback. Say so in the report: "no imagery — the brief is a ledger tool and the grid carries it".
2. **A generated texture or diagram.** CSS gradients, rules, and inline SVG that encode something true about the subject. These count as `inline_svg`, not `bitmap_count` — do not report them as photography.
3. **A labelled placeholder.** `placehold.co` or a flat rectangle is acceptable mid-build and unacceptable in a screen you call finished. If one survives to the report, it goes in **Not verified** with the words "placeholder image".

What is never acceptable: an `<img>` pointing at a file that does not exist, an invented credit line, or a hotlinked URL from a site whose terms you did not read. The first is a `bitmap_broken` finding, the second is a false statement in the deliverable, the third breaks the page the day the source reorganises.

## Failure path

No network, or no result with a usable license: stop looking, state it, and keep building.

> Images: none. Openverse and Wikimedia both timed out from this environment; the hero uses a CSS composition instead. The page renders without photography — nothing is pending.

That paragraph is a finished answer. The failure to avoid is the silent one: sourcing an image you never verified, writing a caption around it, and reporting the screen as complete — the reader finds out when the image does not load.

## Checklist

- [ ] Page kind actually wants photography (not auth, settings or a dashboard).
- [ ] File exists locally and opens; `bitmap_broken` is 0.
- [ ] A row in `assets.md` with source, license, author, date.
- [ ] `width`/`height` or `aspect-ratio` set — no layout shift.
- [ ] `alt` present on every `<img>`; empty only when decorative.
- [ ] Hero is `eager`, everything below the fold is `lazy`.
- [ ] No placeholder and no hotlink left in a screen reported as done.
