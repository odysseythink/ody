# Design Mode

Design Mode is a **brainstorming / specification-exploration** collaboration mode in Ody.  It is the sibling of [Plan Mode](plan_mode.md): Plan Mode produces an implementation plan (the *how*), while Design Mode produces an approved design (the *what* and *why*).

## When to use it

Use `/design` when you need to think through architecture, contracts, data models, algorithms, and failure modes *before* deciding how to implement something.  Once the design is approved, switch to `/plan` to turn it into an executable implementation plan.

## How it works

1. Enter the mode with the `/design` slash command (no approval required).
2. Ody injects a structured Design Mode workflow prompt that guides the session through:
   - a strictness audit gate (Basic / Standard / Deep);
   - upstream inventory / prior art (for ports and mirrors);
   - internal reuse scan;
   - seven-dimension clarification (Scope, Data & State, Integration, Error & Degradation, Security, Observability, Operations);
   - proposal of 2–3 genuinely different approaches with trade-offs;
   - segmented presentation and user approval;
   - writing the design file to `.ody-code/designs/`;
   - adversarial self-review and a C1–C8 exit checklist.
3. While in Design Mode the workspace is **read-only**: writes are only allowed to the current design file and its `<stem>/` split parts.  Any other file modifications are rejected by the same safety gate used by Plan Mode.
4. Leaving Design Mode for Plan Mode triggers a handoff reminder: "Design saved to `<path>` — create a concrete implementation plan based on the approved design."

## Configuration

Design Mode intentionally shares the `PlanModeConfigToml` section in `~/.ody-code/config.toml`:

```toml
[plan_mode]
enforcement = "Strict"        # Strict / Ask / Advisory
split_threshold = 8
split_plan_compaction_ratio = 0.5
```

The same `enforcement` level controls both Plan and Design write gates, and the same `split_threshold` is rendered into both mode templates.

## File layout

Design artifacts live under `<project>/.ody-code/designs/` with the filename convention `YYYY-MM-DD-<topic>.md`.  Large designs are split into an index file plus part files under `<design-stem>/<subsystem>.md`.

## Differences from the upstream ody-code Design Mode

- There are no `EnterDesignMode` / `ExitDesignMode` tools; Design Mode is a collaboration mode, entered via `/design` and exited by switching to `/plan` or `/default`.
- There is no `ShowDesignMockup` tool; visual ideas are described with ASCII / structured text inside the design file.
- Structured questions use the standard `request_user_input` tool, not a dedicated `AskUserQuestion` tool.
- Periodic re-injection uses the `developer_instructions` mechanism (the same as Plan Mode), rather than ody-code's `full/sparse/reentry` injector states.
