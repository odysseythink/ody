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
