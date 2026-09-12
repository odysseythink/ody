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
        let result = run_workflow_script(
            &source,
            &WorkflowRunConfig {
                args: serde_json::Value::Object(ctx.args),
                batch_limit: FLOW_BATCH_LIMIT,
            },
            &FlowWorkflowHost { host },
        )
        .await
        .map_err(|message| FlowError::Agent {
            step: "workflow.js".to_string(),
            source: FlowHostError(message),
        })?;

        let mut outputs = serde_json::Map::new();
        if let Some(value) = result {
            outputs.insert("result".to_string(), value);
        }
        Ok(FlowOutcome { outputs })
    }
}

/// `WorkflowHost` bridge over `FlowAgentHost`: checkpoint first (M2.1
/// read→miss→run→write, same as the yaml/star runtimes), progress lines
/// map onto `FlowProgress::Log`.
struct FlowWorkflowHost<'h, H: FlowAgentHost> {
    host: &'h H,
}

impl<H: FlowAgentHost> WorkflowHost for FlowWorkflowHost<'_, H> {
    async fn run_agent(&self, prompt: String) -> Result<String, String> {
        if let Some(cached) = self.host.checkpoint_read(&prompt).await {
            return Ok(cached);
        }
        let raw = self.host.run_agent(prompt.clone()).await.map_err(|e| e.0)?;
        self.host.checkpoint_write(&prompt, &raw).await;
        Ok(raw)
    }

    async fn report_progress(&self, message: String) {
        self.host.report_progress(FlowProgress::Log { message }).await;
    }
}
