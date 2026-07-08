use crate::plan_artifact::{ManifestSnapshot, PartRow, PartStatus, PlanArtifact};
use crate::plan_mode_injector::parts_manifest::{normalize_part_path, parse_parts_manifest, RowStatus};
use ody_config::config_toml::PlanModeConfigToml;
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
                warn!("plan_mode_injector: ignored invalid manifest path {}", row.file);
                continue;
            }
            match row.status {
                RowStatus::Done => done_count += 1,
                RowStatus::Pending => pending_rows.push(PartRow {
                    file_name: row.file.clone(),
                    scope: row.scope.clone(),
                    status: PartStatus::Pending,
                }),
            }
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
                        status: match row.status {
                            RowStatus::Done => PartStatus::Done,
                            RowStatus::Pending => PartStatus::Pending,
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
}

pub fn render_directive(directive: &PlanModeDirective, index_path: &Path) -> Option<String> {
    let _ = index_path;
    match directive {
        PlanModeDirective::StartSplit { next_part } => Some(format!(
            "This plan has been split into multiple parts. Focus this turn on writing only the first pending part: {} (scope: {}). Do not write other parts yet.",
            next_part.relative_path, next_part.scope
        )),
        PlanModeDirective::ContinueSplit { next_part } => Some(format!(
            "Good progress. The next pending part is: {} (scope: {}). Write only this part in the current turn.",
            next_part.relative_path, next_part.scope
        )),
        PlanModeDirective::FinalReview => Some(
            "All parts are marked done. Before finalizing the plan, review the parts for consistency, then output the final <proposed_plan> summary in the index file.".to_string()
        ),
        PlanModeDirective::None => None,
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
                PartRow { file_name: "core.md".to_string(), scope: "models".to_string(), status: PartStatus::Pending },
                PartRow { file_name: "api.md".to_string(), scope: "endpoints".to_string(), status: PartStatus::Pending },
            ],
        };
        artifact.set_last_manifest_snapshot(prev);
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
                PartRow { file_name: "core.md".to_string(), scope: "models".to_string(), status: PartStatus::Pending },
                PartRow { file_name: "api.md".to_string(), scope: "endpoints".to_string(), status: PartStatus::Pending },
            ],
        };
        artifact.set_last_manifest_snapshot(prev);
        let markdown = "## Parts\n| # | File | Scope | Status |\n|---|---|---|---|\n| 1 | core.md | models | done |\n| 2 | api.md | endpoints | done |\n";
        let result = PlanModeInjector::after_plan_turn(&artifact, markdown, None);
        assert_eq!(result.directive, PlanModeDirective::FinalReview);
        assert!(result.boundary_crossed);
    }

    #[tokio::test]
    async fn no_directive_when_nothing_changed() {
        let (artifact, _tmp) = artifact("2026-07-04");
        artifact.finalize_name("topic").await.unwrap();
        let prev = ManifestSnapshot {
            done_count: 1,
            pending_count: 1,
            rows: vec![
                PartRow { file_name: "core.md".to_string(), scope: "models".to_string(), status: PartStatus::Done },
                PartRow { file_name: "api.md".to_string(), scope: "endpoints".to_string(), status: PartStatus::Pending },
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
        assert!(PlanModeInjector::should_trigger_compaction(true, Some(&config), 0.6));
        assert!(!PlanModeInjector::should_trigger_compaction(true, Some(&config), 0.4));
        assert!(!PlanModeInjector::should_trigger_compaction(false, Some(&config), 0.9));
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
}
