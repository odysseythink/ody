# odyBox memories (`ody-odybox-memories`)

Assistant-facing memory for odyBox. Independent of, and layered on top of, the
Ody memory pipeline in `memories/` and `ext/memories/`.

## Isolation contract

Three guarantees keep this crate from affecting the existing Ody memory system:

1. **Product gate.** Contributors activate only when the session source resolves
   to `Product::OdyBox`. Plain `ody` CLI, `vscode`, `exec`, `mcp`, `atlas`, and
   internal / sub-agent threads are untouched.
2. **No shared thread state.** When the gate is closed nothing enters the thread
   store, so no prompt or tool contributor can observe assistant-memory state.
3. **Separate storage root.** Assistant memory lives under
   `$ODY_HOME/odybox_memories`, never under the Ody memory workspace
   `$ODY_HOME/memories` owned by `ody-memories-write`.

All three are locked by tests in `src/contract_tests.rs`.

## Why a runtime gate instead of a Cargo feature

Product separation in this workspace is a *runtime* decision expressed through
`Product` / `--session-source` — the same mechanism `policy.products` already
uses to isolate skills and plugins. Cargo features in this workspace cut
*capabilities* (`v8`, `windows-sandbox`, `bundled-builtin-mcp`), and they are
additive by design: Cargo takes the union of all requested features for a
package, and mutually exclusive features are not supported. "Give this product
this module and not that one" is a product question, so it stays runtime-side.

Two practical reasons this matters here:

- `odyBox` does not compile this workspace. `.erb/scripts/stage-ody-runtime.cjs`
  copies `ody-rs/target/release/ody-app-server[.exe]` into the installer, so
  `ody` and `odyBox` ship the *same* binary. A build-time switch would force two
  build products and break that delivery model.
- A Cargo feature would not protect the existing Ody memory path anyway. That
  risk comes from shared code being *edited*, not from code being *linked in*.
  The contract tests are what actually hold that line.

## Status

Both paths are in place:

- **Read** — `read` / `search` / `add_note` tools in the `odybox_memories`
  namespace, scoped to the assistant memory root.
- **Write** — a filesystem-only extraction pipeline: list rollout files filtered
  by `SessionSource` *by value* (`scan.rs`), skip sessions already recorded
  (`ledger.rs`), run one model call per session (`extractor_model.rs`), and write
  the record into the assistant memory root (`pipeline.rs`).

Not yet implemented: cross-session consolidation (the equivalent of Ody's
Phase 2). Session records are written independently and never merged.

### When it runs

The pipeline is triggered from thread lifecycle callbacks on odyBox threads:

- **New thread** (`on_thread_start`) — the main path. It carries `Config` and
  `SessionSource`, so the product gate and the storage root are derived directly.
- **Resumed thread** (`on_thread_resume`) — reuses the context captured at start
  in the same process. `ThreadResumeInput` carries neither `Config` nor
  `SessionSource`, so a resume **after a restart** finds an empty thread store and
  does nothing.

That gap is not a correctness hole: every pass scans *all* unprocessed sessions,
and the ledger marks what has been handled. Content from a resume-only session is
therefore still extracted on the next new thread, just later. The resume path
only improves latency.

### Required configuration on third-party gateways

`model-provider` hardcodes the extraction model default:

```rust
pub const DEFAULT_MEMORY_EXTRACTION_PREFERRED_MODEL: &str = "glm-4.5";
pub const DEFAULT_MEMORY_CONSOLIDATION_PREFERRED_MODEL: &str = "k3";
```

A custom OpenAI-compatible gateway almost certainly does not serve those names,
and extraction then fails silently. Set the model explicitly in
`$ODY_HOME/config.toml`:

```toml
[memories]
extract_model = "<a model your provider actually serves>"
```

Two field notes from bringing this up on a real gateway:

- The model must be routable **under the same API key** the runtime uses. Some
  gateways partition models by key group, so a model visible in the UI may still
  return `model_not_found`.
- When that happens the runtime reports *"Selected model is at capacity. Please
  try a different model."*, which is misleading — the gateway actually answered
  `model_not_found`. Verify providers with a direct HTTP call before trusting
  the runtime's error text.

### What is deliberately absent

- No state DB and no shared job table. Discovery reads rollout files and the
  ledger is the filesystem, so extraction never touches `state` or the Ody
  memory tables. The cost is a single-process assumption (odyBox ships a
  single-instance guard), with no lease to coordinate two racing processes.
- No Phase 1 / Phase 2 split. Sessions are processed independently and at most
  once each; there is no consolidation agent.

## Mounting point

`app-server/src/extensions.rs` calls
`ody_odybox_memories::install(&mut builder, odybox_thread_manager)` next to
`ody_memories_extension::install(...)`. Installing is unconditional and safe:
every contributor is a no-op unless the thread is an odyBox thread. The write
pipeline is triggered from `on_thread_start` on odyBox threads only, as a
best-effort background task — it can never affect thread startup.
