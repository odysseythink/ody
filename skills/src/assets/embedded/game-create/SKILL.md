---
name: game-create
description: Multi-phase flow skill that takes a game concept from a one-line theme to a runnable prototype — designs the GDD, fans out implementation across game mechanics, then reviews in parallel. Trigger with /game-create followed by the theme.
type: flow
---

# Game Create (flow sample)

This is the reference sample for a `type: flow` skill: the executable plan
lives in `flow.yaml` next to this file, and the flow runtime (not the model)
drives the phases.

## How it runs

1. **design** — one agent turns the theme (`args.text`) into a GDD whose
   output binds to `gdd`.
2. **implement** — a pipeline fans out one agent per entry of `gdd.mechanics`,
   interpolating `${item.name}` and `${gdd.constraints}` into each prompt.
3. **verify** — a parallel group reviews the implementations and runs the
   tests concurrently.

When the run finishes, the outputs (`gdd`, `implementations`, `review`) are
recorded back into the conversation as a flow result item.

## Trigger

```
/game-create <one-line game theme>
```
