# Errors and Self-Review — TUI `/preferences` slash command

## Error Handling / Degradation

| Scenario | Behavior | Source |
|---|---|---|
| `/preferences` typed while task running | Error message in chat: "`'/preferences' is unavailable while a task is running.`" | [C:USER] |
| `/preferences` typed in side conversation | Error message in chat: "`'/preferences' is unavailable in side conversations.`" | [C:USER] |
| Client-side validation fails (e.g., `rounds` not 1..=3) | Field remains invalid; inline error banner in popup; no persistence event sent. | [C:USER] |
| App server unreachable or `ConfigBatchWrite` fails | Popup stays open; error banner set; `SyncDesignReviewPreferences` resets the form to the last known persisted state. | [C:USER] |
| Write succeeds but config is overridden elsewhere (`OkOverridden`) | App reads effective config, syncs the form, and shows a warning that the value was overridden. | [C:UPSTREAM] |
| Config reload fails after write | The write may have succeeded on disk; app logs a warning and shows a chat error but leaves the popup open. | [C:INFERRED] |
| User closes popup while a write is pending | The event is already in the App queue; the view is dropped, but the write completes normally. No UI feedback is needed because the popup is gone. | [C:INFERRED] |
| Unknown/unsupported mode kind (e.g., `Execute`) | Fall back to the Default placeholder; no crash. | [C:INFERRED] |

## Self-Review

### Most expensive decisions if wrong

1. **Using `ConfigBatchWrite` with `reload_user_config = true` for immediate effect.** If this does not actually reload the user config or the design review resolution chain is not re-evaluated, the UI would claim success but the next design finalize would still use stale values. This is mitigated by the existing `SyncDesignReviewPreferences` sync event and by verifying the config reload path during implementation. [C:INFERRED]
2. **Optimistic UI updates with server-side sync on failure.** If the sync event is dropped or the view fails to apply it, the user could see a value that does not match the persisted config. The builder sends all fields, so the synced state is always complete and unambiguous. [C:INFERRED]
3. **Extending `AppEvent` with a new `ConfigEdit`-carrying variant.** If `ConfigEdit` is too large or too generic for the AppEvent enum, it could make event dispatch harder to reason about. The alternative (a typed struct for every field) was rejected because it makes future mode extensions verbose. [C:INFERRED]

### Lenses

- **Security**: No new secrets, no network calls beyond the existing app-server `ConfigBatchWrite`, no new trust boundaries. The command is gated by the same task/side-conversation rules as other non-task-safe commands. [C:USER]
- **Test/Verification**: Add unit tests for `config_update::build_design_review_edits` covering empty-string clearing, `rounds` boundaries, and boolean/enum serialization. Add a test in `chatwidget/tests/slash_commands.rs` that `/preferences` resolves and is gated during tasks. Add a test in `chatwidget/tests/popups_and_settings.rs` (or a new test file) that the view renders the 9 design fields and emits the correct `AppEvent` on toggle. [C:INFERRED]
- **Operations**: No new config schema, no migration, no CLI flag changes. The feature is purely additive. [C:INFERRED]
- **Integration**: The feature depends on `ConfigBatchWrite` and the app-server config reload path. The design reuses both verbatim. [C:UPSTREAM]

### Fixes applied during review

- Initially considered a revert-only event. Replaced with a unified `SyncDesignReviewPreferences` event that handles success, failure, and overridden writes in one path, reducing event-surface area and keeping the UI consistent with the config layer. [C:INFERRED]
- Added explicit `Default` fallback for non-TUI-visible mode kinds so the popup never panics if invoked in an unexpected mode. [C:INFERRED]
- Added a scope-out line clarifying that the popup does not retroactively affect an in-progress design finalization. [C:INFERRED]

## Assumptions & Unverified Items

| # | Assumption | Confidence | Impact if wrong | How to verify |
|---|---|---|---|---|
| 1 | `ConfigBatchWrite` with `reload_user_config = true` is sufficient to make the next Design-mode finalization pick up new `design_review` values without a process restart. | medium | Design-mode review would continue using stale values; users would be confused. | Check `core/src/config/mod.rs` reload path and `ConfigBatchWrite` handler in app-server. |
| 2 | No analytics/telemetry events are required for this feature. | low | Product team may lack visibility into usage of the new command. | Confirm with product owner or existing analytics schema. |
| 3 | Placeholder views for Default and Plan modes can reuse the existing centered overlay frame without new navigation patterns. | medium | If the overlay framework differs, the placeholder implementation may need redesign. | Inspect existing popup/overlay render paths (e.g., `bottom_pane/command_popup.rs`, `chatwidget/settings_popups.rs`). |
| 4 | Text fields writing on blur (rather than debounce or per-keystroke) is acceptable UX for model-name fields. | medium | Users may expect live validation or faster feedback; may need adjustment. | Usability review during implementation. |
| 5 | The `design_review` schema in `config/src/config_toml.rs` will not change incompatibly before this feature ships. | high | Builder logic would need regeneration. | Schema is already stable in the current codebase. |
| 6 | A unified `SyncDesignReviewPreferences` event from App to view is sufficient for rollback, success confirmation, and overridden-write handling. | medium | If the event is dropped or the view cannot apply it, the UI may diverge from disk. | Review event dispatch and view lifecycle tests during implementation. |
| 7 | The `BottomPane` view stack can host a form with mixed text/toggle/enum inputs using the existing `ListKeymap` and `handle_key_event` path. | medium | If not, a custom keymap or input widget may be needed. | Inspect `bottom_pane_view.rs` and `memories_settings_view.rs` key handling. |
