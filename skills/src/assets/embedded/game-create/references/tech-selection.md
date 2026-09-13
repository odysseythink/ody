# Tech Selection — Web Game Stack Decision Table

Prototype rule: **pick the simplest stack that can express the core fun moment.** You can migrate later; a playable ugly prototype beats a beautiful skeleton.

| Need | Stack | Why |
|---|---|---|
| UI-heavy play: cards, quizzes, text, board states, menus | **DOM + CSS** | Free layout, accessibility, CSS transitions; no render loop to babysit |
| Free-moving entities: sprites, collision, particles, projectiles | **Canvas 2D** | Direct draw loop, full control, zero dependencies, single file |
| Tilemaps, levels, platformer physics, spritesheets, animations | **Phaser 3** | Built-in physics/arcade/tilemap/scene system; saves days of engine plumbing |
| Real 3D or pseudo-3D perspective | **Three.js** | WebGL; only when 3D *is* the fun (spatial puzzles, racers, FPS) |

## Decision order

1. Can the core loop be expressed with DOM state changes (deal a card, reveal a square, toggle a tile)? → DOM + CSS.
2. Do entities move freely every frame (bounce, fly, orbit, spawn/despawn)? → Canvas 2D.
3. Do you need tiles/levels/arcade physics as *data* rather than code? → Phaser 3.
4. Is depth/perspective essential to the mechanic? → Three.js.

## Constraints worth defaulting to

- **Single file (`index.html`)** until a mechanic genuinely needs modules; inlined JS/CSS keeps prototypes shareable and reviewable.
- **No external assets** — shapes, particles, and synthesized audio (`WebAudio`) carry a prototype; art drops in later.
- **60fps budget** — animate only `transform`/`opacity` in DOM; in Canvas, keep per-frame allocation at zero (reuse objects, pool particles).
- **Input**: keyboard + pointer covers prototypes; gamepad only if the mechanic demands it.

## Running

- `python3 -m http.server 8000` → `http://localhost:8000` (loopback navigates in `browser-control` without approval).
- `npx serve .` as an alternative.
- Plain Canvas/DOM single files also open directly via `file://`.

## Migration path (when the prototype wins)

DOM+CSS → Canvas 2D: move per-frame visual state into a `requestAnimationFrame` loop.
Canvas 2D → Phaser 3: extract entity logic into Phaser scenes/sprites; physics becomes config.
Any → Three.js: only rebuild when the 2D prototype can't fake the mechanic (e.g. real depth perception). Rewrite, don't wrap.
