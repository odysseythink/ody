//! Host-side declarative Flow runtime for `SkillType::Flow` skills (M1.1).
//!
//! The loader (`ody-core-skills`) parses and validates `flow.yaml` into
//! [`ody_core_skills::FlowPlan`] at load time; this module executes a
//! validated plan as a pure in-crate state machine. Host capabilities are
//! injected via [`FlowAgentHost`] so the kernel is fully testable with
//! mocks; M1.2 maps [`FlowAgentHost::run_agent`] to
//! `crate::agent::control` spawn + wait.
//!
//! # Host-function semantics (single source of truth)
//!
//! Every Flow runtime implementation (`flow.yaml` today; `flow.star` and
//! `workflow.js` in M3) MUST match these semantics. The M3 conformance
//! suite tests all runtimes against this document.
//!
//! - `agent(prompt, { schema?, label? })` — run one sub-agent with the
//!   rendered prompt and return its final result. The yaml surface exposes
//!   `prompt` only (`agent: <template>`); `schema` (JSON validation +
//!   retry) and `label` are reserved for script runtimes (M4). A failed
//!   agent fails the whole plan immediately: in-flight siblings are
//!   cancelled, no further steps run, and the error is reported.
//! - `pipeline(items, each)` — fan out one agent per item; all items of one
//!   batch start concurrently; results bind in item order regardless of
//!   completion order. A single batch is capped at [`FLOW_BATCH_LIMIT`]
//!   (4096, aligned with Claude Code dynamic workflows); exceeding the cap
//!   is a hard error, never silent truncation or chunking. `items` must
//!   evaluate to a JSON array.
//! - `parallel(steps)` — run child steps concurrently and wait for all.
//!   Children see the binding snapshot taken before the group starts;
//!   their `output`s merge into the context in declaration order after all
//!   succeed; the group's optional `output` binds an array of child
//!   results in declaration order. Same [`FLOW_BATCH_LIMIT`] cap.
//! - `phase(title)` / `log(msg)` — progress reporting only. Yaml phases
//!   are structural (the `phases:` list); phase begin/end progress events
//!   are wired in M1.4. No ordering side effects beyond sequential
//!   execution.
//! - `args` — trigger inputs, pre-bound as a context root and addressable
//!   as `${{ args.key }}`.
//!
//! # Determinism and concurrency notes
//!
//! - Each step may bind its result under `output`. Binding an already-used
//!   name — including the reserved roots `args` and `item` — is
//!   [`FlowError::DuplicateBinding`] / [`FlowError::ReservedBinding`]: fail
//!   fast, no silent shadowing.
//! - Implementations MAY bound real concurrency with a semaphore as long as
//!   result order and semantics are unchanged (evaluated at the M1.2 spawn
//!   mapping).
//! - Templates: [`interp`] implements `${{ name.path }}` and
//!   `${item.path}`.

use std::fmt;

use ody_core_skills::FlowPlan;

pub(crate) mod interp;

mod checkpoint;
mod host;
mod plan_summary;
mod runner;
mod runtime;
mod schema;
#[cfg(feature = "flow-starlark")]
mod star;
mod trigger;

pub(crate) use checkpoint::CheckpointStore;
pub(crate) use checkpoint::plan_fingerprint;
pub(crate) use plan_summary::FlowPlanSummary;
pub(crate) use runner::SessionFlowRunner;

pub(crate) use host::SessionFlowAgentHost;
#[cfg(feature = "flow-starlark")]
pub(crate) use star::StarlarkFlowRuntime;
pub(crate) use trigger::flow_args_from_input;
pub(crate) use trigger::partition_flow_skills;
pub(crate) use trigger::run_flow_skills_in_turn;
pub(crate) use trigger::run_one_flow_skill;

#[cfg(test)]
#[path = "flow_tests.rs"]
mod flow_tests;

/// Single-batch fan-out cap for `pipeline` / `parallel` (parent report
/// §2.2, aligned with Claude Code dynamic workflows). Exceeding it is a
/// hard [`FlowError::LimitExceeded`].
pub(crate) const FLOW_BATCH_LIMIT: usize = 4096;

/// Failure reported by a [`FlowAgentHost`] implementation; wrapped into
/// [`FlowError::Agent`] at the call site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FlowHostError(pub(crate) String);

impl fmt::Display for FlowHostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "flow host error: {}", self.0)
    }
}

impl std::error::Error for FlowHostError {}

/// Everything that can go wrong while validating or executing a Flow.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum FlowError {
    /// Static validation (`FlowRuntime::validate`) rejected the source.
    Parse { reason: String },
    /// A template referenced a binding that is not in scope.
    UnknownBinding { name: String, step: String },
    /// A template expression failed lexical validation.
    InvalidExpression { expr: String, step: String },
    /// A template had an unterminated `${{` or `${`.
    UnterminatedTemplate { step: String },
    /// A dotted path segment did not resolve inside the bound JSON value.
    MissingPath { name: String, path: String, step: String },
    /// A pipeline expression evaluated to a non-array value.
    NotAnArray { expr: String, step: String },
    /// A single batch exceeded [`FLOW_BATCH_LIMIT`].
    LimitExceeded { limit: usize, actual: usize, step: String },
    /// An `output` name was bound twice.
    DuplicateBinding { name: String, step: String },
    /// An `output` name collided with a reserved root (`args` / `item`).
    ReservedBinding { name: String, step: String },
    /// The host agent failed; the plan short-circuits.
    Agent { step: String, source: FlowHostError },
    /// The run was denied, timed out, or aborted by the pre-run guardian
    /// approval (M2.2).
    Denied { reason: String },
}

impl fmt::Display for FlowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FlowError::Parse { reason } => write!(f, "invalid flow plan: {reason}"),
            FlowError::UnknownBinding { name, step } => {
                write!(f, "flow step '{step}': unknown binding '{name}'")
            }
            FlowError::InvalidExpression { expr, step } => {
                write!(f, "flow step '{step}': invalid template expression '{expr}'")
            }
            FlowError::UnterminatedTemplate { step } => {
                write!(f, "flow step '{step}': unterminated template expression")
            }
            FlowError::MissingPath { name, path, step } => {
                write!(f, "flow step '{step}': '{path}' not found in binding '{name}'")
            }
            FlowError::NotAnArray { expr, step } => {
                write!(f, "flow step '{step}': pipeline expression '{expr}' is not an array")
            }
            FlowError::LimitExceeded { limit, actual, step } => {
                write!(f, "flow step '{step}': batch of {actual} exceeds the limit of {limit}")
            }
            FlowError::DuplicateBinding { name, step } => {
                write!(f, "flow step '{step}': output '{name}' is already bound")
            }
            FlowError::ReservedBinding { name, step } => {
                write!(f, "flow step '{step}': output '{name}' uses a reserved name")
            }
            FlowError::Agent { step, source } => {
                write!(f, "flow step '{step}': agent failed: {source}")
            }
            FlowError::Denied { reason } => {
                write!(f, "flow run denied: {reason}")
            }
        }
    }
}

impl std::error::Error for FlowError {}

/// Per-run inputs for a Flow execution.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct FlowContext {
    /// Trigger arguments supplied by the invoker (slash payload in M1.3).
    /// Pre-bound as the `args` root; templates use `${{ args.key }}`.
    pub(crate) args: serde_json::Map<String, serde_json::Value>,
}

/// Result of a successful Flow run: every binding produced by steps.
/// `args` is an input and never appears in `outputs`.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct FlowOutcome {
    pub(crate) outputs: serde_json::Map<String, serde_json::Value>,
}

/// Kernel-side progress signal (M1.4): emitted at phase begin/end and on
/// every completed top-level step. Hosts surface these to the user via
/// [`FlowAgentHost::report_progress`]; the yaml surface has no `log()` step
/// (reserved for M3 script runtimes), so phase/step events carry the whole
/// observable progress of a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FlowProgress {
    /// A phase started; `total_steps` is its top-level step count.
    PhaseBegin { phase_id: String, total_steps: u32 },
    /// Step `step_index` (1-based) of `phase_id` completed.
    StepCompleted { phase_id: String, step_index: u32, total_steps: u32 },
    /// A phase ended; on failure `completed_steps` < `total_steps`.
    PhaseEnd { phase_id: String, completed_steps: u32, total_steps: u32 },
    /// Free-form progress line from a script runtime (M3: `flow.star`
    /// `phase()`/`log()`; `workflow.js` in M3.2). Script carriers have no
    /// structured phases/steps, so their progress surfaces as log lines.
    Log { message: String },
}

/// Injected host capability that actually runs one agent. M1.1 tests mock
/// this; M1.2 maps it to `crate::agent::control` spawn + wait (reusing
/// `exceeds_thread_spawn_depth_limit` per the M1 report).
pub(crate) trait FlowAgentHost: Send + Sync {
    /// Run one agent with the fully rendered prompt and return its final
    /// result text. Structured results should be returned as JSON text —
    /// the kernel stores parsed JSON when possible, else the raw string.
    ///
    /// The returned future must be cancellation-safe: the kernel drops
    /// in-flight sibling futures when a batch short-circuits, and dropping
    /// must abort the underlying agent (the M1.2 spawn mapping guarantees
    /// this by aborting its JoinSet on drop).
    ///
    /// The future is declared `Send` so batched futures can move across
    /// worker threads (implementations may write plain `async fn`, which
    /// satisfies this signature when its captures are `Send`).
    fn run_agent(
        &self,
        prompt: String,
    ) -> impl std::future::Future<Output = Result<String, FlowHostError>> + Send;

    /// Report [`FlowProgress`] for the run this host is executing. Provided
    /// with a no-op default so existing implementations (M1.1 mocks) keep
    /// compiling; M1.2's session host overrides it to emit protocol events.
    /// Declared as RPITIT (like `run_agent`) so overrides keep the `Send`
    /// guarantee required by batched execution.
    fn report_progress(
        &self,
        _progress: FlowProgress,
    ) -> impl std::future::Future<Output = ()> + Send {
        async {}
    }

    /// Look up a previously recorded result for the exact rendered prompt
    /// (M2.1 checkpoint/replay). Provided default: no cache.
    fn checkpoint_read(
        &self,
        _prompt: &str,
    ) -> impl std::future::Future<Output = Option<String>> + Send {
        async { None }
    }

    /// Record a completed agent result for future replay. Provided default:
    /// no-op. Implementations write through eagerly so an aborted run keeps
    /// every agent that finished before the abort.
    fn checkpoint_write(
        &self,
        _prompt: &str,
        _output: &str,
    ) -> impl std::future::Future<Output = ()> + Send {
        async {}
    }
}

/// A validated Flow plan: the executable form of one carrier's source.
/// Script carriers (`flow.star`, `workflow.js`) validate syntax up front
/// but re-parse at run time — for them the source text IS the plan.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum FlowPlanSource {
    /// `flow.yaml`: the declarative AST parsed by `ody-core-skills`.
    Yaml(FlowPlan),
    /// `flow.star`: syntax-checked Starlark source (M3.1).
    #[cfg(feature = "flow-starlark")]
    Starlark(String),
    /// `workflow.js`: syntax-checked JS source (M3.2, `flow-v8` feature).
    #[cfg(feature = "flow-v8")]
    V8(String),
}

impl FlowPlanSource {
    /// Carrier artifact name for this plan.
    pub(crate) fn artifact(&self) -> &'static str {
        match self {
            FlowPlanSource::Yaml(_) => "flow.yaml",
            #[cfg(feature = "flow-starlark")]
            FlowPlanSource::Starlark(_) => "flow.star",
            #[cfg(feature = "flow-v8")]
            FlowPlanSource::V8(_) => "workflow.js",
        }
    }
}

/// Unified interface for Flow runtime implementations (`flow.yaml` today;
/// Starlark / V8 in M3). Host-function semantics documented on this module
/// are the single source of truth all implementations must match.
///
/// Intentionally not object-safe (generic `run`): M3 dispatches over an
/// enum of compiled runtimes instead of `dyn`, matching the compile-time
/// feature selection in the parent report §2.3.
pub(crate) trait FlowRuntime: Send + Sync {
    /// Artifact filename this runtime executes (`flow.yaml`).
    fn supported_artifact(&self) -> &'static str;

    /// Parse and statically validate source before it enters the runtime.
    fn validate(&self, source: &str) -> Result<FlowPlanSource, FlowError>;

    /// Execute a previously validated plan to completion (in-turn
    /// blocking per the locked M1 architecture decision).
    async fn run<H: FlowAgentHost>(
        &self,
        plan: FlowPlanSource,
        ctx: FlowContext,
        host: &H,
    ) -> Result<FlowOutcome, FlowError>;
}

/// Compile-time-assembled set of runtimes (parent report §2.3). Exactly
/// which variants exist depends on the `flow-starlark` / `flow-v8`
/// features; `select_flow_runtime` reports a clear error when the
/// requested carrier was compiled out.
#[derive(Debug)]
pub(crate) enum AnyFlowRuntime {
    Yaml(runtime::YamlFlowRuntime),
    #[cfg(feature = "flow-starlark")]
    Starlark(StarlarkFlowRuntime),
    #[cfg(feature = "flow-v8")]
    V8(crate::flow::v8::V8FlowRuntime),
}

impl FlowRuntime for AnyFlowRuntime {
    fn supported_artifact(&self) -> &'static str {
        match self {
            AnyFlowRuntime::Yaml(runtime) => runtime.supported_artifact(),
            #[cfg(feature = "flow-starlark")]
            AnyFlowRuntime::Starlark(runtime) => runtime.supported_artifact(),
            #[cfg(feature = "flow-v8")]
            AnyFlowRuntime::V8(runtime) => runtime.supported_artifact(),
        }
    }

    fn validate(&self, source: &str) -> Result<FlowPlanSource, FlowError> {
        match self {
            AnyFlowRuntime::Yaml(runtime) => runtime.validate(source),
            #[cfg(feature = "flow-starlark")]
            AnyFlowRuntime::Starlark(runtime) => runtime.validate(source),
            #[cfg(feature = "flow-v8")]
            AnyFlowRuntime::V8(runtime) => runtime.validate(source),
        }
    }

    async fn run<H: FlowAgentHost>(
        &self,
        plan: FlowPlanSource,
        ctx: FlowContext,
        host: &H,
    ) -> Result<FlowOutcome, FlowError> {
        match self {
            AnyFlowRuntime::Yaml(runtime) => runtime.run(plan, ctx, host).await,
            #[cfg(feature = "flow-starlark")]
            AnyFlowRuntime::Starlark(runtime) => runtime.run(plan, ctx, host).await,
            #[cfg(feature = "flow-v8")]
            AnyFlowRuntime::V8(runtime) => runtime.run(plan, ctx, host).await,
        }
    }
}

/// Pick the runtime for a flow artifact by its file name. Unknown names
/// and carriers compiled out (per the locked M3 decision: fail at trigger
/// time, not load time) return a clear [`FlowError::Parse`].
pub(crate) fn select_flow_runtime(artifact_name: &str) -> Result<AnyFlowRuntime, FlowError> {
    match artifact_name {
        "flow.yaml" => Ok(AnyFlowRuntime::Yaml(runtime::YamlFlowRuntime::new())),
        "flow.star" => select_starlark_runtime(),
        "workflow.js" => select_v8_runtime(),
        other => Err(FlowError::Parse {
            reason: format!(
                "unsupported flow artifact '{other}' (expected flow.yaml, flow.star, or workflow.js)"
            ),
        }),
    }
}

#[cfg(feature = "flow-starlark")]
fn select_starlark_runtime() -> Result<AnyFlowRuntime, FlowError> {
    Ok(AnyFlowRuntime::Starlark(StarlarkFlowRuntime::new()))
}

#[cfg(not(feature = "flow-starlark"))]
fn select_starlark_runtime() -> Result<AnyFlowRuntime, FlowError> {
    Err(compiled_without("flow-starlark", "flow.star"))
}

#[cfg(feature = "flow-v8")]
fn select_v8_runtime() -> Result<AnyFlowRuntime, FlowError> {
    Ok(AnyFlowRuntime::V8(crate::flow::v8::V8FlowRuntime::new()))
}

#[cfg(not(feature = "flow-v8"))]
fn select_v8_runtime() -> Result<AnyFlowRuntime, FlowError> {
    Err(compiled_without("flow-v8", "workflow.js"))
}

#[cfg(not(all(feature = "flow-starlark", feature = "flow-v8")))]
fn compiled_without(feature: &str, carrier: &str) -> FlowError {
    FlowError::Parse {
        reason: format!(
            "the '{carrier}' flow carrier is not available in this build (compiled without the `{feature}` Cargo feature)"
        ),
    }
}
