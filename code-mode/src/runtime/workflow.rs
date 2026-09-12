//! Deterministic one-shot workflow script engine for Flow skills (M3.2).
//!
//! [`run_workflow_script`] executes a `workflow.js` source on a dedicated
//! OS thread with its own V8 isolate, bridging the async
//! [`WorkflowHost`] exactly like the flow.star runtime bridges
//! [`FlowAgentHost`](ody_code_mode_protocol): native `agent()` returns a
//! `Promise` backed by a `PromiseResolver`; the request travels over a
//! std channel to the async driver, which runs the real host call
//! concurrently ([`futures::stream::FuturesUnordered`]) and posts the
//! response back; the eval thread performs a microtask checkpoint after
//! every response until the top-level module promise settles. Top-level
//! await is supported (module evaluation returns a promise).
//!
//! Host functions (single source of truth: `core/src/flow/mod.rs` docs):
//! - `agent(prompt)` — one sub-agent, returns a Promise of its final text.
//! - `pipeline(items, each)` / `parallel(fns)` — injected prelude shims
//!   over the native `agent()` promise: render all prompts first (`each` /
//!   `fns` are pure prompt renderers), enforce the batch cap as a hard
//!   error, then fan out concurrently with results bound in item order
//!   by `Promise.all`. Same cap semantics as the yaml/star runtimes.
//! - `phase(msg)` / `log(msg)` — progress lines (fire-and-forget).
//!
//! Determinism: the global `Date` is deleted and `Math.random` is
//! replaced with a throwing function; wall-clock time and randomness are
//! banned outright (decision 7).
//!
//! Result contract: the script binds its final value to a **global**
//! `result` (`var result = ...` or `globalThis.result = ...`; module
//! top-level `const`/`let` do NOT create global properties and will read
//! back as unset). It becomes `outputs["result"]` (`None` when unset).
//!
//! Cancellation safety: dropping the run future drops
//! [`WorkflowRunGuard`], which terminates the isolate — this unwinds
//! even CPU-bound scripts (`while (true) {}`) that a plain channel
//! disconnect could never wake — and disconnects the response channel,
//! which unwinds pump-blocked scripts waiting on a never-settling await.

use std::collections::HashMap;
use std::sync::mpsc as std_mpsc;
use std::thread;

use futures::future::BoxFuture;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use serde_json::Value as JsonValue;

use super::globals::delete_global;
use super::globals::helper_function;
use super::globals::set_global;
use super::initialize_v8;
use super::module_loader;
use super::value::json_to_v8;
use super::value::throw_type_error;
use super::value::v8_value_to_json;
use super::value::value_to_error_text;

/// Host capability injected by the caller (core maps this onto its
/// `FlowAgentHost`; checkpoints live on that side, not here).
pub trait WorkflowHost: Send + Sync {
    /// Run one agent with the fully rendered prompt; Ok carries its final
    /// text, Err carries a displayable failure reason.
    fn run_agent(
        &self,
        prompt: String,
    ) -> impl std::future::Future<Output = Result<String, String>> + Send;

    /// Free-form progress line (`phase()` / `log()`).
    fn report_progress(
        &self,
        _message: String,
    ) -> impl std::future::Future<Output = ()> + Send {
        async {}
    }
}

/// Per-run inputs for one `workflow.js` execution.
#[derive(Debug, Clone)]
pub struct WorkflowRunConfig {
    /// Trigger arguments, exposed to the script as the `args` global.
    /// Should be a JSON object.
    pub args: JsonValue,
    /// Single-batch fan-out cap for `pipeline` / `parallel`; exceeding it
    /// is a hard error (same semantics as `FLOW_BATCH_LIMIT` in core).
    pub batch_limit: usize,
}

/// Request from the eval thread to the async driver.
enum WorkflowRequest {
    Agent { id: String, prompt: String },
    Log { message: String },
}

/// Response from the async driver back to the eval thread.
enum WorkflowResponse {
    Agent { id: String, result: Result<String, String> },
}

/// Isolate slot state for one run. `Send + 'static` by construction.
struct WorkflowState {
    request_tx: std_mpsc::SyncSender<WorkflowRequest>,
    pending_agents: HashMap<String, v8::Global<v8::PromiseResolver>>,
    next_agent_id: u64,
}

/// Abort handle for one [`run_workflow_script`] call. Held by the async
/// driver future; dropping it terminates the isolate so a CPU-bound
/// script cannot keep the eval thread alive. `terminate_execution` is
/// thread-safe and a no-op on an already-finished isolate.
pub struct WorkflowRunGuard {
    handle: v8::IsolateHandle,
}

impl WorkflowRunGuard {
    /// Terminate the isolate now (explicit abort without dropping).
    pub fn abort(&self) {
        self.handle.terminate_execution();
    }
}

impl Drop for WorkflowRunGuard {
    fn drop(&mut self) {
        self.handle.terminate_execution();
    }
}

/// Execute a `workflow.js` source. Returns the script's global `result`
/// as JSON (`Ok(None)` when unset). The source IS the plan; syntax was
/// pre-checked by [`check_workflow_syntax`] at trigger time.
pub async fn run_workflow_script<H: WorkflowHost>(
    source: &str,
    config: &WorkflowRunConfig,
    host: &H,
) -> Result<Option<JsonValue>, String> {
    let (request_tx, request_rx) = std_mpsc::sync_channel::<WorkflowRequest>(64);
    let (response_tx, response_rx) = std_mpsc::channel::<WorkflowResponse>();
    let (guard_tx, guard_rx) = std_mpsc::sync_channel::<WorkflowRunGuard>(1);

    let source = source.to_string();
    let args = config.args.clone();
    let batch_limit = config.batch_limit;
    let eval_thread = thread::Builder::new()
        .name("flow-workflow-js".to_string())
        .spawn(move || {
            run_workflow_thread(source, args, batch_limit, request_tx, response_rx, guard_tx)
        })
        .map_err(|e| format!("failed to spawn workflow.js eval thread: {e}"))?;

    // Held until this future completes or is dropped (turn abort): the
    // guard terminates the isolate, unwinding even CPU-bound scripts.
    let _guard = guard_rx
        .recv()
        .map_err(|_| "workflow.js runtime failed to start".to_string())?;

    // Bridge: std channel -> tokio channel (same pattern as the flow.star
    // runtime in core/src/flow/star.rs).
    let (async_tx, mut async_rx) = tokio::sync::mpsc::unbounded_channel::<WorkflowRequest>();
    let pump = tokio::task::spawn_blocking(move || {
        while let Ok(request) = request_rx.recv() {
            if async_tx.send(request).is_err() {
                break;
            }
        }
    });

    // In-flight agent tasks borrow `host`, so they live in a
    // FuturesUnordered (not a 'static JoinSet) interleaved with request
    // intake via select!. Dropping this future drops them all; pending
    // responses become harmless no-ops on the dead response channel.
    let mut in_flight: FuturesUnordered<BoxFuture<'_, ()>> = FuturesUnordered::new();
    loop {
        tokio::select! {
            request = async_rx.recv() => {
                let Some(request) = request else { break };
                match request {
                    WorkflowRequest::Log { message } => host.report_progress(message).await,
                    WorkflowRequest::Agent { id, prompt } => {
                        let response_tx = response_tx.clone();
                        in_flight.push(Box::pin(async move {
                            let result = host.run_agent(prompt).await;
                            let _ = response_tx.send(WorkflowResponse::Agent { id, result });
                        }));
                    }
                }
            }
            _ = in_flight.next(), if !in_flight.is_empty() => {}
        }
    }
    // The eval thread exited; drain in-flight agents, then collect.
    while in_flight.next().await.is_some() {}
    let _ = pump.await;
    eval_thread
        .join()
        .map_err(|_| "workflow.js eval thread panicked".to_string())?
}

/// Syntax-check a `workflow.js` source without executing it. Runs at
/// trigger time (the skill loaded fine; only triggering needs V8).
pub fn check_workflow_syntax(source: &str) -> Result<(), String> {
    initialize_v8()?;
    let isolate = &mut v8::Isolate::new(v8::CreateParams::default());
    v8::scope!(let scope, isolate);
    let context = v8::Context::new(scope, Default::default());
    let scope = &mut v8::ContextScope::new(scope, context);

    let tc = std::pin::pin!(v8::TryCatch::new(scope));
    let mut tc = tc.init();
    let source_text = v8::String::new(&tc, source)
        .ok_or_else(|| "failed to allocate workflow source".to_string())?;
    let origin = workflow_script_origin(&mut tc)?;
    let mut source = v8::script_compiler::Source::new(source_text, Some(&origin));
    v8::script_compiler::compile_module(&tc, &mut source)
        .map(|_| ())
        .ok_or_else(|| {
            tc.exception()
                .map(|exception| value_to_error_text(&mut tc, exception))
                .unwrap_or_else(|| "unknown workflow.js syntax error".to_string())
        })
}

// ---------------------------------------------------------------------------
// Eval thread (single owner of the isolate).
// ---------------------------------------------------------------------------

fn run_workflow_thread(
    source: String,
    args: JsonValue,
    batch_limit: usize,
    request_tx: std_mpsc::SyncSender<WorkflowRequest>,
    response_rx: std_mpsc::Receiver<WorkflowResponse>,
    guard_tx: std_mpsc::SyncSender<WorkflowRunGuard>,
) -> Result<Option<JsonValue>, String> {
    initialize_v8()?;
    let isolate = &mut v8::Isolate::new(v8::CreateParams::default());
    if guard_tx
        .send(WorkflowRunGuard { handle: isolate.thread_safe_handle() })
        .is_err()
    {
        return Err("workflow driver dropped before the runtime started".to_string());
    }

    v8::scope!(let scope, isolate);
    let context = v8::Context::new(scope, Default::default());
    let scope = &mut v8::ContextScope::new(scope, context);

    scope.set_slot(WorkflowState {
        request_tx,
        pending_agents: HashMap::new(),
        next_agent_id: 1,
    });
    install_workflow_globals(scope, &args, batch_limit)?;

    // Module top-level `var` is module-scoped, NOT a global property, so
    // the `var result = ...` contract cannot read back through the global
    // object directly. Append a same-module tail that mirrors the
    // module-scope `result` binding onto globalThis; `typeof` guards the
    // unset case (a bare `result` reference would throw ReferenceError).
    let source = format!(
        "{source}\n;globalThis.result = typeof result === \"undefined\" ? undefined : result;\n"
    );
    let pending_promise = module_loader::evaluate_main_module(scope, &source)?;
    let Some(pending_promise) = pending_promise else {
        // Synchronous completion (no top-level await).
        return read_result_global(scope);
    };

    // Pump: block for agent responses; each settles one pending promise,
    // then microtasks run. Exits when the top-level promise settles or
    // the driver disconnects (abort). The state check runs BEFORE
    // blocking on recv: a rejection with no agent calls at all (e.g. the
    // batch-limit prelude) settles the module promise during the
    // evaluate_main_module checkpoint, and no response will ever arrive.
    loop {
        scope.perform_microtask_checkpoint();
        let promise = v8::Local::new(scope, &pending_promise);
        match promise.state() {
            v8::PromiseState::Pending => {}
            v8::PromiseState::Fulfilled => return read_result_global(scope),
            v8::PromiseState::Rejected => {
                let exception = promise.result(scope);
                return Err(value_to_error_text(scope, exception));
            }
        }
        match response_rx.recv() {
            Ok(WorkflowResponse::Agent { id, result }) => {
                resolve_agent_response(scope, &id, result)?;
            }
            Err(_) => return Err("workflow aborted: driver disconnected".to_string()),
        }
    }
}

fn install_workflow_globals(
    scope: &mut v8::PinScope<'_, '_>,
    args: &JsonValue,
    batch_limit: usize,
) -> Result<(), String> {
    let global = scope.get_current_context().global(scope);

    // Determinism: ban wall-clock time and randomness outright (decision
    // 7). Deleting `Date` covers `Date.now()` and `new Date()` in every
    // arity; `Math.random` is replaced with a throwing function so the
    // `Math` namespace stays usable for everything else.
    for name in ["console", "Atomics", "SharedArrayBuffer", "WebAssembly", "Date"] {
        delete_global(scope, global, name)?;
    }
    let math_key = v8::String::new(scope, "Math")
        .ok_or_else(|| "failed to allocate Math key".to_string())?;
    let math = global
        .get(scope, math_key.into())
        .and_then(|value| v8::Local::<v8::Object>::try_from(value).ok())
        .ok_or_else(|| "failed to read Math global".to_string())?;
    let random = helper_function(scope, "random", deterministic_ban_callback)?;
    let random_key = v8::String::new(scope, "random")
        .ok_or_else(|| "failed to allocate Math.random key".to_string())?;
    if math.set(scope, random_key.into(), random.into()) != Some(true) {
        return Err("failed to replace Math.random".to_string());
    }

    let args_value = json_to_v8(scope, args).ok_or_else(|| "failed to serialize args".to_string())?;
    set_global(scope, global, "args", args_value)?;

    let agent = helper_function(scope, "agent", agent_callback)?;
    set_global(scope, global, "agent", agent.into())?;
    let phase = helper_function(scope, "phase", log_callback)?;
    set_global(scope, global, "phase", phase.into())?;
    let log = helper_function(scope, "log", log_callback)?;
    set_global(scope, global, "log", log.into())?;

    install_prelude(scope, batch_limit)
}

fn workflow_prelude(batch_limit: usize) -> String {
    // pipeline/parallel are determinism-preserving shims over the native
    // `agent()` promise. batch_limit is a Rust usize formatted via
    // Display (pure decimal digits), so string interpolation is safe.
    format!(
        "globalThis.pipeline = (items, each) => {{\n\
         \x20 const prompts = Array.from(items, each);\n\
         \x20 if (prompts.length > {batch_limit}) {{\n\
         \x20   return Promise.reject(new Error(\"workflow batch of \" + prompts.length + \" exceeds the limit of {batch_limit}\"));\n\
         \x20 }}\n\
         \x20 return Promise.all(prompts.map((prompt) => globalThis.agent(prompt)));\n\
         }};\n\
         globalThis.parallel = (fns) => globalThis.pipeline(fns, (task) => task());\n"
    )
}

/// The prelude is compiled and evaluated as its own module before the
/// user module. Same script_compiler pattern as `check_workflow_syntax`;
/// it has no imports and no TLA, so evaluate cannot legitimately fail.
fn install_prelude(scope: &mut v8::PinScope<'_, '_>, batch_limit: usize) -> Result<(), String> {
    let prelude = workflow_prelude(batch_limit);
    let tc = std::pin::pin!(v8::TryCatch::new(scope));
    let mut tc = tc.init();
    let source_text = v8::String::new(&tc, &prelude)
        .ok_or_else(|| "failed to allocate workflow prelude".to_string())?;
    let origin = workflow_script_origin(&mut tc)?;
    let mut source = v8::script_compiler::Source::new(source_text, Some(&origin));
    let module = v8::script_compiler::compile_module(&tc, &mut source)
        .ok_or_else(|| "failed to compile workflow prelude".to_string())?;
    module
        .instantiate_module(&tc, reject_module_import_callback)
        .ok_or_else(|| "failed to instantiate workflow prelude".to_string())?;
    module
        .evaluate(&tc)
        .ok_or_else(|| "failed to run workflow prelude".to_string())?;
    tc.perform_microtask_checkpoint();
    Ok(())
}

fn reject_module_import_callback<'s>(
    context: v8::Local<'s, v8::Context>,
    specifier: v8::Local<'s, v8::String>,
    _import_attributes: v8::Local<'s, v8::FixedArray>,
    _referrer: v8::Local<'s, v8::Module>,
) -> Option<v8::Local<'s, v8::Module>> {
    v8::callback_scope!(unsafe scope, context);
    let specifier = specifier.to_rust_string_lossy(scope);
    if let Some(message) =
        v8::String::new(scope, &format!("Unsupported import in workflow: {specifier}"))
    {
        scope.throw_exception(message.into());
    } else {
        scope.throw_exception(v8::undefined(scope).into());
    }
    None
}

fn workflow_script_origin<'s>(
    scope: &mut v8::PinScope<'s, '_>,
) -> Result<v8::ScriptOrigin<'s>, String> {
    let resource_name = v8::String::new(scope, "workflow.mjs")
        .ok_or_else(|| "failed to allocate script origin".to_string())?;
    let source_map_url = v8::String::new(scope, "workflow.mjs")
        .ok_or_else(|| "failed to allocate source map url".to_string())?;
    Ok(v8::ScriptOrigin::new(
        scope,
        resource_name.into(),
        0,
        0,
        true,
        0,
        Some(source_map_url.into()),
        true,
        false,
        true,
        None,
    ))
}

fn deterministic_ban_callback(
    scope: &mut v8::PinScope<'_, '_>,
    _args: v8::FunctionCallbackArguments,
    _retval: v8::ReturnValue<v8::Value>,
) {
    throw_type_error(scope, "nondeterministic APIs are disabled in workflow scripts");
}

fn agent_callback(
    scope: &mut v8::PinScope<'_, '_>,
    args: v8::FunctionCallbackArguments,
    mut retval: v8::ReturnValue<v8::Value>,
) {
    let prompt = match args.get(0).to_string(scope) {
        Some(prompt) => prompt.to_rust_string_lossy(scope),
        None => {
            throw_type_error(scope, "agent prompt must be a string");
            return;
        }
    };

    // Borrow discipline mirrors callbacks.rs::tool_callback: create the
    // resolver first, then take the state borrow.
    let Some(resolver) = v8::PromiseResolver::new(scope) else {
        throw_type_error(scope, "failed to create agent promise");
        return;
    };
    let promise = resolver.get_promise(scope);
    let resolver = v8::Global::new(scope, resolver);
    let (request_tx, id) = {
        let Some(state) = scope.get_slot_mut::<WorkflowState>() else {
            throw_type_error(scope, "workflow state unavailable");
            return;
        };
        let id = format!("agent-{}", state.next_agent_id);
        state.next_agent_id = state.next_agent_id.saturating_add(1);
        state.pending_agents.insert(id.clone(), resolver);
        (state.request_tx.clone(), id)
    };

    if request_tx
        .send(WorkflowRequest::Agent { id: id.clone(), prompt })
        .is_err()
    {
        // Driver gone: throw so the awaiting module unwinds immediately.
        if let Some(state) = scope.get_slot_mut::<WorkflowState>() {
            state.pending_agents.remove(&id);
        }
        throw_type_error(scope, "workflow aborted: agent bridge closed");
        return;
    }
    retval.set(promise.into());
}

fn log_callback(
    scope: &mut v8::PinScope<'_, '_>,
    args: v8::FunctionCallbackArguments,
    mut retval: v8::ReturnValue<v8::Value>,
) {
    let message = match args.get(0).to_string(scope) {
        Some(message) => message.to_rust_string_lossy(scope),
        None => {
            throw_type_error(scope, "log message must be a string");
            return;
        }
    };
    if let Some(state) = scope.get_slot::<WorkflowState>() {
        // Fire-and-forget; a closed bridge means the run was aborted.
        let _ = state.request_tx.send(WorkflowRequest::Log { message });
    }
    retval.set(v8::undefined(scope).into());
}

/// Settle one pending agent promise on the eval thread. Modeled on
/// module_loader::resolve_tool_response.
fn resolve_agent_response(
    scope: &mut v8::PinScope<'_, '_>,
    id: &str,
    result: Result<String, String>,
) -> Result<(), String> {
    let resolver = scope
        .get_slot_mut::<WorkflowState>()
        .and_then(|state| state.pending_agents.remove(id))
        .ok_or_else(|| format!("unknown agent call `{id}`"))?;

    let tc = std::pin::pin!(v8::TryCatch::new(scope));
    let mut tc = tc.init();
    let resolver = v8::Local::new(&tc, resolver);
    match result {
        Ok(text) => {
            let value = json_to_v8(&mut tc, &JsonValue::String(text))
                .ok_or_else(|| "failed to serialize agent response".to_string())?;
            resolver.resolve(&tc, value);
        }
        Err(error_text) => {
            let message = v8::String::new(&tc, &error_text)
                .ok_or_else(|| "failed to allocate agent error".to_string())?;
            resolver.reject(&tc, message.into());
        }
    }
    if tc.has_caught() {
        return Err(tc
            .exception()
            .map(|exception| value_to_error_text(&mut tc, exception))
            .unwrap_or_else(|| "unknown workflow exception".to_string()));
    }
    Ok(())
}

/// Read the script's global `result` (`None` when unset). The engine
/// appends a tail that mirrors the module-scope `result` binding here
/// (see `run_workflow_thread`), so `var`/`let`/`const`/`globalThis`
/// assignments all land on the same property.
fn read_result_global(scope: &mut v8::PinScope<'_, '_>) -> Result<Option<JsonValue>, String> {
    let global = scope.get_current_context().global(scope);
    let key = v8::String::new(scope, "result")
        .ok_or_else(|| "failed to allocate result key".to_string())?;
    let Some(value) = global.get(scope, key.into()) else {
        return Ok(None);
    };
    if value.is_undefined() {
        return Ok(None);
    }
    match v8_value_to_json(scope, value) {
        Ok(Some(value)) => Ok(Some(value)),
        Ok(None) => Err("workflow `result` global is not JSON-serializable".to_string()),
        Err(error_text) => Err(error_text),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::time::Duration;

    use super::*;

    /// Records prompts/progress; canned responses in order, falling back
    /// to `done: <prompt>` echoes. `invert_completion` makes later
    /// prompts finish sooner (order-binding probe).
    #[derive(Default)]
    struct MockWorkflowHost {
        prompts: Mutex<Vec<String>>,
        responses: Mutex<VecDeque<Result<String, String>>>,
        progress: Mutex<Vec<String>>,
        invert_completion: bool,
    }

    impl WorkflowHost for MockWorkflowHost {
        async fn run_agent(&self, prompt: String) -> Result<String, String> {
            self.prompts.lock().unwrap().push(prompt.clone());
            if self.invert_completion {
                let index: usize = prompt.strip_prefix("item ").unwrap().parse().unwrap();
                for _ in 0..(3 - index) {
                    tokio::task::yield_now().await;
                }
                return Ok(format!("r{index}"));
            }
            match self.responses.lock().unwrap().pop_front() {
                Some(result) => result,
                None => Ok(format!("done: {prompt}")),
            }
        }

        async fn report_progress(&self, message: String) {
            self.progress.lock().unwrap().push(message);
        }
    }

    fn config(args: serde_json::Value) -> WorkflowRunConfig {
        WorkflowRunConfig { args, batch_limit: 8 }
    }

    #[tokio::test]
    async fn smoke_agent_tla_args_and_result_contract() {
        let host = MockWorkflowHost::default();
        let source = r#"
const first = await agent("write about " + args.topic);
var result = { first, second: await agent("summarize " + first) };
"#;
        let outcome = run_workflow_script(source, &config(serde_json::json!({"topic": "otters"})), &host)
            .await
            .expect("smoke run succeeds");
        assert_eq!(
            outcome,
            Some(serde_json::json!({
                "first": "done: write about otters",
                "second": "done: summarize done: write about otters"
            }))
        );
        assert_eq!(
            *host.prompts.lock().unwrap(),
            vec!["write about otters".to_string(), "summarize done: write about otters".to_string()]
        );
    }

    #[tokio::test]
    async fn pipeline_binds_results_in_item_order() {
        let host = MockWorkflowHost { invert_completion: true, ..Default::default() };
        let outcome = run_workflow_script(
            r#"var result = await pipeline(["1", "2", "3"], (i) => "item " + i);"#,
            &config(serde_json::json!({})),
            &host,
        )
        .await
        .unwrap();
        // Later items complete sooner; order must hold.
        assert_eq!(outcome, Some(serde_json::json!(["r1", "r2", "r3"])));
    }

    #[tokio::test]
    async fn parallel_fans_out_and_phase_reports_progress() {
        let host = MockWorkflowHost::default();
        let outcome = run_workflow_script(
            r#"
phase("verify");
var result = await parallel([() => "Review", () => "Test"]);
"#,
            &config(serde_json::json!({})),
            &host,
        )
        .await
        .unwrap();
        assert_eq!(outcome, Some(serde_json::json!(["done: Review", "done: Test"])));
        assert_eq!(*host.progress.lock().unwrap(), vec!["verify".to_string()]);
    }

    #[tokio::test]
    async fn determinism_bans_date_and_math_random() {
        let host = MockWorkflowHost::default();
        let outcome = run_workflow_script(
            "var result = typeof Date;",
            &config(serde_json::json!({})),
            &host,
        )
        .await
        .unwrap();
        assert_eq!(outcome, Some(serde_json::json!("undefined")));

        let err = run_workflow_script(
            "Math.random();",
            &config(serde_json::json!({})),
            &host,
        )
        .await
        .expect_err("Math.random must be banned");
        assert!(err.contains("nondeterministic APIs are disabled"), "{err}");
    }

    #[tokio::test]
    async fn agent_failure_rejects_the_run() {
        let host = MockWorkflowHost {
            responses: Mutex::new(VecDeque::from(vec![Err("boom".to_string())])),
            ..Default::default()
        };
        let err = run_workflow_script(
            r#"await agent("explode");"#,
            &config(serde_json::json!({})),
            &host,
        )
        .await
        .expect_err("agent failure must fail the run");
        assert!(err.contains("boom"), "{err}");
    }

    #[tokio::test]
    async fn batch_limit_is_a_hard_error() {
        let host = MockWorkflowHost::default();
        let small = WorkflowRunConfig { args: serde_json::json!({}), batch_limit: 2 };
        let err = run_workflow_script(
            r#"var result = await pipeline(["a", "b", "c"], (i) => "item " + i);"#,
            &small,
            &host,
        )
        .await
        .expect_err("batch over the cap must fail");
        assert!(err.contains("exceeds the limit of 2"), "{err}");
        assert!(host.prompts.lock().unwrap().is_empty(), "no agent may spawn past the cap");
    }

    #[tokio::test]
    async fn unset_result_yields_none() {
        let host = MockWorkflowHost::default();
        let outcome = run_workflow_script(
            r#"await agent("side effect only");"#,
            &config(serde_json::json!({})),
            &host,
        )
        .await
        .unwrap();
        assert_eq!(outcome, None);
    }

    #[test]
    fn check_workflow_syntax_accepts_valid_and_rejects_broken_source() {
        assert!(check_workflow_syntax("var result = 1;\n").is_ok());
        let err = check_workflow_syntax("export const x = ;\n").expect_err("broken source must fail");
        assert!(!err.is_empty());
    }

    #[tokio::test]
    async fn dropping_the_run_future_terminates_a_hanging_script() {
        let host = MockWorkflowHost::default();
        let handle = tokio::spawn(async move {
            // CPU-bound stretch (execute-blocked) then a never-settling
            // await (pump-blocked): the abort path must cover both.
            run_workflow_script(
                "for (let i = 0; i < 1e6; i++) {}\nawait new Promise(() => {});\nvar result = 1;\n",
                &config(serde_json::json!({})),
                &host,
            )
            .await
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        handle.abort();
        let joined = tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("aborted workflow run must finish within 2s");
        assert!(joined.is_err() || joined.unwrap().is_err());

        // The runtime must stay usable after the abort.
        let host = MockWorkflowHost::default();
        let outcome = run_workflow_script("var result = 41 + 1;\n", &config(serde_json::json!({})), &host)
            .await
            .unwrap();
        assert_eq!(outcome, Some(serde_json::json!(42)));
    }
}
