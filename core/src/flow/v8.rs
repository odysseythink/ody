//! The `workflow.js` V8 runtime (M3.2, `flow-v8` feature).
//!
//! Thin core-side adapter over [`ody_code_mode::run_workflow_script`]
//! (the deterministic one-shot workflow engine; see that module for the
//! thread/bridge/cancellation design and the host-function semantics).
//! This file owns only the Flow mapping: plan source in, [`FlowOutcome`]
//! out, plus the [`FlowAgentHost`] -> `WorkflowHost` bridge. Checkpoint
//! read/run/write lives in the bridge, mirroring the M2.1 yaml semantics
//! and the `flow.star` runtime exactly (decision 7).
//!
//! Result contract: the script binds a top-level `result`
//! (`var result = ...` or `globalThis.result = ...`; the engine mirrors
//! module-scope bindings onto `globalThis`). It becomes
//! `FlowOutcome.outputs["result"]`.

use ody_code_mode::WorkflowHost;
use ody_code_mode::WorkflowRunConfig;
use ody_code_mode::run_workflow_script;

use super::FLOW_BATCH_LIMIT;
use super::FlowAgentHost;
use super::FlowContext;
use super::FlowError;
use super::FlowHostError;
use super::FlowOutcome;
use super::FlowPlanSource;
use super::FlowProgress;
use super::FlowRuntime;

/// The `workflow.js` runtime: syntax check via
/// [`ody_code_mode::check_workflow_syntax`] at validate time; execute via
/// the one-shot workflow engine.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct V8FlowRuntime;

impl V8FlowRuntime {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl FlowRuntime for V8FlowRuntime {
    fn supported_artifact(&self) -> &'static str {
        "workflow.js"
    }

    fn validate(&self, source: &str) -> Result<FlowPlanSource, FlowError> {
        ody_code_mode::check_workflow_syntax(source)
            .map(|_| FlowPlanSource::V8(source.to_string()))
            .map_err(|reason| FlowError::Parse { reason })
    }

    async fn run<H: FlowAgentHost>(
        &self,
        plan: FlowPlanSource,
        ctx: FlowContext,
        host: &H,
    ) -> Result<FlowOutcome, FlowError> {
        let FlowPlanSource::V8(source) = plan else {
            return Err(FlowError::Parse {
                reason: "v8 runtime received a non-v8 plan".to_string(),
            });
        };
        let workflow_host = FlowWorkflowHost {
            host,
            open_phase: std::sync::Mutex::new(None),
        };
        let result = run_workflow_script(
            &source,
            &WorkflowRunConfig {
                args: serde_json::Value::Object(ctx.args),
                batch_limit: FLOW_BATCH_LIMIT,
            },
            &workflow_host,
        )
        .await;

        match result {
            Ok(result) => {
                // Run succeeded: close the final phase as completed (1/1).
                workflow_host.close_open_phase(1).await;
                let mut outputs = serde_json::Map::new();
                if let Some(value) = result {
                    outputs.insert("result".to_string(), value);
                }
                Ok(FlowOutcome { outputs })
            }
            Err(message) => {
                // Failure path mirrors the yaml/star runtimes: the open
                // phase (if any) closes with completed < total.
                workflow_host.close_open_phase(0).await;
                Err(FlowError::Agent {
                    step: "workflow.js".to_string(),
                    source: FlowHostError(message),
                })
            }
        }
    }
}

/// `WorkflowHost` bridge over `FlowAgentHost`: checkpoint first (M2.1
/// read→miss→run→write, same as the yaml/star runtimes), `phase(title)`
/// calls map onto structured `FlowProgress::PhaseBegin`/`PhaseEnd` pairs
/// (same open/close semantics as the starlark runtime), `log(msg)` stays a
/// free-form `FlowProgress::Log` line.
struct FlowWorkflowHost<'h, H: FlowAgentHost> {
    host: &'h H,
    /// Title of the currently open phase, if any. The eval thread drives
    /// callbacks sequentially, so a plain Mutex is sufficient.
    open_phase: std::sync::Mutex<Option<String>>,
}

impl<H: FlowAgentHost> FlowWorkflowHost<'_, H> {
    /// Close the currently open phase (if any) with the given completed
    /// count; called by the runtime at run teardown.
    async fn close_open_phase(&self, completed_steps: u32) {
        let open = self.open_phase.lock().unwrap().take();
        if let Some(phase_id) = open {
            self.host
                .report_progress(FlowProgress::PhaseEnd {
                    phase_id,
                    completed_steps,
                    total_steps: 1,
                })
                .await;
        }
    }
}

impl<H: FlowAgentHost> WorkflowHost for FlowWorkflowHost<'_, H> {
    /// Checkpoint replay + schema retry delegate to the shared
    /// [`run_agent_text`](super::runtime::run_agent_text) (M2.1/M4.1, same
    /// semantics as the yaml/star carriers); this impl only adapts the
    /// `WorkflowHost` signature and parses the schema transport JSON.
    async fn run_agent(&self, prompt: String, schema_json: Option<String>) -> Result<String, String> {
        let schema = schema_json
            .map(|raw| {
                serde_json::from_str::<serde_json::Value>(&raw)
                    .map_err(|e| format!("agent schema is not valid JSON: {e}"))
            })
            .transpose()?
            .and_then(|value| value.as_object().cloned());
        super::runtime::run_agent_text(self.host, prompt, "agent", schema.as_ref())
            .await
            .map_err(|e| e.to_string())
    }

    async fn report_phase(&self, title: String) {
        // Close the previous phase (1/1) before opening the new one.
        let previous = self.open_phase.lock().unwrap().replace(title.clone());
        if let Some(phase_id) = previous {
            self.host
                .report_progress(FlowProgress::PhaseEnd {
                    phase_id,
                    completed_steps: 1,
                    total_steps: 1,
                })
                .await;
        }
        self.host
            .report_progress(FlowProgress::PhaseBegin { phase_id: title, total_steps: 1 })
            .await;
    }

    async fn report_progress(&self, message: String) {
        self.host.report_progress(FlowProgress::Log { message }).await;
    }
}
