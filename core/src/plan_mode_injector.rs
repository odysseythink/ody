use crate::plan_artifact::{ManifestSnapshot, PartRow, PartStatus, PlanArtifact};
use crate::plan_mode_injector::parts_manifest::{
    RowStatus, normalize_part_path, parse_parts_manifest, row_is_verified_done,
};
use ody_config::config_toml::PlanModeConfigToml;
use ody_protocol::config_types::ModeKind;
use std::path::Path;
use tracing::warn;

pub(crate) mod parts_manifest;

#[derive(Debug, Clone, PartialEq)]
pub enum PlanModeDirective {
    None,
    StartSplit { next_part: PartTarget },
    ContinueSplit { next_part: PartTarget },
    FinalReview,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PartTarget {
    pub relative_path: String,
    pub scope: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AfterPlanTurnResult {
    pub directive: PlanModeDirective,
    pub boundary_crossed: bool,
}

pub struct PlanModeInjector;

impl PlanModeInjector {
    pub fn after_plan_turn(
        artifact: &PlanArtifact,
        plan_markdown: &str,
        plan_mode_config: Option<&PlanModeConfigToml>,
    ) -> AfterPlanTurnResult {
        let _ = plan_mode_config;
        let parse_result = parse_parts_manifest(plan_markdown);
        if let Some(warning) = parse_result.warning {
            warn!("plan_mode_injector: {}", warning);
        }

        let Some(manifest) = parse_result.manifest else {
            artifact.clear_last_manifest_snapshot();
            return AfterPlanTurnResult {
                directive: PlanModeDirective::None,
                boundary_crossed: false,
            };
        };

        let Some(stem_dir) = artifact.stem_dir() else {
            artifact.clear_last_manifest_snapshot();
            return AfterPlanTurnResult {
                directive: PlanModeDirective::None,
                boundary_crossed: false,
            };
        };

        let mut done_count = 0usize;
        let mut pending_rows: Vec<PartRow> = Vec::new();
        for row in &manifest.rows {
            if normalize_part_path(&stem_dir, &row.file).is_none() {
                warn!(
                    "plan_mode_injector: ignored invalid manifest path {}",
                    row.file
                );
                continue;
            }
            if row_is_verified_done(&stem_dir, row) {
                done_count += 1;
                continue;
            }
            if row.status == RowStatus::Done {
                warn!(
                    "plan_mode_injector: part {} marked done but its file is missing on disk; treating as pending",
                    row.file
                );
            }
            pending_rows.push(PartRow {
                file_name: row.file.clone(),
                scope: row.scope.clone(),
                status: PartStatus::Pending,
            });
        }

        let prev_snapshot = artifact.last_manifest_snapshot();
        let boundary_crossed = prev_snapshot
            .as_ref()
            .is_some_and(|prev| done_count > prev.done_count);

        artifact.set_last_manifest_snapshot(ManifestSnapshot {
            done_count,
            pending_count: pending_rows.len(),
            rows: manifest
                .rows
                .iter()
                .filter_map(|row| {
                    normalize_part_path(&stem_dir, &row.file).map(|_| PartRow {
                        file_name: row.file.clone(),
                        scope: row.scope.clone(),
                        status: if row_is_verified_done(&stem_dir, row) {
                            PartStatus::Done
                        } else {
                            PartStatus::Pending
                        },
                    })
                })
                .collect(),
        });

        if pending_rows.is_empty() {
            if done_count > 0 {
                return AfterPlanTurnResult {
                    directive: PlanModeDirective::FinalReview,
                    boundary_crossed,
                };
            }
            return AfterPlanTurnResult {
                directive: PlanModeDirective::None,
                boundary_crossed,
            };
        }

        let next_part = &pending_rows[0];
        let target = PartTarget {
            relative_path: next_part.file_name.clone(),
            scope: next_part.scope.clone(),
        };

        let directive = if prev_snapshot.is_none() {
            PlanModeDirective::StartSplit { next_part: target }
        } else if boundary_crossed {
            PlanModeDirective::ContinueSplit { next_part: target }
        } else {
            PlanModeDirective::None
        };

        AfterPlanTurnResult {
            directive,
            boundary_crossed,
        }
    }

    pub fn should_trigger_compaction(
        boundary_crossed: bool,
        plan_mode_config: Option<&PlanModeConfigToml>,
        context_usage_ratio: f64,
    ) -> bool {
        if !boundary_crossed {
            return false;
        }
        let ratio = plan_mode_config
            .and_then(|cfg| cfg.split_plan_compaction_ratio)
            .unwrap_or(0.5);
        ratio > 0.0 && context_usage_ratio >= ratio
    }

    /// Advances the per-plan turn counter, selects a rigor reminder if one is due,
    /// renders it, and records the injection back into the artifact.
    ///
    /// Returns `Some((kind, rendered_text))` when a reminder should be injected,
    /// or `None` when the cadence says no reminder is due this turn.
    ///
    /// Only `ModeKind::Plan` has a rigor tier. This must stay mode-gated even if a
    /// future caller (e.g. a widened Design after-turn hook) reuses this function for
    /// other read-only modes — the reminder text is hard-coded Plan rigor-tier content
    /// (`render_full_reminder`/`render_sparse_reminder`) and must never reach Design.
    pub fn render_reminder_if_due(
        artifact: &PlanArtifact,
        plan_mode_config: Option<&PlanModeConfigToml>,
        mode: ModeKind,
    ) -> Option<(ReminderKind, String)> {
        if mode != ModeKind::Plan {
            return None;
        }
        let current_turn = artifact.next_plan_mode_turn();
        let (last_full_turn, last_any_turn) = artifact.last_reminder_turns();

        // Before the first full reminder has been injected, suppress sparse
        // reminders so the initial full contract (already present in context)
        // isn't diluted by partial pointers ahead of the first scheduled full
        // refresh. When full refresh is disabled (`full_refresh_turns == 0`),
        // leave `last_any_turn` untouched so sparse cadence starts normally.
        let full_refresh = plan_mode_config
            .and_then(|c| c.full_refresh_turns)
            .unwrap_or(5);
        let last_any_turn_for_selection = if full_refresh > 0 && last_full_turn == Some(0) {
            Some(current_turn.saturating_sub(1))
        } else {
            last_any_turn
        };

        let kind = select_reminder(
            current_turn,
            last_full_turn,
            last_any_turn_for_selection,
            plan_mode_config,
        )?;
        let text = match kind {
            ReminderKind::Full => render_full_reminder(),
            ReminderKind::Sparse => render_sparse_reminder(),
        };
        artifact.record_reminder_injected(kind == ReminderKind::Full, current_turn);
        Some((kind, text))
    }
}

/// Resolves the path a pending part should actually be written at: the
/// index's own `<stem>/` directory joined with the part's basename.
///
/// `next_part.relative_path` comes verbatim from the `## Parts` table's
/// `File` cell, which the model is supposed to fill with a bare filename
/// (e.g. `core.md`) — but models sometimes write a directory-prefixed value
/// there instead (e.g. copying a design doc's own filename). `normalize_part_path`
/// already discards any such prefix and re-joins with the real stem directory
/// for *validation*; this does the same for the *directive text shown to the
/// model*, so the two never disagree about where a part belongs.
fn resolved_part_path(index_path: &Path, relative_path: &str) -> String {
    let basename = Path::new(relative_path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| relative_path.to_string());
    match index_path.file_stem() {
        Some(stem) => format!("{}/{}", index_path.with_file_name(stem).display(), basename),
        None => basename,
    }
}

pub fn render_directive(
    directive: &PlanModeDirective,
    index_path: &Path,
    mode: ModeKind,
) -> Option<String> {
    match (mode, directive) {
        (ModeKind::Design, PlanModeDirective::StartSplit { next_part }) => Some(format!(
            "This design has been split into multiple parts. Write only the first pending part this turn: {} (scope: {}). Place it under the design's `<stem>/` directory. One part per turn — do not write other parts yet.",
            resolved_part_path(index_path, &next_part.relative_path), next_part.scope
        )),
        (ModeKind::Design, PlanModeDirective::ContinueSplit { next_part }) => Some(format!(
            "Good progress. The next pending design part is: {} (scope: {}). Write only this part under the `<stem>/` directory in the current turn. One part per turn.",
            resolved_part_path(index_path, &next_part.relative_path), next_part.scope
        )),
        (ModeKind::Design, PlanModeDirective::FinalReview) => Some(
            "All design parts are marked done. Before asking for final approval, run a cross-file consistency review across the index and every `<stem>/` part, then present the final design for approval.".to_string()
        ),
        (_, PlanModeDirective::StartSplit { next_part }) => Some(format!(
            "This plan has been split into multiple parts. Focus this turn on writing only the first pending part: {} (scope: {}). Do not write other parts yet.",
            resolved_part_path(index_path, &next_part.relative_path), next_part.scope
        )),
        (_, PlanModeDirective::ContinueSplit { next_part }) => Some(format!(
            "Good progress. The next pending part is: {} (scope: {}). Write only this part in the current turn.",
            resolved_part_path(index_path, &next_part.relative_path), next_part.scope
        )),
        (_, PlanModeDirective::FinalReview) => Some(
            "All parts are marked done. Before finalizing the plan, review the parts for consistency, then call the submit_plan tool with the final plan markdown as the only action to persist it and end the turn.".to_string()
        ),
        (_, PlanModeDirective::None) => None,
    }
}

/// Renders the full rigor-tier plan-mode reminder.
///
/// Re-injects all rigor fragments so the model keeps the complete contract
/// in context during a long planning session.
pub fn render_full_reminder() -> String {
    format!(
        "## Plan-mode rigor reminder (full)\n\n{}\n\n{}\n\n{}\n\n{}\n\n{}\n\n{}",
        ody_collaboration_mode_templates::PLAN_RIGOR_COVERAGE,
        ody_collaboration_mode_templates::PLAN_RIGOR_SELFREVIEW,
        ody_collaboration_mode_templates::PLAN_RIGOR_INVARIANTS,
        ody_collaboration_mode_templates::PLAN_RIGOR_GROUNDING,
        ody_collaboration_mode_templates::PLAN_RIGOR_SCOPE,
        ody_collaboration_mode_templates::PLAN_RIGOR_RENAME,
    )
}

/// Renders a condensed rigor-tier plan-mode reminder.
///
/// Use between full reinjections to keep the contract alive without
/// repeating the full text every turn.
pub fn render_sparse_reminder() -> String {
    r#"## Plan-mode rigor reminder

You are writing a rigor-tier plan. Keep the following artifacts current:

- Dependency Overview
- Spec-coverage table
- Self-review checklist (all seven items)
- Shared-signature build-green invariant
- No-placeholders rule
- Source-grounding mandate
- Out-of-scope / false-positive discipline
- Rename-vs-delete decision prompt

Quality bar: the plan must stay concrete enough to execute with zero follow-up — complete code in every step (not pseudocode or "similar to Task N"), exact commands with expected output, and per-task tests that assert the actual risk being changed.
"#
    .to_string()
}

/// Kind of rigor-tier reminder that may be reinjected into a plan-mode session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReminderKind {
    Full,
    Sparse,
}

/// Selects the reminder kind (if any) for the current plan-mode turn.
///
/// `last_full_turn` is the turn at which a full reminder was last injected;
/// `last_any_turn` is the turn at which any reminder (full or sparse) was
/// last injected. The caller is responsible for updating both values after
/// injecting a reminder. Returns `None` when no reminder is due.
pub fn select_reminder(
    current_turn: usize,
    last_full_turn: Option<usize>,
    last_any_turn: Option<usize>,
    config: Option<&PlanModeConfigToml>,
) -> Option<ReminderKind> {
    let full_refresh = config.and_then(|c| c.full_refresh_turns).unwrap_or(5);
    let dedup_min = config.and_then(|c| c.dedup_min_turns).unwrap_or(2);

    if full_refresh > 0 {
        let full_due = last_full_turn.map_or(true, |last| {
            current_turn.saturating_sub(last) >= full_refresh
        });
        if full_due {
            return Some(ReminderKind::Full);
        }
    }

    if dedup_min > 0 {
        let sparse_due =
            last_any_turn.map_or(true, |last| current_turn.saturating_sub(last) >= dedup_min);
        if sparse_due {
            return Some(ReminderKind::Sparse);
        }
    }

    None
}

#[cfg(test)]
mod directive_tests {
    use super::*;
    use crate::plan_artifact::PlanArtifact;
    use ody_protocol::ThreadId;
    use ody_utils_absolute_path::AbsolutePathBuf;

    fn artifact(date: &str) -> (PlanArtifact, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let plans_base_dir = AbsolutePathBuf::from_absolute_path(tmp.path()).unwrap();
        let thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000001").unwrap();
        (PlanArtifact::new_temp(plans_base_dir, thread_id, date), tmp)
    }

    #[tokio::test]
    async fn start_split_on_first_manifest() {
        let (artifact, _tmp) = artifact("2026-07-04");
        artifact.finalize_name("topic").await.unwrap();
        let markdown = "## Parts\n| # | File | Scope | Status |\n|---|---|---|---|\n| 1 | core.md | models | pending |\n| 2 | api.md | endpoints | pending |\n";
        let result = PlanModeInjector::after_plan_turn(&artifact, markdown, None);
        assert_eq!(
            result.directive,
            PlanModeDirective::StartSplit {
                next_part: PartTarget {
                    relative_path: "core.md".to_string(),
                    scope: "models".to_string(),
                }
            }
        );
        assert!(!result.boundary_crossed);
    }

    #[tokio::test]
    async fn continue_split_after_boundary() {
        let (artifact, _tmp) = artifact("2026-07-04");
        artifact.finalize_name("topic").await.unwrap();
        let prev = ManifestSnapshot {
            done_count: 0,
            pending_count: 2,
            rows: vec![
                PartRow {
                    file_name: "core.md".to_string(),
                    scope: "models".to_string(),
                    status: PartStatus::Pending,
                },
                PartRow {
                    file_name: "api.md".to_string(),
                    scope: "endpoints".to_string(),
                    status: PartStatus::Pending,
                },
            ],
        };
        artifact.set_last_manifest_snapshot(prev);
        let stem_dir = artifact.stem_dir().unwrap();
        std::fs::create_dir_all(&stem_dir).unwrap();
        std::fs::write(stem_dir.join("core.md"), "# Core\n").unwrap();
        let markdown = "## Parts\n| # | File | Scope | Status |\n|---|---|---|---|\n| 1 | core.md | models | done |\n| 2 | api.md | endpoints | pending |\n";
        let result = PlanModeInjector::after_plan_turn(&artifact, markdown, None);
        assert_eq!(
            result.directive,
            PlanModeDirective::ContinueSplit {
                next_part: PartTarget {
                    relative_path: "api.md".to_string(),
                    scope: "endpoints".to_string(),
                }
            }
        );
        assert!(result.boundary_crossed);
    }

    #[tokio::test]
    async fn final_review_when_all_done() {
        let (artifact, _tmp) = artifact("2026-07-04");
        artifact.finalize_name("topic").await.unwrap();
        let prev = ManifestSnapshot {
            done_count: 0,
            pending_count: 2,
            rows: vec![
                PartRow {
                    file_name: "core.md".to_string(),
                    scope: "models".to_string(),
                    status: PartStatus::Pending,
                },
                PartRow {
                    file_name: "api.md".to_string(),
                    scope: "endpoints".to_string(),
                    status: PartStatus::Pending,
                },
            ],
        };
        artifact.set_last_manifest_snapshot(prev);
        let stem_dir = artifact.stem_dir().unwrap();
        std::fs::create_dir_all(&stem_dir).unwrap();
        std::fs::write(stem_dir.join("core.md"), "# Core\n").unwrap();
        std::fs::write(stem_dir.join("api.md"), "# Api\n").unwrap();
        let markdown = "## Parts\n| # | File | Scope | Status |\n|---|---|---|---|\n| 1 | core.md | models | done |\n| 2 | api.md | endpoints | done |\n";
        let result = PlanModeInjector::after_plan_turn(&artifact, markdown, None);
        assert_eq!(result.directive, PlanModeDirective::FinalReview);
        assert!(result.boundary_crossed);
    }

    /// Regression test: a model can flip a `## Parts` row to `done` in the
    /// index markdown without ever having written that part's file (e.g. it
    /// had no working file-write tool in Plan mode). That must not be treated
    /// as real completion — the row should stay pending so the model is
    /// redirected back to it instead of ending Plan mode with a missing part.
    #[tokio::test]
    async fn done_row_with_missing_file_is_treated_as_pending() {
        let (artifact, _tmp) = artifact("2026-07-04");
        artifact.finalize_name("topic").await.unwrap();
        let prev = ManifestSnapshot {
            done_count: 0,
            pending_count: 2,
            rows: vec![
                PartRow {
                    file_name: "core.md".to_string(),
                    scope: "models".to_string(),
                    status: PartStatus::Pending,
                },
                PartRow {
                    file_name: "api.md".to_string(),
                    scope: "endpoints".to_string(),
                    status: PartStatus::Pending,
                },
            ],
        };
        artifact.set_last_manifest_snapshot(prev);
        // Note: neither core.md nor api.md is ever written to disk here.
        let markdown = "## Parts\n| # | File | Scope | Status |\n|---|---|---|---|\n| 1 | core.md | models | done |\n| 2 | api.md | endpoints | done |\n";
        let result = PlanModeInjector::after_plan_turn(&artifact, markdown, None);
        assert_eq!(
            result.directive,
            PlanModeDirective::None,
            "a done row without a file on disk must not trigger FinalReview"
        );
        assert!(
            !result.boundary_crossed,
            "no row is actually verified done, so no boundary was crossed"
        );
    }

    #[tokio::test]
    async fn no_directive_when_nothing_changed() {
        let (artifact, _tmp) = artifact("2026-07-04");
        artifact.finalize_name("topic").await.unwrap();
        let prev = ManifestSnapshot {
            done_count: 1,
            pending_count: 1,
            rows: vec![
                PartRow {
                    file_name: "core.md".to_string(),
                    scope: "models".to_string(),
                    status: PartStatus::Done,
                },
                PartRow {
                    file_name: "api.md".to_string(),
                    scope: "endpoints".to_string(),
                    status: PartStatus::Pending,
                },
            ],
        };
        artifact.set_last_manifest_snapshot(prev);
        let markdown = "## Parts\n| # | File | Scope | Status |\n|---|---|---|---|\n| 1 | core.md | models | done |\n| 2 | api.md | endpoints | pending |\n";
        let result = PlanModeInjector::after_plan_turn(&artifact, markdown, None);
        assert_eq!(result.directive, PlanModeDirective::None);
        assert!(!result.boundary_crossed);
    }

    #[test]
    fn should_trigger_compaction_respects_ratio() {
        let config = PlanModeConfigToml {
            split_plan_compaction_ratio: Some(0.5),
            ..Default::default()
        };
        assert!(PlanModeInjector::should_trigger_compaction(
            true,
            Some(&config),
            0.6
        ));
        assert!(!PlanModeInjector::should_trigger_compaction(
            true,
            Some(&config),
            0.4
        ));
        assert!(!PlanModeInjector::should_trigger_compaction(
            false,
            Some(&config),
            0.9
        ));
    }

    #[test]
    fn full_reminder_contains_selfreview_and_coverage() {
        let reminder = render_full_reminder();
        assert!(
            reminder.contains("## Rigor tier addendum: Self-review checklist"),
            "full reminder should contain self-review fragment:\n{reminder}"
        );
        assert!(
            reminder.contains("## Rigor tier addendum: Dependency Overview + Spec-coverage"),
            "full reminder should contain coverage fragment:\n{reminder}"
        );
        assert!(
            reminder.contains("## Spec-coverage table"),
            "full reminder should contain spec-coverage requirement:\n{reminder}"
        );
        assert!(
            reminder.contains("trace one concrete value"),
            "full reminder should contain end-to-end trace requirement:\n{reminder}"
        );
    }

    #[test]
    fn sparse_reminder_lists_key_artifacts() {
        let reminder = render_sparse_reminder();
        assert!(
            reminder.contains("## Plan-mode rigor reminder"),
            "sparse reminder should have a heading:\n{reminder}"
        );
        for artifact in [
            "Dependency Overview",
            "Spec-coverage table",
            "Self-review checklist",
            "Shared-signature build-green invariant",
            "No-placeholders rule",
            "Source-grounding mandate",
            "Out-of-scope",
            "Rename-vs-delete",
        ] {
            assert!(
                reminder.contains(artifact),
                "sparse reminder should mention {artifact}:\n{reminder}"
            );
        }
    }

    #[test]
    fn sparse_reminder_restates_the_complete_code_quality_bar() {
        // The sparse reminder is what actually reaches long plan-mode sessions between
        // full reinjections (see `select_reminder`: full only fires once, at turn 5).
        // If it only names artifact categories without restating the concrete
        // "no pseudocode, complete code" bar, a long session can drift toward the
        // base template's "compress/omit" guidance and stop writing full code per task.
        let reminder = render_sparse_reminder();
        assert!(
            reminder.contains("zero follow-up"),
            "sparse reminder should restate the zero-follow-up execution bar:\n{reminder}"
        );
        assert!(
            reminder.contains("complete code in every step"),
            "sparse reminder should restate the complete-code-per-step requirement:\n{reminder}"
        );
    }

    #[test]
    fn select_reminder_defaults_to_full_every_five_turns() {
        let config = PlanModeConfigToml::default();
        assert_eq!(
            select_reminder(5, None, None, Some(&config)),
            Some(ReminderKind::Full),
            "turn 5 should trigger the first full reminder"
        );
    }

    #[test]
    fn select_reminder_sparse_between_full_injections() {
        let config = PlanModeConfigToml::default();
        // After a full injection at turn 5, turn 7 is the first turn a sparse reminder is allowed.
        assert_eq!(
            select_reminder(7, Some(5), Some(5), Some(&config)),
            Some(ReminderKind::Sparse),
            "turn 7 should trigger a sparse reminder after full at turn 5"
        );
        assert_eq!(
            select_reminder(6, Some(5), Some(5), Some(&config)),
            None,
            "turn 6 should be deduplicated after full at turn 5"
        );
    }

    #[test]
    fn select_reminder_respects_zero_to_disable() {
        let no_full = PlanModeConfigToml {
            full_refresh_turns: Some(0),
            dedup_min_turns: Some(2),
            ..Default::default()
        };
        assert_eq!(
            select_reminder(5, None, None, Some(&no_full)),
            Some(ReminderKind::Sparse),
            "full_refresh=0 should fall back to sparse when sparse is due"
        );

        let no_sparse = PlanModeConfigToml {
            full_refresh_turns: Some(5),
            dedup_min_turns: Some(0),
            ..Default::default()
        };
        assert_eq!(
            select_reminder(7, Some(5), Some(5), Some(&no_sparse)),
            None,
            "dedup_min=0 should disable sparse reminders"
        );
        assert_eq!(
            select_reminder(10, Some(5), Some(5), Some(&no_sparse)),
            Some(ReminderKind::Full),
            "dedup_min=0 should still allow full reminders when due"
        );
    }

    #[test]
    fn render_reminder_if_due_follows_full_sparse_cadence() {
        let tmp = tempfile::tempdir().unwrap();
        let plans_base_dir = AbsolutePathBuf::from_absolute_path(tmp.path()).unwrap();
        let thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000001").unwrap();
        let artifact = PlanArtifact::new_temp(plans_base_dir, thread_id, "2026-07-04");
        let config = PlanModeConfigToml::default();

        // Turns 1-4: nothing is due.
        for _ in 1..=4 {
            assert_eq!(
                PlanModeInjector::render_reminder_if_due(&artifact, Some(&config), ModeKind::Plan),
                None,
                "no reminder before turn 5"
            );
        }

        // Turn 5: full reminder.
        let (kind, text) =
            PlanModeInjector::render_reminder_if_due(&artifact, Some(&config), ModeKind::Plan)
                .expect("turn 5 should emit a full reminder");
        assert_eq!(kind, ReminderKind::Full);
        assert!(
            text.contains("## Plan-mode rigor reminder (full)"),
            "full reminder should carry the full heading:\n{text}"
        );

        // Turn 6: deduplicated after full at turn 5.
        assert_eq!(
            PlanModeInjector::render_reminder_if_due(&artifact, Some(&config), ModeKind::Plan),
            None,
            "turn 6 should be deduplicated"
        );

        // Turn 7: sparse reminder.
        let (kind, text) =
            PlanModeInjector::render_reminder_if_due(&artifact, Some(&config), ModeKind::Plan)
                .expect("turn 7 should emit a sparse reminder");
        assert_eq!(kind, ReminderKind::Sparse);
        assert!(
            text.contains("## Plan-mode rigor reminder"),
            "sparse reminder should carry the sparse heading:\n{text}"
        );
    }

    #[test]
    fn render_reminder_if_due_never_fires_for_design_mode() {
        // D4 will widen the after-turn hook's mode gate from `== Plan` to
        // `is_read_only_session_mode` (which admits Design). This function's own
        // mode guard must independently block Design so that hard-coded Plan
        // rigor-tier text ("Self-review checklist", "Shared-signature build-green
        // invariant") can never leak into a Design session, regardless of how the
        // call site is gated.
        let tmp = tempfile::tempdir().unwrap();
        let plans_base_dir = AbsolutePathBuf::from_absolute_path(tmp.path()).unwrap();
        let thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000002").unwrap();
        let artifact = PlanArtifact::new_design(plans_base_dir, thread_id, "2026-07-10");
        let config = PlanModeConfigToml::default();

        // Turn 5 is exactly when Plan mode would emit a full reminder (see the
        // cadence test above). Design must stay silent at every one of these turns.
        for _ in 1..=7 {
            assert_eq!(
                PlanModeInjector::render_reminder_if_due(
                    &artifact,
                    Some(&config),
                    ModeKind::Design
                ),
                None,
                "Design mode must never receive a Plan rigor-tier reminder"
            );
        }
    }

    #[test]
    fn render_directive_plan_keeps_existing_copy() {
        let dir = PlanModeDirective::StartSplit {
            next_part: PartTarget {
                relative_path: "core.md".into(),
                scope: "models".into(),
            },
        };
        let text =
            render_directive(&dir, Path::new("plan.md"), ModeKind::Plan).expect("plan start");
        assert!(text.contains("split into multiple parts"), "{text}");
        assert!(text.contains("core.md"), "{text}");
        assert!(text.contains("models"), "{text}");
    }

    /// Regression test for a real session: the model wrote a directory-prefixed
    /// value into the `## Parts` table's `File` cell (copying an unrelated design
    /// doc's filename) instead of a bare basename. `normalize_part_path` already
    /// discards that prefix for *validation*, re-joining with the real stem
    /// directory — but `render_directive` used to echo the raw (wrong) cell value
    /// verbatim in the directive text shown to the model, so the model dutifully
    /// tried to `mkdir`/write into a directory that didn't match the plan's real
    /// stem and got denied. The directive must always show the real path.
    #[test]
    fn render_directive_resolves_wrong_directory_prefix_in_relative_path() {
        let dir = PlanModeDirective::StartSplit {
            next_part: PartTarget {
                relative_path: "2026-07-12-mode-models-per-mode-config/config-schema.md".into(),
                scope: "config schema".into(),
            },
        };
        let index_path =
            Path::new("/repo/.ody-code/plans/2026-07-12-mode_models_implementation_plan.md");
        let text = render_directive(&dir, index_path, ModeKind::Plan).expect("plan start");
        assert!(
            text.contains(
                "/repo/.ody-code/plans/2026-07-12-mode_models_implementation_plan/config-schema.md"
            ),
            "directive must resolve the part against the real stem directory, not the model's guessed prefix: {text}"
        );
        assert!(
            !text.contains("2026-07-12-mode-models-per-mode-config"),
            "directive must not repeat the model's wrong directory prefix: {text}"
        );
    }

    #[test]
    fn render_directive_design_start_split_one_part_per_turn() {
        let dir = PlanModeDirective::StartSplit {
            next_part: PartTarget {
                relative_path: "core.md".into(),
                scope: "data models".into(),
            },
        };
        let text =
            render_directive(&dir, Path::new("design.md"), ModeKind::Design).expect("design start");
        assert!(text.contains("core.md"), "{text}");
        assert!(text.contains("data models"), "{text}");
        assert!(text.to_lowercase().contains("one part per turn"), "{text}");
        assert!(
            !text.contains("Self-review checklist"),
            "design copy must not carry plan rigor: {text}"
        );
    }

    #[test]
    fn render_directive_design_continue_split_mentions_stem() {
        let dir = PlanModeDirective::ContinueSplit {
            next_part: PartTarget {
                relative_path: "api.md".into(),
                scope: "endpoints".into(),
            },
        };
        let text = render_directive(&dir, Path::new("design.md"), ModeKind::Design)
            .expect("design continue");
        assert!(text.contains("api.md"), "{text}");
        assert!(text.to_lowercase().contains("stem"), "{text}");
    }

    #[test]
    fn render_directive_design_final_review_mentions_cross_file() {
        let text = render_directive(
            &PlanModeDirective::FinalReview,
            Path::new("design.md"),
            ModeKind::Design,
        )
        .expect("design final review");
        assert!(text.to_lowercase().contains("cross-file"), "{text}");
        assert!(
            !text.contains("submit_plan"),
            "design final review must not ask for a plan summary: {text}"
        );
    }
}
