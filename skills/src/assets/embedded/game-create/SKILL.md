---
name: game-create
description: "Flow skill that takes a one-line game theme to a runnable web game prototype: concept convergence, GDD, tech selection, per-mechanic parallel implementation, then browser playtest. Trigger with /game-create followed by the theme. Use for game prototypes, mini-games, gameplay experiments on the web (做游戏/游戏原型/小游戏/玩法验证)."
type: flow
---

# Game Create

`/game-create <one-line theme>` turns a theme into a runnable web game prototype. The flow runtime (not the model improvising) drives five phases; each phase's output binds to the next via structured data.

## Phases

1. **concept** — converges the theme into a minimal concept: one-sentence pitch, core loop, input, win/lose condition, and the single fun moment. Over-scoped themes get cut to a 30-second playable loop.
2. **gdd** — expands the concept into a design doc (JSON): 3–5 independently implementable mechanics (`mechanics[]` with `name`/`spec`), technical `constraints` (single-file, no external assets, 60fps), and per-action `feedback` requirements.
3. **tech-select** — picks the web stack against the constraints: lightweight UI/cards → DOM+CSS; sprites/collision/particles → Canvas 2D; tilemaps/levels/platform physics → Phaser 3; real 3D → Three.js. Outputs file list and run instructions. Full decision table in `references/tech-selection.md`.
4. **implement** — fans out **one agent per mechanic** in parallel. Each agent gets its spec, the stack, the constraints, the feedback requirements, and a mandatory motion/feel spec (easing curves, duration budgets, hit-feedback presets from `references/motion-presets.md`).
5. **playtest** — two agents in parallel: one plays the game through `browser-control` (serve locally, navigate/screenshot/evaluate — loopback is approval-exempt) and reports blockers; one reviews the implementation against the GDD (edge cases, feedback completeness, constraint compliance).

Outputs (`concept`, `gdd`, `plan`, `implementations`, `review`) are recorded back into the conversation as a flow result item when the run finishes.

## Scope

Web-first by design: the playtest loop depends on `browser-control`, which closes the loop only in the browser. Unity/Godot/其他原生平台不在范围内——技术选型会引导到 Web 方案。

## Motion seam

Game feel comes from `motion-design`, declared as a skill dependency in `agents/odysseythink.yaml` — auto-injected alongside this flow on the extension selection path. Implement-phase agents additionally read it directly via `skills.list` + `skills.read`, which works on every path, including inside flow sub-agents.
