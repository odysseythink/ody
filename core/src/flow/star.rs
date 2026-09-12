//! The `flow.star` Starlark runtime (M3.1).
//!
//! Starlark evaluation is synchronous, so the async [`FlowAgentHost`] bridge
//! works exactly as validated by the M3 spike: the whole eval runs on a
//! dedicated OS thread (`starlark::environment::Module` / `Evaluator` are not
//! `Send`); native `agent()` sends a request over a std channel and blocks on
//! a oneshot for the result while this `run` future (the driver) executes the
//! real async host call. Cancellation safety: dropping the `run` future drops
//! the driver side (request pump + reply channels), which makes every blocked
//! native call fail and the eval thread unwind promptly.
//!
//! Host functions (single source of truth: the module docs on `crate::flow`):
//! - `agent(prompt)` — one sub-agent, returns its final text.
//! - `pipeline(items, each)` — `each` is a top-level function mapping one
//!   item to a prompt string; all prompts are rendered synchronously, then
//!   fanned out concurrently (batch cap [`FLOW_BATCH_LIMIT`]); results bind
//!   in item order. `each` must be pure: calling `agent()` inside it runs
//!   sequentially (Starlark is single-threaded) — use top-level `agent()`
//!   calls for sequential work instead.
//! - `parallel(fns)` — `fns` is a list of zero-arg top-level functions, each
//!   returning a prompt string; same fan-out semantics as `pipeline`.
//! - `phase(title)` / `log(msg)` — progress lines via
//!   [`FlowProgress::Log`].
//!
//! Contract: the script binds its final value to the module-global `result`;
//! it becomes `FlowOutcome.outputs["result"]` (absent if unset).

use std::sync::mpsc;
use std::thread;

use anyhow::Context as _;
use futures::future::BoxFuture;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use starlark::any::ProvidesStaticType;
use starlark::environment::GlobalsBuilder;
use starlark::environment::Module;
use starlark::eval::Evaluator;
use starlark::starlark_module;
use starlark::syntax::AstModule;
use starlark::syntax::Dialect;
use starlark::values::AllocValue;
use starlark::values::Heap;
use starlark::values::Value;
use starlark::values::list::UnpackList;
use starlark::values::none::NoneType;
use tokio::sync::oneshot;

use super::FLOW_BATCH_LIMIT;
use super::FlowAgentHost;
use super::FlowContext;
use super::FlowError;
use super::FlowHostError;
use super::FlowOutcome;
use super::FlowPlanSource;
use super::FlowProgress;
use super::FlowRuntime;

/// The `flow.star` runtime: validates syntax through `AstModule::parse` and
/// executes via the thread + channel bridge described above.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct StarlarkFlowRuntime;

impl StarlarkFlowRuntime {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl FlowRuntime for StarlarkFlowRuntime {
    fn supported_artifact(&self) -> &'static str {
        "flow.star"
    }

    fn validate(&self, source: &str) -> Result<FlowPlanSource, FlowError> {
        parse_star_module(source)
            .map(|_| FlowPlanSource::Starlark(source.to_string()))
            .map_err(|reason| FlowError::Parse { reason })
    }

    async fn run<H: FlowAgentHost>(
        &self,
        plan: FlowPlanSource,
        ctx: FlowContext,
        host: &H,
    ) -> Result<FlowOutcome, FlowError> {
        let FlowPlanSource::Starlark(source) = plan else {
            return Err(FlowError::Parse {
                reason: "starlark runtime received a non-starlark plan".to_string(),
            });
        };
        run_star(&source, ctx, host).await
    }
}

/// Parse a Starlark module, returning a displayable reason on failure.
/// Shared by `validate` (eager, trigger time) and `run` (re-parse).
fn parse_star_module(source: &str) -> Result<AstModule, String> {
    let mut dialect = Dialect::Extended;
    dialect.enable_f_strings = true;
    AstModule::parse("flow.star", source.to_string(), &dialect)
        .map_err(|err| format!("invalid flow.star: {err}"))
}

// ---------------------------------------------------------------------------
// Bridge between the Starlark thread and the async driver.
// ---------------------------------------------------------------------------

/// One request from the eval thread to the async driver.
enum BridgeRequest {
    /// Run one agent; the reply channel delivers the final text.
    Agent {
        prompt: String,
        reply: oneshot::Sender<Result<String, String>>,
    },
    /// Free-form progress line (`phase()` / `log()`).
    Log { message: String },
}

/// Carried to native functions through `eval.extra`.
#[derive(ProvidesStaticType)]
struct Bridge {
    tx: mpsc::SyncSender<BridgeRequest>,
}

fn bridge<'v, 'e>(eval: &Evaluator<'v, 'e, '_>) -> anyhow::Result<&'e Bridge> {
    eval.extra
        .and_then(|extra| extra.downcast_ref::<Bridge>())
        .context("flow.star bridge missing")
}

fn err(msg: impl Into<String>) -> anyhow::Error {
    anyhow::anyhow!(msg.into())
}

/// Abort-aware send: once the driver is gone every blocked native call must
/// fail fast so the eval thread unwinds instead of hanging.
fn send_request(bridge: &Bridge, request: BridgeRequest) -> anyhow::Result<()> {
    bridge
        .tx
        .send(request)
        .map_err(|_| err("flow aborted: agent bridge closed"))
}

fn await_reply(rx: oneshot::Receiver<Result<String, String>>) -> anyhow::Result<String> {
    rx.blocking_recv()
        .map_err(|_| err("flow aborted: agent result channel closed"))?
        .map_err(err)
}

/// Render every item through `each`, then fan the prompts out as one batch.
/// Results come back in item order because the replies are collected in
/// prompt order after all sends.
fn fan_out_via_bridge(
    bridge: &Bridge,
    prompts: Vec<String>,
    limit_step: &str,
) -> anyhow::Result<Vec<String>> {
    if prompts.len() > FLOW_BATCH_LIMIT {
        return Err(anyhow::Error::new(FlowError::LimitExceeded {
            limit: FLOW_BATCH_LIMIT,
            actual: prompts.len(),
            step: limit_step.to_string(),
        }));
    }
    let mut replies = Vec::with_capacity(prompts.len());
    for prompt in prompts {
        let (reply, rx) = oneshot::channel();
        send_request(bridge, BridgeRequest::Agent { prompt, reply })?;
        replies.push(rx);
    }
    replies.into_iter().map(await_reply).collect()
}

/// Render `call(item)` for every item and collect prompt strings.
fn render_prompts<'v>(
    eval: &mut Evaluator<'v, '_, '_>,
    each: Value<'v>,
    items: Vec<Value<'v>>,
    what: &str,
) -> anyhow::Result<Vec<String>> {
    let mut prompts = Vec::with_capacity(items.len());
    for item in items {
        let rendered = eval
            .eval_function(each, &[item], &[])
            .map_err(|e| err(format!("{what} render: {e}")))?;
        let prompt = rendered
            .unpack_str()
            .ok_or_else(|| err(format!("{what} must return a prompt string")))?;
        prompts.push(prompt.to_string());
    }
    Ok(prompts)
}

/// `json.decode` / `json.encode` for flow scripts. Starlark's own
/// `LibraryExtension::Json` is not publicly reachable from outside the
/// starlark crate (private `stdlib` module), so the flow runtime provides
/// its own namespace on top of the serde bridge in this file.
#[starlark_module]
fn flow_json(builder: &mut GlobalsBuilder) {
    /// Parse a JSON string into a Starlark value.
    fn decode<'v>(
        #[starlark(require = pos)] source: &str,
        heap: Heap<'v>,
    ) -> anyhow::Result<Value<'v>> {
        let value: serde_json::Value =
            serde_json::from_str(source).map_err(|e| err(format!("json.decode: {e}")))?;
        json_to_starlark(heap, &value)
    }

    /// Serialize a Starlark value to a JSON string.
    fn encode(#[starlark(require = pos)] value: Value) -> anyhow::Result<String> {
        let value = value
            .to_json_value()
            .map_err(|e| err(format!("json.encode: {e}")))?;
        serde_json::to_string(&value).map_err(|e| err(format!("json.encode: {e}")))
    }
}

#[starlark_module]
fn flow_builtins(builder: &mut GlobalsBuilder) {
    // NOTE: `json` (decode/encode) is registered as a namespace alongside
    // these builtins (see `flow_json`) — agent() returns text, and scripts
    // parse JSON replies with json.decode (the yaml runtime auto-parses).
    /// Run one sub-agent with the given prompt; returns its final text.
    fn agent(prompt: &str, eval: &mut Evaluator) -> anyhow::Result<String> {
        let bridge = bridge(eval)?;
        let (reply, rx) = oneshot::channel();
        send_request(bridge, BridgeRequest::Agent { prompt: prompt.to_string(), reply })?;
        await_reply(rx)
    }

    /// Fan out one agent per item. `each` maps one item to a prompt string
    /// (pure; it must not call `agent()`).
    fn pipeline<'v>(
        items: UnpackList<Value<'v>>,
        each: Value<'v>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Vec<String>> {
        let bridge = bridge(eval)?;
        let prompts = render_prompts(eval, each, items.items, "pipeline `each`")?;
        fan_out_via_bridge(bridge, prompts, "pipeline")
    }

    /// Fan out one agent per zero-arg function, each returning a prompt.
    fn parallel<'v>(
        fns: UnpackList<Value<'v>>,
        eval: &mut Evaluator<'v, '_, '_>,
    ) -> anyhow::Result<Vec<String>> {
        let bridge = bridge(eval)?;
        let mut prompts = Vec::new();
        for function in fns.items {
            let rendered = eval
                .eval_function(function, &[], &[])
                .map_err(|e| err(format!("parallel task: {e}")))?;
            let prompt = rendered
                .unpack_str()
                .ok_or_else(|| err("parallel task must return a prompt string"))?;
            prompts.push(prompt.to_string());
        }
        fan_out_via_bridge(bridge, prompts, "parallel")
    }

    /// Free-form progress line.
    fn phase(msg: &str, eval: &mut Evaluator) -> anyhow::Result<NoneType> {
        log_impl(msg, eval)
    }

    /// Free-form progress line.
    fn log(msg: &str, eval: &mut Evaluator) -> anyhow::Result<NoneType> {
        log_impl(msg, eval)
    }
}

fn log_impl(msg: &str, eval: &mut Evaluator) -> anyhow::Result<NoneType> {
    let bridge = bridge(eval)?;
    // Fire-and-forget; a closed bridge means the run was aborted.
    let _ = bridge.tx.send(BridgeRequest::Log { message: msg.to_string() });
    Ok(NoneType)
}

// ---------------------------------------------------------------------------
// serde_json <-> Starlark value conversion (args in, result out).
// ---------------------------------------------------------------------------

fn json_to_starlark<'v>(heap: Heap<'v>, value: &serde_json::Value) -> anyhow::Result<Value<'v>> {
    Ok(match value {
        serde_json::Value::Null => Value::new_none(),
        serde_json::Value::Bool(b) => Value::new_bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                heap.alloc(i)
            } else {
                heap.alloc(n.as_f64().context("args number out of range")?)
            }
        }
        serde_json::Value::String(s) => heap.alloc(s.as_str()),
        serde_json::Value::Array(items) => {
            let values: anyhow::Result<Vec<Value<'v>>> =
                items.iter().map(|item| json_to_starlark(heap, item)).collect();
            values?.alloc_value(heap)
        }
        serde_json::Value::Object(map) => {
            let mut dict = starlark::collections::SmallMap::new();
            for (key, item) in map {
                dict.insert_hashed(
                    heap.alloc_str(key.as_str()).get_hashed(),
                    json_to_starlark(heap, item)?,
                );
            }
            heap.alloc(dict)
        }
    })
}

// ---------------------------------------------------------------------------
// Execution driver.
// ---------------------------------------------------------------------------

/// Execute a validated `flow.star` source. See the module docs for the
/// thread/bridge design and cancellation story.
async fn run_star<H: FlowAgentHost>(
    source: &str,
    ctx: FlowContext,
    host: &H,
) -> Result<FlowOutcome, FlowError> {
    let ast = parse_star_module(source).map_err(|reason| FlowError::Parse { reason })?;
    // Standard globals + flow builtins + the `json` namespace (see
    // [`flow_json`]): agent results are text; the idiom for structured
    // replies is `json.decode(agent(...))`.
    let mut builder = GlobalsBuilder::standard();
    builder.namespace("json", flow_json);
    let globals = builder.with(flow_builtins).build();

    // The request channel carries agent/log requests from the eval thread.
    // The eval thread owns the sender (inside `Bridge`); when it unwinds the
    // sender drops and the pump below exits.
    let (tx, rx) = mpsc::sync_channel::<BridgeRequest>(64);
    let args = serde_json::Value::Object(ctx.args);

    let eval_thread = thread::Builder::new()
        .name("flow-star".to_string())
        .spawn(move || -> Result<Option<serde_json::Value>, String> {
            Module::with_temp_heap(|module| {
                eval_module_with_bridge(module, ast, &globals, tx, &args)
            })
        })
        .map_err(|e| FlowError::Agent {
            step: "flow.star".to_string(),
            source: FlowHostError(format!("failed to spawn eval thread: {e}")),
        })?;

    // Pump requests from the std channel onto the async side. Exits when the
    // eval thread drops the sender (normal exit or abort unwind).
    let (async_tx, mut async_rx) = tokio::sync::mpsc::unbounded_channel::<BridgeRequest>();
    let pump = tokio::task::spawn_blocking(move || {
        while let Ok(request) = rx.recv() {
            if async_tx.send(request).is_err() {
                break;
            }
        }
    });

    // In-flight agent tasks borrow `host`, so they live in a FuturesUnordered
    // (not a 'static JoinSet) interleaved with request intake via select!.
    // Dropping the run future drops them all, closing every pending reply
    // channel, which unwinds the eval thread's blocked native calls.
    let mut in_flight: FuturesUnordered<BoxFuture<'_, ()>> = FuturesUnordered::new();
    loop {
        tokio::select! {
            request = async_rx.recv() => {
                let Some(request) = request else { break };
                match request {
                    BridgeRequest::Log { message } => {
                        host.report_progress(FlowProgress::Log { message }).await;
                    }
                    BridgeRequest::Agent { prompt, reply } => {
                        in_flight.push(Box::pin(async move {
                            let result = run_one_agent_checked(host, prompt).await;
                            let _ = reply.send(result);
                        }));
                    }
                }
            }
            _ = in_flight.next(), if !in_flight.is_empty() => {}
        }
    }
    // The eval thread finished (it dropped the sender); no new requests can
    // arrive. Drain in-flight agents — replies to an already-unwound eval
    // are harmless — then collect the eval result.
    while in_flight.next().await.is_some() {}
    let _ = pump.await;

    let result = tokio::task::spawn_blocking(move || eval_thread.join())
        .await
        .map_err(|e| FlowError::Agent {
            step: "flow.star".to_string(),
            source: FlowHostError(format!("eval thread join failed: {e}")),
        })?
        .map_err(|panic| FlowError::Agent {
            step: "flow.star".to_string(),
            source: FlowHostError(format!("eval thread panicked: {panic:?}")),
        })?
        .map_err(|message| FlowError::Agent {
            step: "flow.star".to_string(),
            source: FlowHostError(message),
        })?;

    let mut outputs = serde_json::Map::new();
    if let Some(value) = result {
        outputs.insert("result".to_string(), value);
    }
    Ok(FlowOutcome { outputs })
}

/// One agent call with checkpoint replay (M2.1 semantics, same as the yaml
/// runtime): a cache hit skips the spawn; a miss runs and records.
async fn run_one_agent_checked<H: FlowAgentHost>(
    host: &H,
    prompt: String,
) -> Result<String, String> {
    if let Some(cached) = host.checkpoint_read(&prompt).await {
        return Ok(cached);
    }
    let raw = host.run_agent(prompt.clone()).await.map_err(|e| e.0)?;
    host.checkpoint_write(&prompt, &raw).await;
    Ok(raw)
}

/// Evaluate one parsed module: bind `args`, install the bridge, run, and
/// export the module-global `result` as JSON (None when the script leaves
/// it unset).
fn eval_module_with_bridge(
    module: Module,
    ast: AstModule,
    globals: &starlark::environment::Globals,
    tx: mpsc::SyncSender<BridgeRequest>,
    args: &serde_json::Value,
) -> Result<Option<serde_json::Value>, String> {
    let bridge = Bridge { tx };
    let mut eval = Evaluator::new(&module);
    let args_value =
        json_to_starlark(module.heap(), args).map_err(|e| format!("args conversion: {e}"))?;
    module.set("args", args_value);
    eval.extra = Some(&bridge);
    eval.eval_module(ast, globals).map_err(|e| format!("flow.star eval: {e}"))?;
    module
        .get("result")
        .map(|value| value.to_json_value())
        .transpose()
        .map_err(|e| format!("result conversion: {e}"))
}
