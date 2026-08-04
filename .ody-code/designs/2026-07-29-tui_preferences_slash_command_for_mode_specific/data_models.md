# Data Models — TUI `/preferences` slash command

## TUI types added in `bottom_pane/preferences_view.rs`

```rust
pub(crate) struct PreferencesView {
    content: PreferencesContent,
    mode: ModeKind,
    app_event_tx: AppEventSender,
    keymap: ListKeymap,
    error_message: Option<String>,
}

pub(crate) enum PreferencesContent {
    DefaultPlaceholder,
    PlanPlaceholder,
    DesignEditor(DesignReviewEditState),
}

pub(crate) struct DesignReviewEditState {
    enable: bool,
    review_model: String,               // empty string means None / cleared
    debate_enable: bool,
    debate_rounds: String,              // validated to 1..=3, empty means None
    debate_advocate_model: String,      // empty means None
    debate_skeptic_model: String,       // empty means None
    debate_judge_model: String,         // empty means None
    debate_contest_critic: bool,
    debate_usability_lens: UsabilityLensToml,
}

pub(crate) enum PreferencesFocus {
    Enable,
    ReviewModel,
    DebateEnable,
    DebateRounds,
    DebateAdvocateModel,
    DebateSkepticModel,
    DebateJudgeModel,
    DebateContestCritic,
    DebateUsabilityLens,
}
```

`DesignReviewEditState` maps one-to-one to the `config/src/config_toml.rs` fields. String fields use `String` (not `Option<String>`) so text input is easy to render; an empty string serializes to `clear_config_value(...)`. The enum field `debate_usability_lens` is stored directly as `UsabilityLensToml` because it is edited through a cycling selector, not free text. [C:INFERRED]

## `AppEvent` variants added in `tui/src/app_event.rs`

```rust
/// Request persistence of one or more `design_review` config edits.
PersistDesignReviewPreferences {
    edits: Vec<ConfigEdit>,
},

/// Replace the in-popup `design_review` state with the current persisted truth.
/// Sent after any successful, failed, or overridden write so the UI never diverges
/// from the config layer.
SyncDesignReviewPreferences {
    design_review: DesignReviewToml,
    error: Option<String>,
},
```

`ConfigEdit` is imported from `ody_app_server_protocol`. `DesignReviewToml` is imported from `ody_config::config_toml`. The `error` field is `None` on success and overridden-write warnings, and `Some` on hard failures. [C:INFERRED]

## Config edit builder in `tui/src/config_update.rs`

```rust
pub(crate) fn build_design_review_edits(
    state: &DesignReviewEditState,
) -> Vec<ConfigEdit> {
    vec![
        replace_config_value(
            "design_review.enable",
            serde_json::json!(state.enable),
        ),
        // Empty string => clear the key so it falls back through the legacy/model resolution chain.
        if state.review_model.is_empty() {
            clear_config_value("design_review.review_model")
        } else {
            replace_config_value(
                "design_review.review_model",
                serde_json::json!(state.review_model),
            )
        },
        replace_config_value(
            "design_review.debate.enable",
            serde_json::json!(state.debate_enable),
        ),
        // rounds: empty => clear (None); otherwise validated u8.
        if state.debate_rounds.is_empty() {
            clear_config_value("design_review.debate.rounds")
        } else {
            replace_config_value(
                "design_review.debate.rounds",
                serde_json::json!(state.debate_rounds.parse::<u8>().unwrap()),
            )
        },
        // model fields: empty => clear.
        if state.debate_advocate_model.is_empty() {
            clear_config_value("design_review.debate.advocate_model")
        } else {
            replace_config_value(
                "design_review.debate.advocate_model",
                serde_json::json!(state.debate_advocate_model),
            )
        },
        if state.debate_skeptic_model.is_empty() {
            clear_config_value("design_review.debate.skeptic_model")
        } else {
            replace_config_value(
                "design_review.debate.skeptic_model",
                serde_json::json!(state.debate_skeptic_model),
            )
        },
        if state.debate_judge_model.is_empty() {
            clear_config_value("design_review.debate.judge_model")
        } else {
            replace_config_value(
                "design_review.debate.judge_model",
                serde_json::json!(state.debate_judge_model),
            )
        },
        replace_config_value(
            "design_review.debate.contest_critic",
            serde_json::json!(state.debate_contest_critic),
        ),
        replace_config_value(
            "design_review.debate.usability_lens",
            serde_json::json!(state.debate_usability_lens),
        ),
    ]
}
```

The builder sends *all* fields on every change. Because `ConfigBatchWrite` uses `MergeStrategy::Replace` per key, unchanged keys retain their value; there is no risk of clearing unrelated keys. Clearing an empty model field preserves the existing fallback chain (`design_review_model` → `review_model`). [C:INFERRED]

## Mapping from `core::Config` to `DesignReviewEditState`

```rust
impl DesignReviewEditState {
    fn from_config(config: &Config) -> Self {
        let dr = config.design_review.as_ref().cloned().unwrap_or_default();
        let debate = dr.debate.unwrap_or_default();
        Self {
            enable: dr.enable,
            review_model: dr.review_model.unwrap_or_default(),
            debate_enable: debate.enable,
            debate_rounds: debate.rounds.map(|r| r.to_string()).unwrap_or_default(),
            debate_advocate_model: debate.advocate_model.unwrap_or_default(),
            debate_skeptic_model: debate.skeptic_model.unwrap_or_default(),
            debate_judge_model: debate.judge_model.unwrap_or_default(),
            debate_contest_critic: debate.contest_critic,
            debate_usability_lens: debate.usability_lens,
        }
    }
}
```

`core::Config` exposes the resolved `design_review` table as a `DesignReviewToml` (or a wrapper). If it does not, the TUI should read the raw fields `design_review_enabled`, `design_review_model`, and `design_review_debate` and reconstruct the table. This is a load-bearing integration detail to verify during implementation. [C:INFERRED]
