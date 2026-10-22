---
name: frontend-style
description: Pick a visual direction with the user before writing UI. Runs a design read, renders three candidate identities on the real page, screenshots each, asks one three-option popup, then records the choice in design-system.md. Carries ten fixed identity packs in references/.
---

# Frontend Style

Ten fixed visual identities live in `references/`, one file each. None of them may be applied because you like it. A pack is applied only after the user picked it, and the pick is written down.

## When this runs

Run it when the visual direction is genuinely open:

- a new screen with no established look;
- a restyle or redesign where the brief says "make it feel X";
- the user asks for options, or says the current look is wrong without saying what it should be.

Do not run it when:

- the project already has `design-system.md` with a recorded direction — read that instead;
- the task is a fix inside an existing look (spacing, overflow, contrast, copy) — `frontend-verification` is the right skill;
- the user already named a direction in this or an earlier turn.

One decision per project, not per component. Re-asking on every section is worse than not asking.

## Step 1 — Design read

Before anything else, state one line that fixes the four inputs:

- **Page kind** — landing, pricing, portfolio, product, settings, auth.
- **Vibe words** — the brief's own adjectives, quoted.
- **Audience** — who reads it, and what they already trust.
- **Existing brand** — logo, palette, type already in the repo. Redesigns start here; it is input, not optional.

Example: `Reading this as: B2B pricing page for technical buyers, "calm and precise", existing blue accent must survive — editorial discipline, restrained motion.`

## Step 2 — Nominate exactly three

Pick three from the table. Not ten, not five.

| Pack | Identity | Nominate when the brief says |
| --- | --- | --- |
| `editorial-minimal` | Swiss broadsheet: warm paper, ink, vermillion, serif display + mono labels, hairline grids, indexed sections | calm, editorial, print-inspired, quietly premium |
| `brutalist-industrial` | Declassified blueprint: visible grid rules, heavy caps, one signal colour, mono coordinates | engineering, military, blueprint, hard-edged |
| `quiet-luxury` | Private-bank annual report: ivory, deep ink, brass or oxblood, serif with italic flourishes | expensive, restrained, family-office, heritage |
| `dark-tech` | Linear/Vercel school: near-black, hairline borders, mono metadata, one restrained accent | developer tool, dark, precise, infrastructure |
| `playful-color` | Sticker poster: giant heavy type, thick ink outlines, two saturated inks, hard offset shadows | consumer, young, loud, has a joke in it |
| `retro-print` | Riso / 70s–80s revival: warm stock, pigment inks, halftone dots, misregistered offsets, rubber stamps | retro, printed, music, culture, zine |
| `japanese-minimal` | Wabi-sabi: rice paper, sumi ink, one seal-red hanko, mincho serif, negative space as the layout | quiet, eastern, zen, single-object |
| `neo-brutalism` | Loud consumer web: every element ink-bordered, 6px hard shadows, one hot + one cool accent, chunky pills | brash, anti-polish, confident, deliberately rude |
| `corporate-trust` | Trust-first enterprise: white, one deep blue, system type, evidence tables, compliance chips | B2B, procurement, compliance, conservative |
| `cinematic-motion` | Site-of-the-day: paper canvas, ink type, one electric blue, alternating scene canvases, scrubbed counters | **a modifier, not a peer** — see below |

Rules for the shortlist:

1. **No two candidates may share a base palette.** `editorial-minimal`, `japanese-minimal`, `retro-print` and `quiet-luxury` are all warm paper; offering two of them gives the user a choice they cannot see.
2. **`cinematic-motion` is a layer.** It changes choreography, not identity. Never offer it as one of the three; apply it on top of whichever identity wins, if the brief wants a film.
3. **Say why each one fits the read**, in half a sentence each. A shortlist without reasons is three guesses.
4. **Mind the defaults.** `frontend-design` names three looks as the AI defaults (warm cream `#F4F1EA` + serif + terracotta; near-black + one acid accent; broadsheet hairlines with zero radius). Six packs live inside those clusters. That is allowed here and only here: **the user's pick is the brief's own words**, and the brief wins. But if you nominate one, the read must say what in the brief asks for it — otherwise pick something else.

## Step 3 — Render all three on the real page

A pack name is not a choice the user can make. Render first.

```
mkdir -p .ody-code/frontend/variants/<pack> .ody-code/frontend/shots
```

For each candidate, copy the page you are actually working on into that folder and change **only its tokens** — palette, type roles, spacing rhythm, corner and rule treatment. Structure and copy stay, so the three frames differ by identity and nothing else.

```
browser__navigate      { url: "…", wait_until: "network_idle" }
browser__set_viewport  { width: 1280, height: 900 }
browser__screenshot    { path: ".ody-code/frontend/shots/pricing@1280@<pack>.png" }
view_image             { path: ".ody-code/frontend/shots/pricing@1280@<pack>.png" }
```

`file://` URLs are refused by the browser tool. Serve the variants over local HTTP (a throwaway static server on a free port) and navigate to `http://127.0.0.1:<port>/variants/<pack>/<page>`.

Same width for all three, or the comparison is meaningless. Open every frame with `view_image` before showing it — a frame you did not look at is not evidence.

Then show the three frames in the reply, one line each: what the user should notice, not what you did.

## Step 4 — Ask with one popup

```
request_user_input {
  questions: [{
    id: "style_pack",
    header: "视觉方向",
    question: "这三个方向，哪一个最接近你想要的？",
    options: [
      { label: "<pack> — <3–6 word character>(Recommended)", description: "<the tradeoff you accept if this wins>" },
      { label: "<pack> — <3–6 word character>",            description: "<its tradeoff>" },
      { label: "<pack> — <3–6 word character>",            description: "<its tradeoff>" }
    ]
  }]
}
```

- Exactly three options. The schema says 2–3 and `frontend.md` repeats it; ten options is not a richer choice, it is an unreadable one.
- Recommended first, suffixed `(Recommended)`.
- Label starts with the pack name so the answer can be recorded verbatim.
- Description names a **tradeoff**, never a compliment ("denser, better for six plans, worse for one hero price").
- No "Other" option — the client appends one automatically, and the free-text field is where "none of these" lands.

**If the answer comes back empty or rejected**, that is not consent. `ody exec` returns `request_user_input is not supported in exec mode` and an empty `answers` object. In that case:

1. apply the recommended pack, because a turn cannot end with nothing built;
2. record it as `[C:INFERRED]`, never `[C:USER]`;
3. say in one line that the user did not actually choose, which pack was applied, and which two were not ruled out — so the next turn can re-ask rather than inherit a decision nobody made.

## Step 4.5 — Record it before you build

Write the decision into `.ody-code/frontend/design-system.md` (create it if absent) **before the first line of CSS**, not at the end:

```md
## Visual direction
- Pack: `neo-brutalism` `[C:USER]` — popup answered 2026-10-22T13:24Z
- Frame: `.ody-code/frontend/shots/pricing@1280@neo-brutalism.png`
- Not chosen: `playful-color`, `dark-tech`
- Tokens: paper `#F5F1EA` → `--surface`; ink `#1A1A18` → `--text`; …
```

Order matters because a build turn gets long: the record written last is the record that gets skipped, and the reply then claims a decision that exists nowhere on disk. After writing, read the block back with one command — if you cannot show it, it is not recorded.

Later rounds read this block and do not re-ask. When the user changes direction, rewrite the block and keep the old pack in a `## Previous directions` list — a reversal is information, not noise.

A block labelled `[C:INFERRED]` is a tentative default, not a decision: the user never answered. If the direction matters for the next turn, re-ask once — a popup costs one turn, inheriting a choice nobody made costs the whole screen.

## Applying the chosen pack

Read `references/<pack>.md` in full before writing CSS. It carries a palette, type roles, composition rules, and its own "Fail the page if" checklist.

- Lift its values into `design-system.md` as named tokens, then write CSS against the tokens. No raw hex in components.
- Append the pack's checklist to `audit.md` as extra cases. `frontend-verification` still runs in full on top of it — a pack is a direction, not a replacement for the checks.
- **Existing brand wins on colour.** If the repo already has tokens, logo colours, or a palette in use, borrow the pack's composition and type discipline and leave the palette alone, unless the user says to replace it. Say which one you did.

## Reporting

One line on what was chosen and why, plus the frame path. If a pack was applied without a popup answer, that line says `[C:INFERRED]` and names the two alternatives that were not ruled out.

The reply may name a pack only if the `## Visual direction` block is already on disk. Same rule as `frontend-verification`'s close-out: a claim about state needs a command result behind it, so if you have not read the block back, do not describe the direction as recorded.
