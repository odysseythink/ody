# Ody CLI

[Ody CLI Documentation](https://developers.odysseythink.com/ody/cli)

Ody CLI is an open-source, terminal-based coding assistant. It provides an agentic chat interface, multi-provider model support, optional V8 code-mode execution, and a pluggable skill system—designed to help you write, understand, and refactor code from the command line.

## Highlights

- **Agentic TUI**: Rich terminal chat interface with streaming responses, history, and real-time reasoning visibility in Design and Plan modes.
- **Multi-provider support**: Built-in support for OpenAI-compatible providers, including Kimi, DeepSeek, and GLM.
- **Design / Plan modes**: Structured collaboration modes for design exploration and step-by-step implementation planning.
- **Optional V8 code mode**: Run JavaScript snippets in a sandboxed V8 runtime by enabling the `v8` feature.
- **Pluggable skill system**: Extend capabilities through local or remote skills with a unified integration surface.
- **Sandboxed execution**: Hardened process and filesystem isolation for safe tool and command execution.
- **Modular Rust workspace**: Fast, reliable, and organized as a Cargo workspace.

## Multi-provider support

In addition to the default provider, Ody ships with built-in support for Kimi, DeepSeek, and GLM (all OpenAI-compatible Chat Completions endpoints). See [docs/multi_provider.md](docs/multi_provider.md) for configuration details and provider-specific notes.

## Building

This repository is a Cargo workspace.

### Common build commands

```bash
# Build the entire workspace (debug)
cargo build

# Build only the main CLI binary (debug)
cargo build -p ody-cli --features v8

# Release build of the main CLI binary
cargo build --release -p ody-cli
```

### Output locations

The main binary is named `ody` and is defined in `cli/Cargo.toml` by `[[bin]] name = "ody"`:

```bash
# Debug binary
./target/debug/ody

# Release binary
./target/release/ody
```

### Testing

```bash
# Entire workspace
cargo test

# Single crate, e.g. ody-core
cargo test -p ody-core
```

### Local installation

```bash
cargo install --path cli
```

This compiles `ody` and installs it to `~/.cargo/bin/`.

### Release packaging

There is no `cargo dist` distribution setup in this repository. The practical approach is:

```bash
cargo build --release -p ody-cli
```

Then package the resulting `target/release/ody` binary.

## Design Mode

Ody provides a `/design` collaboration mode for structured design exploration. While in Design mode, the session is read-only except for the current design file under `.ody-code/designs/` and its optional split parts.

Design mode shares `PlanModeConfigToml` settings (`enforcement`, `split_threshold`, `split_plan_compaction_ratio`) with Plan mode and must pass the C1–C8 completeness gate. With `enforcement = "Strict"`, an incomplete design blocks switching to Plan mode.

## Status bar

By default, the status line is disabled. The footer line you normally see on the right is the environment context indicator (`% context left`).

To enable the status line, use one of these configuration options:

- Run the `/statusline` slash command and select items interactively.
- Add the following to `~/.ody/config.toml`:

```toml
[tui]
status_line = ["model", "context-remaining", "used-tokens"]
```

Available context-related items:

| Item | Display |
|------|---------|
| `context-remaining` | Context X% left |
| `context-used` | Context X% used |
| `context-window-size` | N window |
| `used-tokens` | N used |

Using `["model", "context-remaining", "used-tokens"]` gives a status line similar to the `ody-code` context display.

Enabling the status line replaces the existing single footer line rather than adding a new one. When `status_line_active` is true, the left side shows the configured items and the right side shows the collaboration mode indicator. The default context footer (`% context left`) is no longer rendered because it only appears when the status line is inactive.

Default (status line off):

```
⇧Tab to cycle ...                                  75% context left
```

Enabled (status line on):

```
model · Context 75% left · 12.3k used              [Plan]
```

**Note:** `context-remaining`, `context-used`, and `context-window-size` require the provider configuration to include `max_context_tokens`. If Kimi, DeepSeek, or GLM providers are missing this value, these items return `None` and are omitted from the status line. To see the percentage, first set the context window size for those providers.

In short: enable the status line with `/statusline` or `tui.status_line`; it replaces the existing footer line instead of duplicating it, but context percentages require the corresponding provider to expose a context window size.
