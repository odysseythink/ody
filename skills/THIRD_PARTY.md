# Third-Party Skill Content

This crate embeds builtin skills under `src/assets/embedded/`. Some are
distilled from third-party open-source skill repositories. Each file that
contains distilled content carries a header noting its source; this file is
the inventory.

## emilkowalski/skills (MIT License)

- **Upstream**: https://github.com/emilkowalski/skills
- **Pinned commit**: `d23d7f88a2e21c9e4b1418c7abe420f5c1052ba7` (cloned 2026-09-13)
- **Copyright**: Emil Kowalski. Full MIT license text: <https://github.com/emilkowalski/skills/blob/main/LICENSE>
- **Used by**: `motion-design` (added 2026-09-13)

| Embedded file | Distilled from |
|---|---|
| `motion-design/SKILL.md` | animation decision framework + defaults shared across the repo |
| `motion-design/references/motion-vocabulary.md` | `skills/animation-vocabulary` (glossary quoted near-verbatim) |
| `motion-design/references/motion-audit.md` | `skills/improve-animations` (AUDIT.md), `skills/review-animations` (STANDARDS.md), `skills/emil-design-eng` (deduplicated) |
| `motion-design/references/interaction-principles.md` | `skills/apple-design` (motion/gesture sections; materials, typography, and general design foundations omitted) |
| `game-create/references/motion-presets.md` | values from the same five skills, adapted for game-feel presets (`tech-selection.md` is original content, no third-party text) |

### Sync policy

Quarterly manual review of upstream changes. When syncing, update the pinned
commit in this file and re-check the attribution headers inside each embedded
file.

### Not imported (license unverifiable)

Skills surfaced via the SkillForge marketplace but excluded from embedding due
to unclear provenance/licensing: `interaction-design` (Owl-Listener),
`design-taste-frontend` (leonxlnx, appears derived from a Lovable built-in
skill). Consulted only as inspiration; no content copied.
