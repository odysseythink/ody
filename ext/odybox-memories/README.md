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

Skeleton. The gate, the storage root, and the mounting point are in place and
contract-tested. The assistant-facing tool set plugs into
`AssistantMemoryExtension`'s `ToolContributor` impl (see `src/extension.rs`).

## Mounting point

`app-server/src/extensions.rs` calls `ody_odybox_memories::install(&mut builder)`
next to `ody_memories_extension::install(...)`. Installing is unconditional and
safe: every contributor is a no-op unless the thread is an odyBox thread.
