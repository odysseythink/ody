//! Pure Design→Plan handoff evaluator (D6).
//!
//! On any edge leaving Design mode, [`evaluate_design_exit`] runs the C1–C8
//! completeness gate (from `design_completeness`) against the cached artifact
//! and decides whether the switch is allowed. It owns no session state and
//! performs no locking — the only async step is a single `tokio::fs::read_to_string`
//! so callers can release the session lock before awaiting it.

use crate::design_completeness::design_completeness_report;
use crate::plan_artifact::PlanArtifact;
use crate::turn_timing::now_unix_timestamp_ms;
use ody_config::config_toml::PlanEnforcement;
use ody_protocol::config_types::ModeKind;
use ody_protocol::protocol::PlanModeLogEvent;
use ody_protocol::protocol::PlanModeLogKind;
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

/// Render inputs for a Design→Plan reminder. `selected_label` is reserved for a
/// later interactive-selection step (D6-3) and is always `None` in v1.
pub(crate) struct PendingDesignHandoff {
    pub path: PathBuf,
    pub filename: String,
    pub selected_label: Option<String>,
    pub incomplete_note: Option<String>,
}

#[derive(Debug)]
pub(crate) enum HandoffDecision {
    /// The switch may commit. `reminder` is `Some` only when `new == Plan`.
    Allow {
        reminder: Option<String>,
        logs: Vec<PlanModeLogEvent>,
    },
    /// The switch is rejected (Strict + incomplete). `missing_report` is the
    /// user-facing missing-sections list.
    Veto {
        missing_report: String,
        logs: Vec<PlanModeLogEvent>,
    },
}

pub(crate) async fn evaluate_design_exit(
    artifact: Option<Arc<PlanArtifact>>,
    new_mode: ModeKind,
    enforcement: PlanEnforcement,
) -> HandoffDecision {
    let content = read_artifact_content(artifact.as_ref()).await;
    let report = design_completeness_report(&content);
    let complete = report.is_none();

    let mut logs = Vec::new();
    logs.push(make_design_completeness_log(complete, report.as_deref()));

    if !complete && enforcement == PlanEnforcement::Strict {
        logs.push(make_mode_transition_log(
            new_mode,
            "Design handoff vetoed: design is incomplete.".to_string(),
        ));
        return HandoffDecision::Veto {
            missing_report: render_veto_message(&report.unwrap_or_default()),
            logs,
        };
    }
    if new_mode != ModeKind::Plan {
        // D6-2: the gate fires on every Design exit, but the reminder is only
        // injected when the destination is Plan mode.
        logs.push(make_mode_transition_log(
            new_mode,
            format!("Switching from Design mode to {new_mode:?}."),
        ));
        return HandoffDecision::Allow {
            reminder: None,
            logs,
        };
    }
    let path = artifact
        .as_ref()
        .and_then(|a| a.path())
        .unwrap_or_else(|| PathBuf::from("<unknown>"));
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("<unknown>")
        .to_string();
    let incomplete_note = match report {
        Some(r) if !complete => Some(render_incomplete_note(
            &r,
            /*strong=*/ enforcement == PlanEnforcement::Ask,
        )),
        _ => None,
    };
    let handoff = PendingDesignHandoff {
        path,
        filename,
        selected_label: None,
        incomplete_note,
    };
    let reminder = render_handoff_reminder(&handoff);
    logs.push(make_mode_transition_log(
        ModeKind::Plan,
        "Design handoff approved.".to_string(),
    ));
    HandoffDecision::Allow {
        reminder: Some(reminder),
        logs,
    }
}

fn make_design_completeness_log(complete: bool, report: Option<&str>) -> PlanModeLogEvent {
    let message = if complete {
        "Design completeness check passed.".to_string()
    } else {
        "Design completeness check: missing sections detected.".to_string()
    };
    PlanModeLogEvent {
        event_id: Uuid::new_v4().to_string(),
        occurred_at_ms: now_unix_timestamp_ms(),
        kind: PlanModeLogKind::DesignCompletenessCheck,
        message,
        detail: report.map(|r| r.to_string()),
    }
}

fn make_mode_transition_log(new_mode: ModeKind, message: String) -> PlanModeLogEvent {
    PlanModeLogEvent {
        event_id: Uuid::new_v4().to_string(),
        occurred_at_ms: now_unix_timestamp_ms(),
        kind: PlanModeLogKind::ModeTransition,
        message,
        detail: Some(format!("new_mode={:?}", new_mode)),
    }
}

async fn read_artifact_content(artifact: Option<&Arc<PlanArtifact>>) -> String {
    let Some(path) = artifact.and_then(|a| a.path()) else {
        return String::new();
    };
    // Fail-safe: any unreadable/missing artifact is treated as incomplete.
    tokio::fs::read_to_string(path).await.unwrap_or_default()
}

pub(crate) fn render_handoff_reminder(handoff: &PendingDesignHandoff) -> String {
    let mut rendered = String::from(
        "Design mode completed. The approved design has been handed off — you are now in plan mode.\n",
    );
    rendered.push_str(&format!("Design saved to: {}\n", handoff.path.display()));
    if let Some(label) = &handoff.selected_label {
        rendered.push_str(&format!(
            "Selected approach: {label}. Execute ONLY the selected approach; do not execute unselected alternatives.\n"
        ));
    }
    rendered.push_str(&format!(
        "Create a concrete, step-by-step implementation plan based on the approved design in `{}`. Do not implement anything yet.",
        handoff.filename
    ));
    if let Some(note) = &handoff.incomplete_note {
        rendered.push_str("\n\n");
        rendered.push_str(note);
    }
    rendered
}

fn render_veto_message(missing_report: &str) -> String {
    format!(
        "Design is incomplete; staying in design mode. Missing sections:\n{missing_report}\n\
         Please complete the design before switching to plan mode."
    )
}

fn render_incomplete_note(report: &str, strong: bool) -> String {
    if strong {
        format!(
            "WARNING: the design appears incomplete and is missing required sections:\n{report}\n\
             You should complete these sections before implementing."
        )
    } else {
        format!("Note: the design may be incomplete:\n{report}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_includes_selected_label_when_some() {
        let handoff = PendingDesignHandoff {
            path: PathBuf::from("/p/.ody-code/designs/2026-07-10-x.md"),
            filename: "2026-07-10-x.md".into(),
            selected_label: Some("Option B".into()),
            incomplete_note: None,
        };
        let rendered = render_handoff_reminder(&handoff);
        assert!(rendered.contains("Selected approach: Option B"));
        assert!(rendered.contains("handed off"));
        assert!(rendered.contains("/p/.ody-code/designs/2026-07-10-x.md"));
    }

    #[test]
    fn render_omits_selected_label_when_none() {
        let handoff = PendingDesignHandoff {
            path: PathBuf::from("/p/d.md"),
            filename: "d.md".into(),
            selected_label: None,
            incomplete_note: None,
        };
        assert!(!render_handoff_reminder(&handoff).contains("Selected approach"));
    }

    #[test]
    fn render_appends_incomplete_note() {
        let handoff = PendingDesignHandoff {
            path: PathBuf::from("/p/d.md"),
            filename: "d.md".into(),
            selected_label: None,
            incomplete_note: Some("NOTE".into()),
        };
        assert!(render_handoff_reminder(&handoff).ends_with("NOTE"));
    }

    #[test]
    fn ask_and_advisory_notes_differ() {
        assert_ne!(
            render_incomplete_note("r", true),
            render_incomplete_note("r", false)
        );
    }

    #[tokio::test]
    async fn evaluate_strict_incomplete_vetoes() {
        let decision = evaluate_design_exit(None, ModeKind::Plan, PlanEnforcement::Strict).await;
        assert!(matches!(decision, HandoffDecision::Veto { .. }));
    }

    #[tokio::test]
    async fn evaluate_advisory_incomplete_allows_plan_with_reminder() {
        let decision = evaluate_design_exit(None, ModeKind::Plan, PlanEnforcement::Advisory).await;
        assert!(matches!(
            decision,
            HandoffDecision::Allow {
                reminder: Some(_),
                ..
            }
        ));
    }

    #[tokio::test]
    async fn evaluate_non_plan_target_never_reminds() {
        let decision =
            evaluate_design_exit(None, ModeKind::Default, PlanEnforcement::Advisory).await;
        assert!(matches!(
            decision,
            HandoffDecision::Allow { reminder: None, .. }
        ));
    }

    #[tokio::test]
    async fn evaluate_advisory_emits_design_completeness_log() {
        let decision = evaluate_design_exit(None, ModeKind::Plan, PlanEnforcement::Advisory).await;
        let logs = match decision {
            HandoffDecision::Allow { logs, .. } => logs,
            other => panic!("expected Allow, got {other:?}"),
        };
        assert!(
            logs.iter()
                .any(|log| log.kind == PlanModeLogKind::DesignCompletenessCheck),
            "expected a design completeness log, got {logs:?}"
        );
    }
}
