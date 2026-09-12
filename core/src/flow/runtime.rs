//! Declarative executor for the `flow.yaml` runtime (M1.1).
//!
//! Pure in-crate state machine over [`ody_core_skills::FlowPlan`]: phases
//! run sequentially; steps within a phase run sequentially; `pipeline` fans
//! out one agent per item; `parallel` runs its children concurrently. The
//! semantics are defined in the module docs on `crate::flow` (single source
//! of truth for all runtimes); this file is the yaml implementation.

use futures::future::BoxFuture;
use futures::future::select_all;
use ody_core_skills::FlowPlan;
use ody_core_skills::FlowStep;
use serde_json::Value;

use super::FLOW_BATCH_LIMIT;
use super::FlowAgentHost;
use super::FlowContext;
use super::FlowError;
use super::FlowOutcome;
use super::FlowProgress;
use super::FlowRuntime;
use super::interp::TemplateContext;
use super::interp::render_template;

/// The `flow.yaml` runtime: validates sources through the shared loader
/// parser and executes plans via [`execute_plan`].
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct YamlFlowRuntime;

impl YamlFlowRuntime {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl FlowRuntime for YamlFlowRuntime {
    fn supported_artifact(&self) -> &'static str {
        "flow.yaml"
    }

    fn validate(&self, source: &str) -> Result<FlowPlan, FlowError> {
        ody_core_skills::parse_flow_plan(source)
            .map_err(|err| FlowError::Parse { reason: err.to_string() })
    }

    async fn run<H: FlowAgentHost>(
        &self,
        plan: FlowPlan,
        ctx: FlowContext,
        host: &H,
    ) -> Result<FlowOutcome, FlowError> {
        execute_plan(plan, ctx, host).await
    }
}

pub(crate) async fn execute_plan<H: FlowAgentHost>(
    plan: FlowPlan,
    ctx: FlowContext,
    host: &H,
) -> Result<FlowOutcome, FlowError> {
    // `args` is a reserved root binding; steps may not re-bind it.
    let mut bindings = serde_json::Map::new();
    bindings.insert("args".to_string(), Value::Object(ctx.args));
    for phase in &plan.phases {
        let total_steps = phase.steps.len() as u32;
        host.report_progress(FlowProgress::PhaseBegin {
            phase_id: phase.id.clone(),
            total_steps,
        })
        .await;
        let mut completed_steps = 0u32;
        let phase_result = run_steps(
            &phase.steps,
            &mut bindings,
            host,
            &format!("phase '{}'", phase.id),
            &phase.id,
            &mut completed_steps,
        )
        .await;
        // PhaseEnd is reported on the failure path too (completed_steps <
        // total_steps signals the failure; A9 in the M1 execution plan).
        host.report_progress(FlowProgress::PhaseEnd {
            phase_id: phase.id.clone(),
            completed_steps,
            total_steps,
        })
        .await;
        phase_result?;
    }
    bindings.remove("args");
    Ok(FlowOutcome { outputs: bindings })
}

/// Run steps sequentially, binding each `output` into `bindings`. Returns
/// every step's primary result, aligned with `steps`. `completed_steps`
/// counts top-level steps finished so far and feeds the PhaseEnd event even
/// when a step fails; step completion progress is reported here (nested
/// parallel children are not counted — only top-level phase steps are).
async fn run_steps<H: FlowAgentHost>(
    steps: &[FlowStep],
    bindings: &mut serde_json::Map<String, Value>,
    host: &H,
    scope: &str,
    phase_id: &str,
    completed_steps: &mut u32,
) -> Result<Vec<Value>, FlowError> {
    let total_steps = steps.len() as u32;
    let mut results = Vec::with_capacity(steps.len());
    for (index, step) in steps.iter().enumerate() {
        let label = format!("{scope} step {} ({})", index + 1, step_kind(step));
        results.push(run_step(step, bindings, host, &label).await?);
        *completed_steps += 1;
        host.report_progress(FlowProgress::StepCompleted {
            phase_id: phase_id.to_string(),
            step_index: *completed_steps,
            total_steps,
        })
        .await;
    }
    Ok(results)
}

fn step_kind(step: &FlowStep) -> &'static str {
    match step {
        FlowStep::Agent { .. } => "agent",
        FlowStep::Pipeline { .. } => "pipeline",
        FlowStep::Parallel { .. } => "parallel",
    }
}

// `run_step` returns an explicit `BoxFuture` instead of being a plain
// `async fn`: it recurses (parallel children may themselves be parallel),
// and rustc cannot prove a recursive async fn's future `Send` through the
// self-referential async block. Boxing gives the recursive call a named
// `Send` type, breaking the auto-trait cycle.
#[allow(clippy::too_many_lines)]
fn run_step<'a, H: FlowAgentHost>(
    step: &'a FlowStep,
    bindings: &'a mut serde_json::Map<String, Value>,
    host: &'a H,
    label: &'a str,
) -> BoxFuture<'a, Result<Value, FlowError>> {
    Box::pin(async move {
        match step {
        FlowStep::Agent { agent, output } => {
            let prompt = render_template(agent, &TemplateContext { bindings, item: None }, label)?;
            let value = run_one_agent(host, prompt, label).await?;
            if let Some(name) = output {
                bind_output(bindings, name, value.clone(), label)?;
            }
            Ok(value)
        }
        FlowStep::Pipeline { pipeline, each, output } => {
            let items = evaluate_pipeline_items(pipeline, bindings, label)?;
            if items.len() > FLOW_BATCH_LIMIT {
                return Err(FlowError::LimitExceeded {
                    limit: FLOW_BATCH_LIMIT,
                    actual: items.len(),
                    step: label.to_string(),
                });
            }
            // Render every item prompt upfront against the shared snapshot so
            // all items observe identical bindings regardless of completion order.
            let mut prompts = Vec::with_capacity(items.len());
            for item in &items {
                prompts.push(render_template(
                    each,
                    &TemplateContext { bindings, item: Some(item) },
                    label,
                )?);
            }
            let results = run_agent_batch(prompts, host, label).await?;
            let value = Value::Array(results);
            if let Some(name) = output {
                bind_output(bindings, name, value.clone(), label)?;
            }
            Ok(value)
        }
        FlowStep::Parallel { parallel, output } => {
            if parallel.len() > FLOW_BATCH_LIMIT {
                return Err(FlowError::LimitExceeded {
                    limit: FLOW_BATCH_LIMIT,
                    actual: parallel.len(),
                    step: label.to_string(),
                });
            }
            // Children observe the binding snapshot taken before the group
            // starts; their `output`s merge only after all children succeed.
            let snapshot = bindings.clone();
            let mut merged: Vec<Option<(Value, serde_json::Map<String, Value>)>> =
                vec![None; parallel.len()];
            let mut pending: Vec<
                BoxFuture<'_, Result<(usize, Value, serde_json::Map<String, Value>), FlowError>>,
            > = Vec::with_capacity(parallel.len());
            for (index, child) in parallel.iter().enumerate() {
                let child_label = format!("{label} child {}", index + 1);
                let child_snapshot = snapshot.clone();
                pending.push(Box::pin(async move {
                    let mut child_bindings = child_snapshot;
                    let value = run_step(child, &mut child_bindings, host, &child_label).await?;
                    Ok((index, value, child_bindings))
                }));
            }
            while !pending.is_empty() {
                let (result, _, remaining) = select_all(pending).await;
                pending = remaining;
                // On error the remaining futures are dropped here, which
                // cancels in-flight children (short-circuit).
                let (index, value, child_bindings) = result?;
                merged[index] = Some((value, child_bindings));
            }
            let mut results = Vec::with_capacity(merged.len());
            for (index, slot) in merged.into_iter().enumerate() {
                let (value, child_bindings) =
                    slot.expect("parallel child results are complete once all futures resolve");
                for (name, bound) in child_bindings {
                    // Merge only bindings the child added on top of the
                    // pre-group snapshot; inherited names (including `args`
                    // and earlier step outputs like `gdd`) are not child
                    // outputs and must not be re-bound into the parent.
                    if snapshot.contains_key(&name) {
                        continue;
                    }
                    bind_output(
                        bindings,
                        &name,
                        bound,
                        &format!("{label} child {}", index + 1),
                    )?;
                }
                results.push(value);
            }
            let value = Value::Array(results);
            if let Some(name) = output {
                bind_output(bindings, name, value.clone(), label)?;
            }
            Ok(value)
        }
    }
    })
}

/// A pipeline expression is a template that must evaluate to a JSON array.
/// `${{ gdd.mechanics }}` renders to JSON text and parses back; literal
/// arrays (`["a", "b"]`) pass through unchanged.
fn evaluate_pipeline_items(
    pipeline: &str,
    bindings: &serde_json::Map<String, Value>,
    label: &str,
) -> Result<Vec<Value>, FlowError> {
    let rendered = render_template(pipeline, &TemplateContext { bindings, item: None }, label)?;
    let value: Value =
        serde_json::from_str(rendered.trim()).map_err(|_| FlowError::NotAnArray {
            expr: pipeline.to_string(),
            step: label.to_string(),
        })?;
    value.as_array().cloned().ok_or_else(|| FlowError::NotAnArray {
        expr: pipeline.to_string(),
        step: label.to_string(),
    })
}

/// Run one agent per prompt concurrently. Results are returned in prompt
/// order. On the first failure all remaining futures are dropped (which
/// cancels them) and the error propagates.
async fn run_agent_batch<H: FlowAgentHost>(
    prompts: Vec<String>,
    host: &H,
    label: &str,
) -> Result<Vec<Value>, FlowError> {
    // NOTE: a for-loop is required here — building this vec via
    // `.map(...).collect()` defeats the `+ Send` bound on
    // `FlowAgentHost::run_agent` during coercion inference (rustc resolves
    // the boxed async block as non-Send in iterator combinators).
    let mut pending: Vec<BoxFuture<'_, Result<(usize, Value), FlowError>>> =
        Vec::with_capacity(prompts.len());
    for (index, prompt) in prompts.into_iter().enumerate() {
        pending.push(Box::pin(async move {
            let value = run_one_agent(host, prompt, label).await?;
            Ok((index, value))
        }));
    }
    let mut results: Vec<Option<Value>> = vec![None; pending.len()];
    while !pending.is_empty() {
        let (result, _, remaining) = select_all(pending).await;
        pending = remaining;
        let (index, value) = result?;
        results[index] = Some(value);
    }
    Ok(results
        .into_iter()
        .map(|slot| slot.expect("batch results are complete once every future resolves"))
        .collect())
}

async fn run_one_agent<H: FlowAgentHost>(
    host: &H,
    prompt: String,
    label: &str,
) -> Result<Value, FlowError> {
    if let Some(cached) = host.checkpoint_read(&prompt).await {
        return Ok(parse_agent_output(&cached));
    }
    let raw = host
        .run_agent(prompt.clone())
        .await
        .map_err(|source| FlowError::Agent {
            step: label.to_string(),
            source,
        })?;
    host.checkpoint_write(&prompt, &raw).await;
    Ok(parse_agent_output(&raw))
}

/// Agent results are stored as parsed JSON when possible, else as the raw string.
fn parse_agent_output(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string()))
}

fn bind_output(
    bindings: &mut serde_json::Map<String, Value>,
    name: &str,
    value: Value,
    label: &str,
) -> Result<(), FlowError> {
    if name == "args" || name == "item" {
        return Err(FlowError::ReservedBinding { name: name.to_string(), step: label.to_string() });
    }
    if bindings.contains_key(name) {
        return Err(FlowError::DuplicateBinding { name: name.to_string(), step: label.to_string() });
    }
    bindings.insert(name.to_string(), value);
    Ok(())
}
