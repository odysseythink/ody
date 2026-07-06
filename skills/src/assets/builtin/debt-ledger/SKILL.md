---
type: inline
name: debt-ledger
description: >-
  Scan the codebase for `// ody:` / `# ody:` simplification-debt markers
  and render a Chinese-first ledger report grouped by file with rot-risk
  warnings. Use after every simplification session to harvest markers,
  and before code review to surface pending simplifications.
arguments: path
---

## Purpose

This skill teaches you how and when to call the `HarvestOdyMarkers` tool
to scan for `ody:` simplification-debt markers in the codebase.

## Marker format

A simplification-debt marker is a line comment in source code:

```
// ody: <天花板>, <升级触发条件>
# ody: <天花板>, <升级触发条件>
```

- **天花板**: the ceiling of the current simplified approach (what you chose NOT to build).
- **升级触发条件**: the concrete condition under which the ceiling should be raised (e.g. "吞吐 > 100 rps", "需要 schema 校验时").

Markers without an upgrade trigger (no comma, or empty after comma) are
flagged as **⚠️ rot** — incomplete debt that should be resolved.

## When to call HarvestOdyMarkers

Invoke the `HarvestOdyMarkers` tool when:
- The user asks for "债务台账", "ody debt", "列出 ody 标记", "harvest ody markers", or similar.
- After a simplification session, to audit markers the agent left behind.
- Before code review, to surface pending simplifications.

Pass an optional `path` argument to scan a specific directory or file.

## Output format

The tool returns a markdown report:
- Grouped by file (sorted alphabetically).
- Each marker on its own line: `<file>:<line> — <ceiling>。天花板：<ceiling>。升级：<upgrade>。`
- Rot markers annotated with `⚠️ rot` and `升级：（未指定）`.
- Footer summary: `**汇总**：N 个标记，M 个 rot 风险。`

## After harvesting

1. Present the ledger to the user.
2. If rot markers exist, note which files have incomplete debt.
3. The user may ask you to resolve specific markers.
