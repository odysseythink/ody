# TUI `/preferences` slash command for mode-specific configuration editing

## Scope In / Scope Out

**Scope In** [C:USER]
- Add a new built-in slash command `/preferences` to the TUI.
- The command opens a modal overlay/popup whose contents depend on the active collaboration mode:
  - **Design mode**: an editor for `design_review.*` configuration keys (`enable`, `review_model`, `debate.enable`, `debate.rounds`, `debate.advocate_model`, `debate.skeptic_model`, `debate.judge_model`, `debate.contest_critic`, `debate.usability_lens`).
  - **Default mode**: a placeholder view stating that no mode-specific preferences are available yet.
  - **Plan mode**: a placeholder view stating that no mode-specific preferences are available yet.
- Edits in Design mode are persisted immediately to the user's `config.toml` via the existing app-server `ConfigBatchWrite` path, with rollback on failure.
- Input is validated client-side (e.g., `rounds` clamped to 1..=3, `usability_lens` restricted to enum values, booleans via toggles).
- The command is disabled while an agent task is running or a side conversation is active.

**Scope Out** [C:USER]
- No editing of general/global preferences in Default mode for this iteration.
- No editing of Plan-mode-specific preferences in Plan mode for this iteration (placeholder only).
- No direct TOML text editing; the UI uses typed controls.
- No support for editing `design_review` while a design finalization is in progress (the overlay is modal and blocks the design flow, but it does not retroactively affect the active review).

Altitude: tactical (altitude gate cleared — no recurrence or structural signals found). [C:INFERRED]

## Clarifications

| Dimension | Confirmed choice | Source |
|---|---|---|
| Scope | `/preferences` is globally available, shown as a modal overlay; Default/Plan modes show a placeholder. | [C:USER] |
| Data & State | Instant per-field write to user-level `config.toml` via `ConfigBatchWrite`; rollback UI on failure. | [C:USER] |
| Integration | Edit the raw TOML layer (`design_review.*`); config loader re-resolves effective values after reload. | [C:USER] |
| Error & Degradation | Client-side immediate validation; invalid values cannot be submitted; write failures keep the overlay open with an inline error. | [C:USER] |
| Security | Command disabled during agent tasks and side conversations. | [C:USER] |
| Observability | No new analytics events; existing tracing and `add_error_message` style error reporting are sufficient. | [C:INFERRED] |
| Operations | Changes take effect on the next config reload; no restart required; no migration needed because schema already exists. | [C:INFERRED] |
| Algorithms | Text fields (model names) write on blur; boolean/enum fields write immediately. | [C:USER] |

## Architecture Overview

Use the existing **BottomPaneView overlay + AppEvent asynchronous persistence** pattern, consistent with `MemoriesSettingsView` and `ExperimentalFeaturesView`. [C:USER]

The flow is: slash command → `ChatWidget::dispatch_command` → `ChatWidget::open_preferences_popup` → `PreferencesView` (new `BottomPaneView`) → `AppEvent::PersistDesignReviewPreferences` → `App` handler → `config_update::write_config_batch` → `AppEvent::SyncDesignReviewPreferences` back to the view.

For a full component diagram, data models, algorithms, and error handling, see the parts below.

## Data Models

The new TUI types are `PreferencesView`, `PreferencesContent`, `DesignReviewEditState`, and `PreferencesFocus`. The new `AppEvent` variants are `PersistDesignReviewPreferences { edits: Vec<ConfigEdit> }` and `SyncDesignReviewPreferences { design_review: DesignReviewToml, error: Option<String> }`. The config edit builder is `config_update::build_design_review_edits(state: &DesignReviewEditState) -> Vec<ConfigEdit>`. See `data_models.md` for full definitions and type mappings. [C:INFERRED]

## Algorithms

The main algorithms are: `ChatWidget::open_preferences_popup()` selects the mode and content; `PreferencesView` renders the form/placeholder and routes keys; field changes emit `PersistDesignReviewPreferences`; `App::update_design_review_preferences` writes via `ConfigBatchWrite` and emits `SyncDesignReviewPreferences`; the view applies the sync to restore truth. See `algorithms.md` for pseudocode. [C:INFERRED]

## Error Handling / Degradation

Client-side validation blocks invalid `rounds` and enum values. Server-side write failures keep the popup open and reset the form via the sync event. Overridden writes are handled like existing memory settings. The command is unavailable during tasks and side conversations. See `errors_and_review.md` for the full table. [C:USER]

## Self-Review

Most expensive if wrong: (1) whether `ConfigBatchWrite` reloads config in time for the next design finalize; (2) optimistic UI sync on failure; (3) adding a generic `ConfigEdit`-carrying AppEvent. The design mitigates these with a unified sync event and by reusing existing config reload paths. Security, test, operations, and integration lenses were swept. See `errors_and_review.md` for the full self-review. [C:INFERRED]

## User Approval

This design is presented for approval. All sections and parts are complete. [C:USER]

## Reuse Analysis

| Component | Reuse candidate | Source |
|---|---|---|
| Slash command enum + registration | `SlashCommand` in `tui/src/slash_command.rs` and `built_in_slash_commands()` | [C:UPSTREAM] |
| Slash command gating/filtering | `BuiltinCommandFlags` / `builtins_for_input()` in `tui/src/bottom_pane/slash_commands.rs` | [C:UPSTREAM] |
| Slash dispatch routing | `ChatWidget::dispatch_command()` in `tui/src/chatwidget/slash_dispatch.rs` | [C:UPSTREAM] |
| Overlay view framework | `BottomPaneView` trait and `BottomPane::show_view()` in `tui/src/bottom_pane/` | [C:UPSTREAM] |
| List/form settings UI pattern | `MemoriesSettingsView` in `tui/src/bottom_pane/memories_settings_view.rs` | [C:UPSTREAM] |
| Config edit helpers | `config_update::replace_config_value()` / `clear_config_value()` / `write_config_batch()` in `tui/src/config_update.rs` | [C:UPSTREAM] |
| App event persistence pattern | `AppEvent::UpdateMemorySettings`, `PersistPersonalitySelection`, etc. in `tui/src/app_event.rs` and `app/event_dispatch.rs` | [C:UPSTREAM] |
| Config types | `DesignReviewToml`, `DesignReviewDebateToml`, `UsabilityLensToml` in `config/src/config_toml.rs` | [C:UPSTREAM] |
| In-memory config | `core::Config::design_review_*` fields in `core/src/config/mod.rs` | [C:UPSTREAM] |
| Collaboration mode detection | `ChatWidget::active_collaboration_mask` and `current_collaboration_mode` in `tui/src/chatwidget.rs` | [C:UPSTREAM] |
| Mode kind enum | `ody_protocol::config_types::ModeKind` in `protocol/src/config_types.rs` | [C:UPSTREAM] |

Greenfield: the per-mode form layout and the `design_review` edit builder are new, but they sit on top of the existing overlay and persistence layers. [C:INFERRED]

## Parts

| # | File | Scope | Status |
|---|---|---|---|
| 1 | `2026-07-29-tui_preferences_slash_command_for_mode_specific/architecture.md` | Components, data flow, mode-specific rendering, persistence/rollback, extension points | done |
| 2 | `2026-07-29-tui_preferences_slash_command_for_mode_specific/data_models.md` | TUI types, `AppEvent` variants, config edit builder | done |
| 3 | `2026-07-29-tui_preferences_slash_command_for_mode_specific/algorithms.md` | Open popup, render, key handling, field change persistence, app-layer persistence, state sync | done |
| 4 | `2026-07-29-tui_preferences_slash_command_for_mode_specific/errors_and_review.md` | Error/degradation table, self-review, assumptions & unverified items | done |

