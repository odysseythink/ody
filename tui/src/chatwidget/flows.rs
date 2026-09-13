//! M4 `/flows` view support: a per-session registry of flow runs fed by
//! `ThreadItem::FlowPhase` (yaml carrier phase progress, M1.4) and
//! `ThreadItem::FlowLog` (script carrier progress lines, M4), rendered
//! read-only by the `/flows` slash command.
//!
//! Read-only by design (M4 plan decision A): run pause/stop/restart and
//! per-agent drill-down need execution-control paths and richer protocol
//! events that do not exist yet.

use std::collections::BTreeMap;

use ody_app_server_protocol::ThreadItem;
use ratatui::text::Line;

/// One phase row within a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FlowPhaseSnapshot {
    pub(super) phase_id: String,
    pub(super) completed_steps: u32,
    pub(super) total_steps: u32,
    pub(super) finished: bool,
}

/// Snapshot of one flow run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct FlowRunSnapshot {
    pub(super) flow_name: String,
    pub(super) phases: Vec<FlowPhaseSnapshot>,
    pub(super) logs: Vec<String>,
}

impl FlowRunSnapshot {
    /// Yaml runs complete when every known phase finished; script carriers
    /// only emit log lines, and completion is not observable on the
    /// protocol surface, so they stay "running" in this view.
    fn status(&self) -> &'static str {
        if !self.phases.is_empty() && self.phases.iter().all(|phase| phase.finished) {
            "completed"
        } else {
            "running"
        }
    }
}

/// Per-session registry keyed by run call id. `BTreeMap` keeps the listing
/// order stable across incremental updates.
#[derive(Debug, Default)]
pub(super) struct FlowRuns(pub(super) BTreeMap<String, FlowRunSnapshot>);

impl FlowRuns {
    /// Record one flow progress item. Unknown item kinds are ignored so
    /// callers can pass every thread item through.
    pub(super) fn record_item(&mut self, item: &ThreadItem) {
        match item {
            ThreadItem::FlowPhase {
                id,
                flow_name,
                phase_id,
                total_steps,
                completed_steps,
                finished,
            } => {
                let run = self.run_mut(id, flow_name.as_deref());
                match run.phases.iter_mut().find(|phase| phase.phase_id == *phase_id) {
                    Some(phase) => {
                        phase.completed_steps = *completed_steps;
                        phase.total_steps = *total_steps;
                        phase.finished = *finished;
                    }
                    None => run.phases.push(FlowPhaseSnapshot {
                        phase_id: phase_id.clone(),
                        completed_steps: *completed_steps,
                        total_steps: *total_steps,
                        finished: *finished,
                    }),
                }
            }
            ThreadItem::FlowLog { id, flow_name, message } => {
                self.run_mut(id, flow_name.as_deref()).logs.push(message.clone());
            }
            _ => {}
        }
    }

    /// Item ids embed the run call id as the `:`-separated prefix
    /// (`{call_id}:{phase_id}` / `{call_id}:log-{ms}`).
    fn run_mut(&mut self, item_id: &str, flow_name: Option<&str>) -> &mut FlowRunSnapshot {
        let call_id = item_id.split(':').next().unwrap_or(item_id).to_string();
        let run = self.0.entry(call_id).or_default();
        if let Some(name) = flow_name {
            if !name.is_empty() {
                run.flow_name = name.to_string();
            }
        }
        run
    }

    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Render the read-only `/flows` listing as history lines.
    pub(super) fn render_lines(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        for (call_id, run) in &self.0 {
            let name = if run.flow_name.is_empty() {
                String::new()
            } else {
                format!(" · {}", run.flow_name)
            };
            lines.push(Line::from(format!(
                "● {call_id}{name} · {}",
                run.status()
            )));
            for phase in &run.phases {
                let marker = if phase.finished { "✓" } else { "·" };
                lines.push(Line::from(format!(
                    "  {marker} phase {} · {}/{} steps",
                    phase.phase_id, phase.completed_steps, phase.total_steps
                )));
            }
            for log in &run.logs {
                lines.push(Line::from(format!("  · {log}")));
            }
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn phase_item(call_id: &str, phase_id: &str, completed: u32, finished: bool) -> ThreadItem {
        ThreadItem::FlowPhase {
            id: format!("{call_id}:{phase_id}"),
            flow_name: Some("demo".to_string()),
            phase_id: phase_id.to_string(),
            total_steps: 2,
            completed_steps: completed,
            finished,
        }
    }

    fn log_item(call_id: &str, ms: i64, message: &str) -> ThreadItem {
        ThreadItem::FlowLog {
            id: format!("{call_id}:log-{ms}"),
            flow_name: Some("demo".to_string()),
            message: message.to_string(),
        }
    }

    #[test]
    fn registry_aggregates_phases_per_run() {
        let mut runs = FlowRuns::default();
        runs.record_item(&phase_item("run-1", "design", 0, false));
        runs.record_item(&phase_item("run-1", "design", 1, false));
        runs.record_item(&phase_item("run-1", "design", 2, true));
        runs.record_item(&phase_item("run-1", "impl", 0, false));
        let run = runs.0.get("run-1").expect("run registered");
        assert_eq!(run.flow_name, "demo");
        assert_eq!(run.phases.len(), 2, "same phase id updates in place");
        assert_eq!(run.phases[0].completed_steps, 2);
        assert!(!run.phases[1].finished);
        assert_eq!(run.status(), "running");
    }

    #[test]
    fn completed_status_when_all_phases_finished() {
        let mut runs = FlowRuns::default();
        runs.record_item(&phase_item("run-2", "a", 1, true));
        runs.record_item(&phase_item("run-2", "b", 1, true));
        assert_eq!(runs.0["run-2"].status(), "completed");
    }

    #[test]
    fn script_log_lines_accumulate_and_stay_running() {
        let mut runs = FlowRuns::default();
        runs.record_item(&log_item("run-3", 10, "planning"));
        runs.record_item(&log_item("run-3", 20, "reviewing"));
        let run = runs.0.get("run-3").expect("run registered");
        assert_eq!(run.logs, vec!["planning".to_string(), "reviewing".to_string()]);
        assert_eq!(run.status(), "running");
    }

    #[test]
    fn render_lists_runs_with_phases_and_logs() {
        let mut runs = FlowRuns::default();
        runs.record_item(&phase_item("run-1", "design", 2, true));
        runs.record_item(&log_item("run-3", 10, "planning"));
        let text: Vec<String> = runs
            .render_lines()
            .into_iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref().to_string())
                    .collect::<Vec<_>>()
                    .join("")
            })
            .collect();
        assert_eq!(
            text,
            vec![
                "● run-1 · demo · completed".to_string(),
                "  ✓ phase design · 2/2 steps".to_string(),
                "● run-3 · demo · running".to_string(),
                "  · planning".to_string(),
            ]
        );
    }

    #[test]
    fn unrelated_items_are_ignored() {
        let mut runs = FlowRuns::default();
        runs.record_item(&ThreadItem::ContextCompaction { id: "compact-1".to_string() });
        assert!(runs.is_empty());
    }
}
