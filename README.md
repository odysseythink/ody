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

Most coding assistants treat design as a prefix to implementation—an offhand "let's think step by step." Ody treats design as a first-class collaboration phase with its own workspace, rules, and handoff contract.

Enter `/design` to switch the session into a structured design studio. While you are in Design mode, the rest of the workspace is read-only: the model can edit only the current design document under `.ody-code/designs/` and its optional split parts. This prevents "implementation drift"—the common failure mode where an agent starts writing code before the design is actually finished.

Every design is written against an adversarial completeness gate (C1–C8) that enforces eight required sections:

1. **Scope / In-Out** — what is in scope and, just as importantly, what is out.
2. **Architecture & Design** — the conceptual approach, not a TODO list.
3. **Data Models** — the state, shapes, and lifecycles the design depends on.
4. **Algorithms & Implementation Notes** — the core logic, not framework plumbing.
5. **Error Handling** — failure modes, degradation, and recovery.
6. **Self-Review** — an adversarial audit against the rubric, not a polite summary.
7. **User Approval** — explicit sign-off before the design can be handed off.
8. **Reuse Analysis** — what existing components can be reused instead of rebuilt.

When you leave Design mode, the C1–C8 gate runs against the saved design artifact. If the document is missing sections or too short, the handoff is vetoed or flagged depending on `enforcement`:

- `Strict` — an incomplete design blocks the switch to Plan mode.
- `Ask` — the handoff proceeds with a warning that the user must acknowledge.
- `Advisory` — a note is appended but the switch is allowed.

Design mode also shares `PlanModeConfigToml` settings with Plan mode (`split_threshold`, `split_plan_compaction_ratio`). Large designs can be split into parts under the design's `<stem>/` directory, so a multi-subsystem design stays navigable rather than becoming a single wall of text.

Before the design starts, you choose an audit level—Basic, Standard, or Deep—that tells the model how aggressively to verify assumptions against the repo and upstream sources. This is a host decision, not a fuzzy prompt: the model receives the level in its instructions and is told not to ask you again.

The end result is a design artifact that can be handed off to Plan mode as an approved blueprint. The Plan mode prompt is seeded with the design file path and a reminder to derive concrete implementation steps from the approved design, not from an improvised reinterpretation.

This makes Ody's Design mode closer to a design-review tool than a chat wrapper: the model is held to a written, auditable, user-approved contract before it is allowed to plan or write code.

## Adversarial Design Review

When you finalize a design in Design mode, Ody runs an adversarial review—a multi-turn debate (v1, with opt-in enhancements) where:

- A **single-shot critic** produces initial findings across correctness, assumptions, failure modes, and simpler alternatives.
- (Optional) An **Advocate↔Skeptic debate** seeds the critic's findings and surfaces gaps the critic missed. Each turn is a structured argument, culminating in a Judge that synthesizes findings across both axes.
- (Optional, v1.6) A **usability-lens Skeptic turn** attacks from the user-facing angle (mode confusion, workflow friction, missing feedback, accessibility) — orthogonal to correctness findings.

The review is configured in `config.toml` under `[design_review]`:

```toml
[design_review]
enable = true                           # Enable adversarial review on design finalize
review_model = "gpt-4o"                 # Model for single-shot critic (required if enable=true)

[design_review.debate]
enable = false                          # Enable Advocate↔Skeptic↔Judge debate (opt-in)
rounds = 1                              # Debate rounds (Advocate/Skeptic pairs); 1 = A→S→J
# Debate model seat overrides (optional; fall back to review_model):
advocate_model = "gpt-4o"
skeptic_model = "gpt-4o"
judge_model = "gpt-4o"
contest_critic = false                 # Judge may refute critic findings → marked Contested (opt-in)
usability_lens = "off"                 # v1.6: append usability-lens Skeptic turn (opt-in)
```

### Usability Lens Configuration (v1.6)

The `usability_lens` setting controls whether to append a usability-focused Skeptic turn to the debate. Set it in `[design_review.debate]`:

- **`"off"` (default)**: No usability lens. Review runs standard correctness debate only.
- **`"on"`**: Always append a forced usability-lens turn. Cost: +1 model call per finalize. Use this if you want every design reviewed for user-facing defects (mode confusion, missing feedback, accessibility, workflow friction).
- **`"ask"` (v1.6b)**: Show a one-question interactive prompt at finalize time. The system classifies the design as "user-facing" or "internal" and recommends whether to run the usability pass. You can accept the recommendation or override it. Your choice is cached across revise rounds so you're not re-prompted for the same design. Cost: +1 cheap classifier call + 1 usability turn (only when you say yes).

#### Example configurations

```toml
# Strict review: both debate and usability lens enabled
[design_review.debate]
enable = true
usability_lens = "on"

# Lean review: debate only, no usability lens (correctness-focused)
[design_review.debate]
enable = true
usability_lens = "off"

# Smart review: debate + optional usability (user confirms per design)
[design_review.debate]
enable = true
usability_lens = "ask"
```

The usability lens is most valuable for designs that have a direct user-facing surface (TUI commands, interactive flows, on-screen text/layout). It has minimal value for internal refactors, data migrations, or pure API changes, so the `"ask"` mode lets you control when it runs.


