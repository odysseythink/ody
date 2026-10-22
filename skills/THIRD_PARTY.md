# Third-Party Skill Content

This crate embeds builtin skills under `src/assets/embedded/`. Some are
distilled from third-party open-source skill repositories. Each file that
contains distilled content carries a header noting its source; this file is
the inventory.

## emilkowalski/skills (MIT License)

- **Upstream**: https://github.com/emilkowalski/skills
- **Pinned commit**: `2d228630ba1446365f614a108cb8252f1feda202` (synced 2026-10-12; previously `d23d7f88a2e21c9e4b1418c7abe420f5c1052ba7`, cloned 2026-09-13)
- **Copyright**: Emil Kowalski. Full MIT license text: <https://github.com/emilkowalski/skills/blob/main/LICENSE>
- **Used by**: `motion-design` (added 2026-09-13), `frontend-design` interaction baseline (added 2026-10-12)

| Embedded file | Distilled from |
|---|---|
| `motion-design/SKILL.md` | animation decision framework + defaults shared across the repo; tool-selection order from `skills/animate` |
| `motion-design/references/motion-vocabulary.md` | `skills/animation-vocabulary` (glossary quoted near-verbatim) |
| `motion-design/references/motion-audit.md` | `skills/improve-animations` (AUDIT.md), `skills/review-animations` (STANDARDS.md), `skills/emil-design-eng` (deduplicated) |
| `motion-design/references/interaction-principles.md` | `skills/apple-design` (motion/gesture sections; materials, typography, and general design foundations omitted) |
| `motion-design/references/animation-recipes.md` | `skills/animate` (RECIPES.md, vendored near-verbatim) |
| `motion-design/references/animation-opportunities.md` | `skills/find-animation-opportunities` (SKILL.md; invocation ritual dropped, cross-skill handoffs adapted) |
| `frontend-design/SKILL.md` (interaction-baseline section only) | press/entrance/stagger values shared across the repo |
| `game-create/references/motion-presets.md` | values from the same five skills, adapted for game-feel presets (`tech-selection.md` is original content, no third-party text) |

Known upstream skills deliberately not imported in this sync: `break-ui`,
`animation-performance`, `prototype`, `pick-ui-library`, `mobile-native`,
`animate-expo`, `ask-sonner`, `write-swift` — re-evaluate at the next
quarterly review.

### Sync policy

Quarterly manual review of upstream changes. When syncing, update the pinned
commit in this file and re-check the attribution headers inside each embedded
file.

## anthropics/skills (Apache License 2.0)

- **Upstream**: https://github.com/anthropics/skills (`skills/frontend-design`)
- **Pinned commit**: `41bbe19d1a1a` (2026-09-03, latest commit touching that path as of 2026-10-12)
- **Copyright**: Anthropic. Full Apache-2.0 license text: <https://github.com/anthropics/skills/blob/main/LICENSE>
- **Used by**: `frontend-design` (registered 2026-10-12)

`frontend-design/SKILL.md` is an in-repo adaptation of Anthropic's
`frontend-design` skill (paraphrased, not vendored), tracking the upstream
framing that non-user-triggered decorative motion reads as AI-generated while
motion answering a person's action is welcome. When syncing, re-read the
upstream SKILL.md and re-check this paragraph's framing against it.

## jakubkrehel/make-interfaces-feel-better (MIT License)

- **Upstream**: https://github.com/jakubkrehel/make-interfaces-feel-better
- **Pinned commit**: `35545ea1512ad59fa463e6b1f95ca9c052981fe6` (fetched 2026-10-12)
- **Copyright**: Jakub Krehel. Full MIT license text: <https://github.com/jakubkrehel/make-interfaces-feel-better/blob/main/LICENSE>
- **Used by**: `make-interfaces-feel-better` (added 2026-10-12)

Vendored near-verbatim: upstream `skills/make-interfaces-feel-better/SKILL.md`
plus its five topic files, moved under `references/` (Quick Reference links
updated) with a Boundary paragraph added to the SKILL.md and attribution
footers appended to every file. When syncing, re-copy upstream, re-apply the
`references/` move and the Boundary paragraph, and update the pinned commit
here and in each footer.

## gamedev-skills/awesome-gamedev-agent-skills (Apache License 2.0)

- **Upstream**: https://github.com/gamedev-skills/awesome-gamedev-agent-skills
- **Pinned commit**: `b105e1cf617adf0b68ed98790a716bbb60993179` (cloned 2026-09-21)
- **Copyright**: the upstream authors. Full Apache-2.0 license text:
  <https://github.com/gamedev-skills/awesome-gamedev-agent-skills/blob/main/LICENSE>
- **Used by**: 64 game-development skills + the router (added 2026-09-21)

Unlike the emilkowalski distillation above, these skills are **vendored
verbatim** (Apache-2.0 permits verbatim redistribution with attribution).
Every vendored `.md` file carries a `Vendored verbatim from ...` header
comment; only the router was renamed (`router` -> `gamedev-router`, frontmatter
`name:` updated) to avoid a generic name in the flat system-skill namespace.

| Vendored set | Skill dirs |
|---|---|
| Router (1) | `gamedev-router` |
| Godot (15) | `godot-gdscript`, `godot-nodes-scenes`, `godot-signals-groups`, `godot-2d-movement`, `godot-tilemap`, `godot-physics`, `godot-ui-control`, `godot-animation`, `godot-shaders`, `godot-3d-essentials`, `godot-resources`, `godot-audio`, `godot-multiplayer`, `godot-export`, `godot-csharp` |
| Unity (8) | `unity-csharp-scripting`, `unity-input-system`, `unity-physics`, `unity-animation`, `unity-scriptableobjects`, `unity-tilemap-2d`, `unity-navmesh`, `unity-build-pipeline` |
| Unreal (6) | `unreal-blueprints`, `unreal-cpp-gameplay`, `unreal-enhanced-input`, `unreal-behavior-trees`, `unreal-niagara`, `unreal-packaging` |
| Web engines (6) | `phaser-core`, `phaser-arcade-physics`, `pixijs-rendering`, `threejs-scene-setup`, `threejs-gltf-loading`, `threejs-materials-lighting` |
| Other engines (1) | `bevy-ecs` |
| Disciplines (15) | `create-game-assets`, `game-ai`, `ai-behavior-trees-utility-ai`, `procedural-gen`, `dialogue-systems`, `save-systems`, `audio-design`, `shader-programming`, `physics-tuning`, `level-design`, `input-systems`, `game-feel`, `camera-systems`, `game-ui-ux`, `performance-optimization` |
| Genres (9) | `platformer`, `roguelike`, `rpg`, `fps-shooter`, `tower-defense`, `card-game`, `visual-novel`, `survival-crafting`, `puzzle` |
| Workflows (4) | `game-jam`, `prototype-fast`, `steam-publish`, `itch-publish` |

**Not vendored (deliberate subset, user decision 2026-09-21)**: `roblox-*` (7;
closed platform ecosystem, off-target), `pygame-core`, `love2d-core`
(non-primary commercial stacks). Upstream `docs/` and per-category `README.md`
files are also not vendored.

### Sync policy

Same quarterly manual review as above. When syncing, re-run the flattening
copy, update the pinned commit here and in every vendored file header, and
re-check the exclusion list against upstream additions.

### Not imported (license unverifiable)

Skills surfaced via the SkillForge marketplace but excluded from embedding due
to unclear provenance/licensing: `interaction-design` (Owl-Listener),
`design-taste-frontend` (leonxlnx, appears derived from a Lovable built-in
skill). Consulted only as inspiration; no content copied.

> Not to be confused with the section below. `design-taste-frontend`
> (leonxlnx) is still not imported. The vendored packs come from a different
> repository, `design-taste`, whose root `LICENSE` is MIT and is quoted there.

## design-taste genre packs (MIT License)

- **Upstream**: local checkout at `~/workspace/frontend_work/design-taste-main`,
  README points at <https://github.com/> (repository has no `.git`, so there is
  no commit to pin — see *Sync policy* below).
- **License**: MIT. Root `LICENSE` reads `Copyright (c) 2026 Mustafa`.
- **Used by**: `frontend-style` (added 2026-10-22)

Ten fixed identity packs are vendored under
`frontend-style/references/<pack>.md`. In the profile the packs are selected by
design-taste's own core skill; here the selector is `frontend-style/SKILL.md`,
which renders three candidates on the caller's page and lets the user pick.

| Embedded file | Upstream path | Upstream sha256 (12) |
|---|---|---|
| `frontend-style/references/editorial-minimal.md` | `skills/editorial-minimal/SKILL.md` | `fd6c52053a99` |
| `frontend-style/references/brutalist-industrial.md` | `skills/brutalist-industrial/SKILL.md` | `7e16d97e74a3` |
| `frontend-style/references/quiet-luxury.md` | `skills/quiet-luxury/SKILL.md` | `3f439a21dbb2` |
| `frontend-style/references/dark-tech.md` | `skills/dark-tech/SKILL.md` | `6b856e651ba2` |
| `frontend-style/references/playful-color.md` | `skills/playful-color/SKILL.md` | `4e0b86bd1842` |
| `frontend-style/references/retro-print.md` | `skills/retro-print/SKILL.md` | `9036bd4daf53` |
| `frontend-style/references/japanese-minimal.md` | `skills/japanese-minimal/SKILL.md` | `7b96ba5a48af` |
| `frontend-style/references/neo-brutalism.md` | `skills/neo-brutalism/SKILL.md` | `bc71fa3bba39` |
| `frontend-style/references/corporate-trust.md` | `skills/corporate-trust/SKILL.md` | `4b692ddc70c6` |
| `frontend-style/references/cinematic-motion.md` | `skills/cinematic-motion/SKILL.md` | `b5e4b30e362c` |
| `frontend-verification/references/accessibility.md` | `skills/taste-accessibility/SKILL.md` | `4d293f850d91` |

The last row is not an identity pack and is not vendored into `frontend-style/`.
It lands in `frontend-verification/` as check 7 because accessibility is owed on every
page, not chosen per project — a pack is picked from a popup, this is not optional. The
upstream file is prose rules; the embedded version is **rewritten**, not copied, so the
rules become four `browser__evaluate` expressions plus a named list of what no expression
can answer. What survives verbatim from upstream is the rule set and the WCAG numbers.

Adaptations applied on import (bodies otherwise verbatim):

- YAML frontmatter dropped (these are reference files, not skills); an
  attribution header and a scope line added to each file.
- `cinematic-motion` referred to upstream's `taste-gsap` four times for
  execution values; those references now point at `motion-design`, which is
  this bundle's motion authority.
- `cinematic-motion` is documented as a **modifier layer** in `SKILL.md` rather
  than a peer identity, because its content is choreography, not palette/type.
- `taste-accessibility` is restructured: upstream's seven rule sections are
  reorganised around what can be measured, each measurable rule gains an
  expression, and the four checks that need a human (tab walk, screen reader,
  dialog focus, alt-text quality) are separated out so they cannot be reported as
  passed. Upstream's 24×24 / 44×44 / 4.5:1 / 3:1 / 200% requirements are kept.

Deliberately not imported: design-taste's `design-taste` core skill (its
design-read procedure is reimplemented in `frontend-style/SKILL.md`) and the
remaining `taste-*` craft/code skills (overlap `frontend-design`,
`make-interfaces-feel-better`, `motion-design`, `frontend-verification`).

### Sync policy

The checkout carries no git metadata, so pinning is by content digest above.
When syncing, re-copy each upstream `SKILL.md`, recompute the twelve-character
sha256 prefixes in this table, and re-apply the adaptations listed here.

