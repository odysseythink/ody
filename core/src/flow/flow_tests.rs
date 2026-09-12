use super::*;
use serde_json::json;

#[test]
fn interp_substitutes_context_binding() {
    let mut bindings = serde_json::Map::new();
    bindings.insert("gdd".to_string(), json!("THE GDD"));
    let ctx = interp::TemplateContext { bindings: &bindings, item: None };
    assert_eq!(interp::render_template("Summary: ${{ gdd }}", &ctx, "test").unwrap(), "Summary: THE GDD");
}

#[test]
fn interp_supports_dotted_path_and_array_index() {
    let mut bindings = serde_json::Map::new();
    bindings.insert("gdd".to_string(), json!({"mechanics": ["draw", "discard"]}));
    let ctx = interp::TemplateContext { bindings: &bindings, item: None };
    assert_eq!(interp::render_template("${{ gdd.mechanics.0 }}", &ctx, "test").unwrap(), "draw");
    assert_eq!(interp::render_template("${{ gdd.mechanics }}", &ctx, "test").unwrap(), "[\"draw\",\"discard\"]");
}

#[test]
fn interp_item_form_resolves_pipeline_item() {
    let bindings = serde_json::Map::new();
    let item = json!({"name": "deck-building"});
    let ctx = interp::TemplateContext { bindings: &bindings, item: Some(&item) };
    assert_eq!(interp::render_template("Implement ${item.name}.", &ctx, "test").unwrap(), "Implement deck-building.");
    assert_eq!(interp::render_template("${{ item }}", &ctx, "test").unwrap(), "{\"name\":\"deck-building\"}");
    // Array-valued items support numeric segments in the single-brace form.
    let arr_item = json!(["x", "y"]);
    let ctx = interp::TemplateContext { bindings: &bindings, item: Some(&arr_item) };
    assert_eq!(interp::render_template("${item.0}", &ctx, "test").unwrap(), "x");
    assert!(matches!(
        interp::render_template("${item.}", &ctx, "test"),
        Err(FlowError::InvalidExpression { .. })
    ));
    assert!(matches!(
        interp::render_template("${items}", &ctx, "test"),
        Err(FlowError::InvalidExpression { .. })
    ));
}

#[test]
fn interp_item_shadows_binding_only_inside_scope() {
    let mut bindings = serde_json::Map::new();
    bindings.insert("item".to_string(), json!("BINDING"));
    let item = json!("SCOPED");
    let scoped = interp::TemplateContext { bindings: &bindings, item: Some(&item) };
    assert_eq!(interp::render_template("${{ item }}", &scoped, "test").unwrap(), "SCOPED");
    let unscoped = interp::TemplateContext { bindings: &bindings, item: None };
    assert_eq!(interp::render_template("${{ item }}", &unscoped, "test").unwrap(), "BINDING");
    assert!(matches!(
        interp::render_template("${item}", &unscoped, "test"),
        Err(FlowError::UnknownBinding { name, .. }) if name == "item"
    ));
}

#[test]
fn interp_reports_unknown_binding_and_missing_path() {
    let bindings = serde_json::Map::new();
    let ctx = interp::TemplateContext { bindings: &bindings, item: None };
    assert!(matches!(
        interp::render_template("${{ missing }}", &ctx, "test"),
        Err(FlowError::UnknownBinding { name, step }) if name == "missing" && step == "test"
    ));
    let mut bindings = serde_json::Map::new();
    bindings.insert("gdd".to_string(), json!({"mechanics": []}));
    let ctx = interp::TemplateContext { bindings: &bindings, item: None };
    assert!(matches!(
        interp::render_template("${{ gdd.nope }}", &ctx, "test"),
        Err(FlowError::MissingPath { name, path, .. }) if name == "gdd" && path == "nope"
    ));
    assert!(matches!(
        interp::render_template("${{ gdd.mechanics.0 }}", &ctx, "test"),
        Err(FlowError::MissingPath { .. })
    ));
}

#[test]
fn interp_reports_invalid_and_unterminated_expressions() {
    let bindings = serde_json::Map::new();
    let ctx = interp::TemplateContext { bindings: &bindings, item: None };
    for bad in ["${{ }}", "${{ 1abc }}", "${{ a..b }}", "${{ a b }}"] {
        assert!(
            matches!(interp::render_template(bad, &ctx, "test"), Err(FlowError::InvalidExpression { .. })),
            "{bad} should be an invalid expression"
        );
    }
    assert!(matches!(
        interp::render_template("${foo}", &ctx, "test"),
        Err(FlowError::InvalidExpression { .. })
    ));
    assert!(matches!(
        interp::render_template("end: ${{ gdd", &ctx, "test"),
        Err(FlowError::UnterminatedTemplate { .. })
    ));
    assert!(matches!(
        interp::render_template("end: ${item", &ctx, "test"),
        Err(FlowError::UnterminatedTemplate { .. })
    ));
}

#[test]
fn interp_passes_through_literals_and_unaffected_dollars() {
    let mut bindings = serde_json::Map::new();
    bindings.insert("x".to_string(), json!("V"));
    let ctx = interp::TemplateContext { bindings: &bindings, item: None };
    assert_eq!(interp::render_template("cost: $5 } end", &ctx, "test").unwrap(), "cost: $5 } end");
    // No escape syntax in M1: `$${{ x }}` renders a literal `$` followed by the value.
    assert_eq!(interp::render_template("$${{ x }}", &ctx, "test").unwrap(), "$V");
}

#[test]
fn flow_error_display_mentions_variant_details() {
    let err = FlowError::LimitExceeded { limit: 4096, actual: 4097, step: "phase 'p' step 1".to_string() };
    let text = err.to_string();
    assert!(text.contains("4096") && text.contains("4097") && text.contains("phase 'p' step 1"));
    let err = FlowError::Agent { step: "s".to_string(), source: FlowHostError("boom".to_string()) };
    assert!(err.to_string().contains("boom"));
}

#[test]
fn flow_types_are_send_sync_and_default() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<FlowContext>();
    assert_send_sync::<FlowOutcome>();
    assert_send_sync::<FlowError>();
    assert_send_sync::<FlowHostError>();
    assert_send_sync::<FlowProgress>();
    assert_eq!(FlowContext::default().args.len(), 0);
    assert_eq!(FlowOutcome::default().outputs.len(), 0);
}


// ---- M1.1 execution kernel tests (T03) ----

use super::runtime::YamlFlowRuntime;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

/// Records every prompt; consumes canned responses in order; falls back to
/// `done: <prompt>` so tests can assert exact interpolated prompts.
#[derive(Default)]
struct MockHost {
    prompts: Mutex<Vec<String>>,
    responses: Mutex<VecDeque<Result<String, String>>>,
}

impl MockHost {
    fn with(responses: Vec<Result<String, String>>) -> Self {
        Self { responses: Mutex::new(responses.into()), ..Self::default() }
    }
}

impl FlowAgentHost for MockHost {
    async fn run_agent(&self, prompt: String) -> Result<String, FlowHostError> {
        self.prompts.lock().unwrap().push(prompt.clone());
        match self.responses.lock().unwrap().pop_front() {
            Some(Ok(text)) => Ok(text),
            Some(Err(reason)) => Err(FlowHostError(reason)),
            None => Ok(format!("done: {prompt}")),
        }
    }
}

/// Later items complete sooner; the kernel must still bind results in item order.
struct SlowFirstHost;

impl FlowAgentHost for SlowFirstHost {
    async fn run_agent(&self, prompt: String) -> Result<String, FlowHostError> {
        let index: usize = prompt.strip_prefix("item ").unwrap().parse().unwrap();
        for _ in 0..(3 - index) {
            tokio::task::yield_now().await;
        }
        Ok(format!("r{index}"))
    }
}

struct DropGuard(Arc<AtomicBool>);

impl Drop for DropGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// "hang" never completes unless its future is dropped (cancellation probe).
#[derive(Default)]
struct CancelProbeHost {
    dropped: Arc<AtomicBool>,
    prompts: Mutex<Vec<String>>,
}

impl FlowAgentHost for CancelProbeHost {
    async fn run_agent(&self, prompt: String) -> Result<String, FlowHostError> {
        self.prompts.lock().unwrap().push(prompt.clone());
        if prompt == "hang" {
            let _guard = DropGuard(self.dropped.clone());
            std::future::pending::<()>().await;
            unreachable!("pending only resolves if the future is dropped");
        }
        Err(FlowHostError("boom".to_string()))
    }
}

fn validate(runtime: &YamlFlowRuntime, source: &str) -> ody_core_skills::FlowPlan {
    runtime.validate(source).expect("fixture plan should validate")
}

#[test]
fn yaml_runtime_reports_supported_artifact() {
    assert_eq!(YamlFlowRuntime.supported_artifact(), "flow.yaml");
}

#[test]
fn validate_roundtrips_loader_parser() {
    let runtime = YamlFlowRuntime;
    assert!(runtime.validate("phases:\n  - id: x\n    steps:\n      - agent: do it\n").is_ok());
    assert!(matches!(runtime.validate("bogus: y\n"), Err(FlowError::Parse { .. })));
}

#[tokio::test]
async fn phases_run_in_order_and_outputs_feed_later_steps() {
    let host = MockHost::default();
    let plan = validate(
        &YamlFlowRuntime,
        "phases:\n  - id: design\n    steps:\n      - agent: Generate\n        output: gdd\n  - id: use\n    steps:\n      - agent: Use ${{ gdd }}\n",
    );
    let outcome = YamlFlowRuntime.run(plan, FlowContext::default(), &host).await.unwrap();
    assert_eq!(outcome.outputs["gdd"], json!("done: Generate"));
    assert_eq!(*host.prompts.lock().unwrap(), vec!["Generate".to_string(), "Use done: Generate".to_string()]);
}

#[tokio::test]
async fn agent_output_is_parsed_as_json_when_possible() {
    let host = MockHost::with(vec![Ok("{\"mechanics\": [\"draw\"]}".to_string())]);
    let plan = validate(
        &YamlFlowRuntime,
        "phases:\n  - id: p\n    steps:\n      - agent: Generate\n        output: gdd\n",
    );
    let outcome = YamlFlowRuntime.run(plan, FlowContext::default(), &host).await.unwrap();
    assert_eq!(outcome.outputs["gdd"], json!({"mechanics": ["draw"]}));
}

#[tokio::test(flavor = "current_thread")]
async fn pipeline_preserves_item_order_regardless_of_completion_order() {
    let mut args = serde_json::Map::new();
    args.insert("items".to_string(), json!([0, 1, 2]));
    let plan = validate(
        &YamlFlowRuntime,
        "phases:\n  - id: p\n    steps:\n      - pipeline: ${{ args.items }}\n        each: item ${item}\n        output: results\n",
    );
    let outcome = YamlFlowRuntime
        .run(plan, FlowContext { args }, &SlowFirstHost)
        .await
        .unwrap();
    assert_eq!(outcome.outputs["results"], json!(["r0", "r1", "r2"]));
}

#[tokio::test]
async fn pipeline_items_interpolate_context_and_empty_array_is_allowed() {
    let host = MockHost::with(vec![Ok("{\"mechanics\": [\"draw\", \"discard\"], \"owner\": \"alice\", \"none\": []}".to_string())]);
    let plan = validate(
        &YamlFlowRuntime,
        "phases:\n  - id: p\n    steps:\n      - agent: Gen\n        output: gdd\n      - pipeline: ${{ gdd.mechanics }}\n        each: Implement ${item} per ${{ gdd.owner }}\n        output: impls\n      - pipeline: ${{ gdd.none }}\n        each: never\n        output: empties\n",
    );
    let outcome = YamlFlowRuntime.run(plan, FlowContext::default(), &host).await.unwrap();
    let prompts = host.prompts.lock().unwrap();
    assert_eq!(prompts[0], "Gen");
    assert_eq!(prompts[1], "Implement draw per alice");
    assert_eq!(prompts[2], "Implement discard per alice");
    assert_eq!(outcome.outputs["impls"], json!(["done: Implement draw per alice", "done: Implement discard per alice"]));
    assert_eq!(outcome.outputs["empties"], json!([]));
}

#[tokio::test]
async fn parallel_binds_results_and_merges_child_outputs_in_order() {
    let host = MockHost::default();
    let plan = validate(
        &YamlFlowRuntime,
        "phases:\n  - id: p\n    steps:\n      - parallel:\n          - agent: A\n            output: x\n          - agent: B\n            output: y\n        output: both\n",
    );
    let outcome = YamlFlowRuntime.run(plan, FlowContext::default(), &host).await.unwrap();
    assert_eq!(outcome.outputs["x"], json!("done: A"));
    assert_eq!(outcome.outputs["y"], json!("done: B"));
    assert_eq!(outcome.outputs["both"], json!(["done: A", "done: B"]));
}

#[tokio::test]
async fn parallel_children_see_pre_group_snapshot_only() {
    let host = MockHost::default();
    // Child 2 references `x`, which child 1 binds — with snapshot semantics
    // this must fail because children cannot observe each other's outputs.
    let plan = validate(
        &YamlFlowRuntime,
        "phases:\n  - id: p\n    steps:\n      - parallel:\n          - agent: A\n            output: x\n          - agent: B uses ${{ x }}\n",
    );
    let err = YamlFlowRuntime.run(plan, FlowContext::default(), &host).await.unwrap_err();
    assert!(matches!(err, FlowError::UnknownBinding { name, .. } if name == "x"));
}

#[tokio::test]
async fn duplicate_and_reserved_output_names_fail() {
    let host = MockHost::default();
    let dup = validate(
        &YamlFlowRuntime,
        "phases:\n  - id: p\n    steps:\n      - agent: A\n        output: same\n      - agent: B\n        output: same\n",
    );
    assert!(matches!(
        YamlFlowRuntime.run(dup, FlowContext::default(), &host).await,
        Err(FlowError::DuplicateBinding { name, .. }) if name == "same"
    ));
    for reserved in ["args", "item"] {
        let source = format!(
            "phases:\n  - id: p\n    steps:\n      - agent: A\n        output: {reserved}\n"
        );
        let plan = validate(&YamlFlowRuntime, &source);
        assert!(matches!(
            YamlFlowRuntime.run(plan, FlowContext::default(), &host).await,
            Err(FlowError::ReservedBinding { name, .. }) if name == reserved
        ));
    }
}

#[tokio::test]
async fn pipeline_over_batch_limit_fails_before_spawning() {
    let host = MockHost::default();
    let mut args = serde_json::Map::new();
    args.insert("items".to_string(), json!((0..4097).collect::<Vec<i32>>()));
    let plan = validate(
        &YamlFlowRuntime,
        "phases:\n  - id: p\n    steps:\n      - pipeline: ${{ args.items }}\n        each: item ${item}\n",
    );
    let err = YamlFlowRuntime.run(plan, FlowContext { args }, &host).await.unwrap_err();
    assert!(matches!(
        err,
        FlowError::LimitExceeded { limit, actual, .. } if limit == 4096 && actual == 4097
    ));
    assert!(host.prompts.lock().unwrap().is_empty(), "no agent may start past the limit");
}

#[tokio::test]
async fn parallel_over_batch_limit_fails_before_spawning() {
    let host = MockHost::default();
    let children: String = (0..4097).map(|_| "          - agent: x\n".to_string()).collect();
    let source = format!("phases:\n  - id: p\n    steps:\n      - parallel:\n{children}");
    let plan = validate(&YamlFlowRuntime, &source);
    let err = YamlFlowRuntime.run(plan, FlowContext::default(), &host).await.unwrap_err();
    assert!(matches!(err, FlowError::LimitExceeded { actual: 4097, .. }));
    assert!(host.prompts.lock().unwrap().is_empty());
}

#[tokio::test]
async fn pipeline_expression_must_evaluate_to_an_array() {
    let host = MockHost::default();
    let mut args = serde_json::Map::new();
    args.insert("count".to_string(), json!(5));
    let plan = validate(
        &YamlFlowRuntime,
        "phases:\n  - id: p\n    steps:\n      - pipeline: ${{ args.count }}\n        each: item ${item}\n",
    );
    assert!(matches!(
        YamlFlowRuntime.run(plan, FlowContext { args }, &host).await,
        Err(FlowError::NotAnArray { .. })
    ));
    assert!(host.prompts.lock().unwrap().is_empty());
}

#[tokio::test]
async fn unknown_binding_fails_before_any_agent_runs() {
    let host = MockHost::default();
    let plan = validate(
        &YamlFlowRuntime,
        "phases:\n  - id: p\n    steps:\n      - agent: Use ${{ missing }}\n",
    );
    assert!(matches!(
        YamlFlowRuntime.run(plan, FlowContext::default(), &host).await,
        Err(FlowError::UnknownBinding { name, .. }) if name == "missing"
    ));
    assert!(host.prompts.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn agent_failure_cancels_in_flight_siblings() {
    let host = CancelProbeHost::default();
    let dropped = host.dropped.clone();
    let plan = validate(
        &YamlFlowRuntime,
        "phases:\n  - id: p\n    steps:\n      - parallel:\n          - agent: hang\n          - agent: fail\n",
    );
    let err = YamlFlowRuntime
        .run(plan, FlowContext::default(), &host)
        .await
        .expect_err("a failing sibling must fail the whole plan");
    assert!(matches!(err, FlowError::Agent { .. }));
    assert!(dropped.load(Ordering::SeqCst), "in-flight sibling future must be dropped on short-circuit");
}

#[tokio::test]
async fn outcome_excludes_the_args_root() {
    let host = MockHost::default();
    let mut args = serde_json::Map::new();
    args.insert("theme".to_string(), json!("space"));
    let plan = validate(
        &YamlFlowRuntime,
        "phases:\n  - id: p\n    steps:\n      - agent: Make a game about ${{ args.theme }}\n        output: game\n",
    );
    let outcome = YamlFlowRuntime.run(plan, FlowContext { args }, &host).await.unwrap();
    assert!(!outcome.outputs.contains_key("args"));
    assert_eq!(outcome.outputs["game"], json!("done: Make a game about space"));
}

#[tokio::test]
async fn nested_parallel_flattens_recursively() {
    let host = MockHost::default();
    let plan = validate(
        &YamlFlowRuntime,
        "phases:\n  - id: p\n    steps:\n      - parallel:\n          - parallel:\n              - agent: Inner\n          - agent: Outer\n        output: nested\n",
    );
    let outcome = YamlFlowRuntime.run(plan, FlowContext::default(), &host).await.unwrap();
    assert_eq!(outcome.outputs["nested"], json!([["done: Inner"], "done: Outer"]));
}


// ---- M1.2 session host tests (T01) ----

use super::host::AbortOnDrop;
use crate::ThreadManager;
use crate::session::tests::make_session_and_context;
use crate::session::tests::make_session_and_context_and_config_and_rx;
use ody_models_manager::bundled_models_response;
use ody_protocol::ThreadId;
use ody_protocol::protocol::EventMsg;
use ody_protocol::protocol::Op;
use ody_protocol::protocol::TurnCompleteEvent;
use tokio::time::Duration;
use tokio::time::sleep;
use tokio::time::timeout;

fn flow_test_thread_manager() -> ThreadManager {
    ThreadManager::with_models_provider_and_catalog_for_tests(
        crate::config::test_provider(),
        Some(bundled_models_response().expect("bundled model catalog should parse")),
    )
}

/// Drive a spawned child thread to `Completed` with a synthetic turn
/// complete event, the same pattern the multi_agents v2 tests use.
async fn drive_child_to_completion(manager: &ThreadManager, thread_id: ThreadId, message: &str) {
    let child = manager
        .get_thread(thread_id)
        .await
        .expect("child thread should be registered");
    let child_turn = child.ody.session.new_default_turn().await;
    child
        .ody
        .session
        .send_event(
            child_turn.as_ref(),
            EventMsg::TurnComplete(TurnCompleteEvent {
                turn_id: child_turn.sub_id.clone(),
                last_agent_message: Some(message.to_string()),
                completed_at: None,
                duration_ms: None,
                time_to_first_token_ms: None,
            }),
        )
        .await;
}

#[tokio::test]
async fn session_flow_host_spawn_and_wait_returns_final_message() {
    let (mut session, turn, rx) =
        make_session_and_context_and_config_and_rx(Vec::new(), |config| {
            config.agent_max_depth = 4;
        })
        .await;
    let manager = flow_test_thread_manager();
    Arc::get_mut(&mut session)
        .expect("unique session arc")
        .services
        .agent_control = manager.agent_control();
    let host = SessionFlowAgentHost::new(session.clone(), turn.clone(), "test-flow");

    let thread_id = host
        .spawn_agent("summarize the repo".to_string())
        .await
        .expect("flow spawn should succeed");

    // The spawn is visible through the shared Collab spawn events, reusing
    // the existing TUI rendering for sub-agent spawns (M1.2 A3).
    let (begin, end) = timeout(Duration::from_secs(5), async {
        let mut begin = None;
        let mut end = None;
        while end.is_none() {
            let event = rx.recv().await.expect("event stream should stay open");
            match event.msg {
                EventMsg::CollabAgentSpawnBegin(ev) => begin = Some(ev),
                EventMsg::CollabAgentSpawnEnd(ev) => end = Some(ev),
                _ => {}
            }
        }
        (begin.expect("spawn begin should precede spawn end"), end.unwrap())
    })
    .await
    .expect("spawn begin/end events should arrive");
    assert_eq!(begin.call_id, end.call_id);
    assert_eq!(begin.prompt, "summarize the repo");
    assert_eq!(end.new_thread_id, Some(thread_id));
    assert_eq!(end.new_agent_role.as_deref(), Some("test-flow"));

    drive_child_to_completion(&manager, thread_id, "all done").await;
    let result = host.wait_agent(thread_id).await.expect("wait should complete");
    assert_eq!(result, "all done");
}

#[tokio::test]
async fn session_flow_host_run_agent_composes_spawn_and_wait() {
    let (mut session, turn) = make_session_and_context().await;
    let manager = flow_test_thread_manager();
    session.services.agent_control = manager.agent_control();
    let session = Arc::new(session);
    let turn = Arc::new(turn);
    let host = SessionFlowAgentHost::new(session.clone(), turn.clone(), "compose-flow");

    let run = tokio::spawn({
        let host = host.clone();
        async move { host.run_agent("do the thing".to_string()).await }
    });
    // The composed `run_agent` path must expose the child through the same
    // registry the rest of the session sees.
    let thread_id = timeout(Duration::from_secs(5), async {
        loop {
            if let Some(thread_id) =
                pending_child_thread_id(&manager, session.thread_id).await
            {
                return thread_id;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("run_agent should spawn a child thread");
    drive_child_to_completion(&manager, thread_id, "composed result").await;
    let result = run.await.expect("run_agent task should finish").expect("run_agent should succeed");
    assert_eq!(result, "composed result");
}

/// Poll the manager for a live child of `parent` that has submitted its
/// initial input (any op capture proves the thread exists and is wired).
async fn pending_child_thread_id(manager: &ThreadManager, parent: ThreadId) -> Option<ThreadId> {
    manager
        .captured_ops()
        .into_iter()
        .find(|(thread_id, _)| *thread_id != parent)
        .map(|(thread_id, _)| thread_id)
}

#[tokio::test]
async fn session_flow_host_spawn_respects_depth_limit() {
    let (mut session, turn, _rx) =
        make_session_and_context_and_config_and_rx(Vec::new(), |config| {
            config.agent_max_depth = 0;
        })
        .await;
    let manager = flow_test_thread_manager();
    Arc::get_mut(&mut session)
        .expect("unique session arc")
        .services
        .agent_control = manager.agent_control();
    let host = SessionFlowAgentHost::new(session.clone(), turn.clone(), "deep-flow");

    let err = host
        .spawn_agent("should never spawn".to_string())
        .await
        .expect_err("depth limit must block the spawn");
    assert!(err.0.contains("depth limit"), "unexpected error: {err}");
}

#[tokio::test]
async fn session_flow_host_abort_on_drop_interrupts_unfinished_agent() {
    let (mut session, turn) = make_session_and_context().await;
    let manager = flow_test_thread_manager();
    session.services.agent_control = manager.agent_control();
    let session = Arc::new(session);
    let turn = Arc::new(turn);
    let host = SessionFlowAgentHost::new(session.clone(), turn.clone(), "cancel-flow");

    let thread_id = host
        .spawn_agent("long running task".to_string())
        .await
        .expect("flow spawn should succeed");

    // Dropping the guard simulates the kernel short-circuiting a batch and
    // dropping the in-flight `run_agent` future (R1/R6): the child must be
    // interrupted on the runtime without any await on the drop path.
    let guard = AbortOnDrop::new(&session, thread_id);
    drop(guard);

    timeout(Duration::from_secs(5), async {
        loop {
            let interrupted = manager
                .captured_ops()
                .iter()
                .any(|(id, op)| *id == thread_id && matches!(op, Op::Interrupt));
            if interrupted {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("dropping the guard should interrupt the unfinished child agent");
}


// ---- M1.3 trigger tests (T02) ----

use super::trigger::flow_args_from_input;
use super::trigger::partition_flow_skills;
use super::trigger::run_flow_skills_in_turn;
use ody_core_skills::HostSkillsSnapshot;
use ody_core_skills::SkillLoadOutcome;
use ody_core_skills::SkillMetadata;
use ody_core_skills::SkillType;
use crate::TurnContext;
use ody_utils_absolute_path::AbsolutePathBuf;

fn skill_of_type(name: &str, skill_type: SkillType) -> SkillMetadata {
    SkillMetadata {
        name: name.to_string(),
        skill_type,
        ..Default::default()
    }
}

fn flow_skill_on_disk(name: &str, flow_yaml: &std::path::Path) -> SkillMetadata {
    SkillMetadata {
        name: name.to_string(),
        description: "test flow skill".to_string(),
        skill_type: SkillType::Flow,
        flow_artifact: Some(
            AbsolutePathBuf::try_from(flow_yaml.to_path_buf())
                .expect("flow.yaml path should be absolute"),
        ),
        path_to_skills_md: AbsolutePathBuf::try_from(flow_yaml.parent().unwrap().join("SKILL.md"))
            .expect("SKILL.md path should be absolute"),
        ..Default::default()
    }
}

fn install_skills(turn: &mut TurnContext, skills: Vec<SkillMetadata>) {
    let mut outcome = SkillLoadOutcome::default();
    outcome.skills = skills;
    turn.turn_skills = crate::session::turn_context::TurnSkillsContext::new(
        HostSkillsSnapshot::new(Arc::new(outcome)),
    );
}

fn user_text_input(text: &str) -> ody_protocol::user_input::UserInput {
    ody_protocol::user_input::UserInput::Text {
        text: text.to_string(),
        text_elements: Vec::new(),
    }
}

fn response_item_text(item: &ody_protocol::models::ResponseItem) -> String {
    let ody_protocol::models::ResponseItem::Message { content, .. } = item else {
        panic!("expected a message item, got {item:?}");
    };
    content
        .iter()
        .filter_map(|content| match content {
            ody_protocol::models::ContentItem::InputText { text }
            | ody_protocol::models::ContentItem::OutputText { text } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn partition_flow_skills_splits_flow_and_preserves_mention_order() {
    let mentioned = vec![
        skill_of_type("inline-a", SkillType::Inline),
        skill_of_type("flow-x", SkillType::Flow),
        skill_of_type("prompt-b", SkillType::Prompt),
        skill_of_type("flow-y", SkillType::Flow),
    ];
    let (flow, injectable) = partition_flow_skills(mentioned);
    fn names(skills: &[SkillMetadata]) -> Vec<&str> {
        skills.iter().map(|skill| skill.name.as_str()).collect()
    }
    // Only non-flow skills continue down the SkillInstructions injection
    // chain, and both lists preserve the original mention order (R2).
    assert_eq!(names(&flow), vec!["flow-x", "flow-y"]);
    assert_eq!(names(&injectable), vec!["inline-a", "prompt-b"]);
}

#[test]
fn flow_args_parse_json_object_or_wrap_joined_text() {
    let args = flow_args_from_input(&[user_text_input(r#"{"a": 1}"#)]);
    assert_eq!(args.get("a"), Some(&json!(1)));

    let args = flow_args_from_input(&[user_text_input("hello"), user_text_input("world")]);
    assert_eq!(args.get("text"), Some(&json!("hello\nworld")));

    // Non-object JSON (arrays, scalars) is not a valid args object and is
    // wrapped as plain text instead.
    let args = flow_args_from_input(&[user_text_input("[1, 2]")]);
    assert_eq!(args.get("text"), Some(&json!("[1, 2]")));

    // Skill mentions in the same submission do not leak into the args.
    let args = flow_args_from_input(&[
        user_text_input("make a game"),
        ody_protocol::user_input::UserInput::Skill {
            name: "demo-flow".to_string(),
            path: std::path::PathBuf::from("/tmp/demo-flow/SKILL.md"),
        },
    ]);
    assert_eq!(args.get("text"), Some(&json!("make a game")));
}

#[tokio::test]
async fn run_flow_skills_in_turn_executes_flow_and_records_result_item() {
    let dir = tempfile::tempdir().expect("create flow dir");
    let flow_yaml = dir.path().join("flow.yaml");
    std::fs::write(
        &flow_yaml,
        "phases:\n  - id: p\n    steps:\n      - agent: Do ${{ args.text }}\n        output: result\n",
    )
    .expect("write flow.yaml");
    let skill = flow_skill_on_disk("demo-flow", &flow_yaml);

    let (mut session, turn) = make_session_and_context().await;
    let manager = flow_test_thread_manager();
    session.services.agent_control = manager.agent_control();
    let session = Arc::new(session);
    let mut turn = turn;
    install_skills(&mut turn, vec![skill.clone()]);
    let turn = Arc::new(turn);

    let run = tokio::spawn({
        let session = session.clone();
        let turn = turn.clone();
        async move {
            run_flow_skills_in_turn(
                &session,
                &turn,
                std::slice::from_ref(&skill),
                flow_args_from_input(&[user_text_input("make a game")]),
            )
            .await
        }
    });
    let thread_id = timeout(Duration::from_secs(5), async {
        loop {
            if let Some(thread_id) = pending_child_thread_id(&manager, session.thread_id).await {
                return thread_id;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("flow execution should spawn a sub-agent");
    drive_child_to_completion(&manager, thread_id, "flow child result").await;

    let items = run.await.expect("flow run task should finish");
    assert_eq!(items.len(), 1, "one result item per flow skill");
    let text = response_item_text(&items[0]);
    assert!(text.contains("<flow_result>"), "missing marker: {text}");
    assert!(text.contains("Flow 'demo-flow' completed."), "missing outcome: {text}");
    assert!(text.contains("flow child result"), "missing outputs: {text}");
}

#[tokio::test]
async fn run_flow_skills_in_turn_records_failure_item_and_warning_event() {
    let dir = tempfile::tempdir().expect("create flow dir");
    let flow_yaml = dir.path().join("flow.yaml");
    std::fs::write(
        &flow_yaml,
        "phases:\n  - id: p\n    steps:\n      - agent: Use ${{ missing }}\n",
    )
    .expect("write flow.yaml");
    let skill = flow_skill_on_disk("broken-flow", &flow_yaml);

    let (mut session, turn, rx) =
        make_session_and_context_and_config_and_rx(Vec::new(), |_| {}).await;
    let manager = flow_test_thread_manager();
    Arc::get_mut(&mut session)
        .expect("unique session arc")
        .services
        .agent_control = manager.agent_control();
    // Install the flow skill into the turn's skills snapshot (the harness
    // returns a uniquely-owned Arc, so in-place mutation is safe).
    let mut turn = turn;
    install_skills(Arc::get_mut(&mut turn).expect("unique turn arc"), vec![skill.clone()]);

    let items = run_flow_skills_in_turn(
        &session,
        &turn,
        std::slice::from_ref(&skill),
        flow_args_from_input(&[]),
    )
    .await;

    assert_eq!(items.len(), 1);
    let text = response_item_text(&items[0]);
    assert!(text.contains("Flow 'broken-flow' failed."), "missing failure: {text}");
    assert!(text.contains("missing"), "missing error detail: {text}");

    // The failure is also surfaced immediately as a warning event (A7).
    let saw_warning = timeout(Duration::from_secs(5), async {
        loop {
            let event = rx.recv().await.expect("event stream should stay open");
            if matches!(event.msg, EventMsg::Warning(_)) {
                return;
            }
        }
    })
    .await;
    assert!(saw_warning.is_ok(), "a warning event should accompany the failure");
}

// ---- M1.4 progress reporting tests (T03) ----

/// [`MockHost`] plus an ordered progress recording; the default
/// `report_progress` trait method is overridden exactly like the session
/// host does it.
#[derive(Default)]
struct RecordingHost {
    inner: MockHost,
    progress: Mutex<Vec<FlowProgress>>,
}

impl FlowAgentHost for RecordingHost {
    async fn run_agent(&self, prompt: String) -> Result<String, FlowHostError> {
        self.inner.run_agent(prompt).await
    }

    fn report_progress(
        &self,
        progress: FlowProgress,
    ) -> impl std::future::Future<Output = ()> + Send {
        self.progress.lock().unwrap().push(progress);
        async {}
    }
}

#[tokio::test]
async fn progress_events_follow_phase_and_step_lifecycle() {
    // The progress future must stay `Send` (R4): recording happens
    // synchronously and the returned future is a ready no-op. Checked on a
    // throwaway host so the probe event does not pollute the sequence below.
    fn assert_send<T: Send>(_: T) {}
    assert_send(RecordingHost::default().report_progress(FlowProgress::PhaseBegin {
        phase_id: "probe".to_string(),
        total_steps: 0,
    }));

    let host = RecordingHost::default();
    let plan = validate(
        &YamlFlowRuntime,
        "phases:\n  - id: design\n    steps:\n      - agent: A\n      - agent: B\n  - id: use\n    steps:\n      - agent: C\n",
    );
    let outcome = YamlFlowRuntime.run(plan, FlowContext::default(), &host).await.unwrap();
    assert_eq!(outcome.outputs.len(), 0);

    assert_eq!(
        *host.progress.lock().unwrap(),
        vec![
            FlowProgress::PhaseBegin { phase_id: "design".to_string(), total_steps: 2 },
            FlowProgress::StepCompleted {
                phase_id: "design".to_string(),
                step_index: 1,
                total_steps: 2,
            },
            FlowProgress::StepCompleted {
                phase_id: "design".to_string(),
                step_index: 2,
                total_steps: 2,
            },
            FlowProgress::PhaseEnd {
                phase_id: "design".to_string(),
                completed_steps: 2,
                total_steps: 2,
            },
            FlowProgress::PhaseBegin { phase_id: "use".to_string(), total_steps: 1 },
            FlowProgress::StepCompleted {
                phase_id: "use".to_string(),
                step_index: 1,
                total_steps: 1,
            },
            FlowProgress::PhaseEnd {
                phase_id: "use".to_string(),
                completed_steps: 1,
                total_steps: 1,
            },
        ]
    );
}

#[tokio::test]
async fn progress_phase_end_reports_partial_completion_on_failure() {
    let host = RecordingHost {
        inner: MockHost::with(vec![Err("boom".to_string())]),
        ..RecordingHost::default()
    };
    let plan = validate(
        &YamlFlowRuntime,
        "phases:\n  - id: p\n    steps:\n      - agent: A\n      - agent: B\n",
    );
    let err = YamlFlowRuntime.run(plan, FlowContext::default(), &host).await.unwrap_err();
    assert!(matches!(err, FlowError::Agent { .. }));

    // PhaseEnd still fires with completed_steps < total_steps (A9).
    assert_eq!(
        *host.progress.lock().unwrap(),
        vec![
            FlowProgress::PhaseBegin { phase_id: "p".to_string(), total_steps: 2 },
            FlowProgress::PhaseEnd {
                phase_id: "p".to_string(),
                completed_steps: 0,
                total_steps: 2,
            },
        ]
    );
}

#[tokio::test]
async fn run_flow_skills_in_turn_emits_progress_events_on_the_turn_stream() {
    let dir = tempfile::tempdir().expect("create flow dir");
    let flow_yaml = dir.path().join("flow.yaml");
    std::fs::write(
        &flow_yaml,
        "phases:\n  - id: build\n    steps:\n      - agent: Do it\n        output: result\n",
    )
    .expect("write flow.yaml");
    let skill = flow_skill_on_disk("progress-flow", &flow_yaml);

    let (mut session, mut turn, mut rx) =
        make_session_and_context_and_config_and_rx(Vec::new(), |_| {}).await;
    let manager = flow_test_thread_manager();
    Arc::get_mut(&mut session)
        .expect("unique session arc")
        .services
        .agent_control = manager.agent_control();
    install_skills(Arc::get_mut(&mut turn).expect("unique turn arc"), vec![skill.clone()]);

    let run = tokio::spawn({
        let session = session.clone();
        let turn = turn.clone();
        async move {
            run_flow_skills_in_turn(
                &session,
                &turn,
                std::slice::from_ref(&skill),
                flow_args_from_input(&[]),
            )
            .await
        }
    });
    let thread_id = timeout(Duration::from_secs(5), async {
        loop {
            if let Some(thread_id) = pending_child_thread_id(&manager, session.thread_id).await {
                return thread_id;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("flow execution should spawn a sub-agent");
    drive_child_to_completion(&manager, thread_id, "progress child result").await;
    let items = run.await.expect("flow run task should finish");
    assert_eq!(items.len(), 1);

    let mut begins = Vec::new();
    let mut steps = Vec::new();
    let mut ends = Vec::new();
    while let Ok(event) = rx.try_recv() {
        match event.msg {
            EventMsg::FlowPhaseBegin(ev) => begins.push(ev),
            EventMsg::FlowStepCompleted(ev) => steps.push(ev),
            EventMsg::FlowPhaseEnd(ev) => ends.push(ev),
            _ => {}
        }
    }
    assert_eq!(begins.len(), 1, "one phase begin: {begins:?}");
    assert_eq!(steps.len(), 1, "one step completed: {steps:?}");
    assert_eq!(ends.len(), 1, "one phase end: {ends:?}");
    let begin = &begins[0];
    assert_eq!(begin.flow_name, "progress-flow");
    assert_eq!(begin.phase_id, "build");
    assert_eq!(begin.total_steps, 1);
    assert_eq!(steps[0].call_id, begin.call_id);
    assert_eq!(steps[0].step_index, 1);
    assert_eq!(ends[0].call_id, begin.call_id);
    assert_eq!(ends[0].completed_steps, 1);
    assert_eq!(ends[0].total_steps, 1);
}


// ---- M1.5 end-to-end fan-out test (mirrors the bundled game-create sample) ----

/// Poll for a freshly spawned child of `parent` not yet in `driven`. Unlike
/// `pending_child_thread_id` (first child wins), this skips threads that were
/// already driven so fan-out batches can be drained one child at a time.
async fn next_pending_child_thread_id(
    manager: &ThreadManager,
    parent: ThreadId,
    driven: &std::collections::HashSet<ThreadId>,
) -> Option<ThreadId> {
    manager
        .captured_ops()
        .into_iter()
        .map(|(thread_id, _)| thread_id)
        .find(|thread_id| *thread_id != parent && !driven.contains(thread_id))
}


#[tokio::test]
async fn parallel_children_inherit_snapshot_without_rebinding_it() {
    // Regression (found by M1.5 e2e): a parallel group that runs after other
    // steps inherits the parent bindings via its snapshot; merging a child's
    // full binding map back would re-bind inherited names (e.g. `gdd`) and
    // fail with "already bound". Only bindings the child itself added may
    // merge.
    let host = MockHost::with(vec![
        Ok("{\"mechanics\": [\"a\"]}".to_string()),
        Ok("done: one".to_string()),
        Ok("done: two".to_string()),
    ]);
    let plan = validate(
        &YamlFlowRuntime,
        "phases:\n  - id: p\n    steps:\n      - agent: Gen\n        output: gdd\n      - parallel:\n          - agent: Uses ${{ gdd.mechanics }}\n          - agent: Plain\n        output: both\n",
    );
    let outcome = YamlFlowRuntime.run(plan, FlowContext::default(), &host).await.unwrap();
    assert_eq!(outcome.outputs["gdd"], json!({"mechanics": ["a"]}));
    assert_eq!(outcome.outputs["both"], json!(["done: one", "done: two"]));
}

#[tokio::test]
async fn flow_fanout_end_to_end_spawns_progresses_and_records_outputs() {
    // Same phase shape as the bundled game-create sample (M1.5): one design
    // agent, a pipeline fan-out over the GDD mechanics, then a parallel
    // verify group. Drives five real child threads through the session host.
    let dir = tempfile::tempdir().expect("create flow dir");
    let flow_yaml = dir.path().join("flow.yaml");
    std::fs::write(
        &flow_yaml,
        r#"phases:
  - id: design
    steps:
      - agent: 根据主题「${{ args.text }}」生成 GDD，JSON 输出 mechanics 与 constraints
        output: gdd
  - id: implement
    steps:
      - pipeline: ${{ gdd.mechanics }}
        each: 实现机制 ${item.name}，遵循 ${{ gdd.constraints }}
        output: implementations
  - id: verify
    steps:
      - parallel:
          - agent: 审查 ${{ implementations }} 的边界情况
          - agent: 运行测试并修复失败
        output: review
"#,
    )
    .expect("write flow.yaml");
    let skill = flow_skill_on_disk("game-create", &flow_yaml);

    let (mut session, turn, rx) =
        make_session_and_context_and_config_and_rx(Vec::new(), |config| {
            config.agent_max_depth = 4;
        })
        .await;
    let manager = flow_test_thread_manager();
    Arc::get_mut(&mut session)
        .expect("unique session arc")
        .services
        .agent_control = manager.agent_control();
    let session = Arc::new(session);
    // The harness returns a uniquely-owned Arc, so in-place mutation is safe.
    let mut turn = turn;
    install_skills(Arc::get_mut(&mut turn).expect("unique turn arc"), vec![skill.clone()]);
    let turn = Arc::new(turn);
    let turn = Arc::new(turn);

    let run = tokio::spawn({
        let session = session.clone();
        let turn = turn.clone();
        async move {
            run_flow_skills_in_turn(
                &session,
                &turn,
                std::slice::from_ref(&skill),
                flow_args_from_input(&[user_text_input("太空跳跃")]),
            )
            .await
        }
    });

    // Five agents spawn in plan order: 1 design, 2 pipeline items, 2 parallel
    // verify agents. The first reply is the GDD JSON the pipeline fans out
    // over; the remaining replies are plain completion messages.
    let scripts = [
        r#"{"mechanics":[{"name":"跳跃"},{"name":"二段跳"}],"constraints":"60fps 像素风"}"#,
        "impl 跳跃 done",
        "impl 二段跳 done",
        "边界审查通过",
        "测试全绿",
    ];
    let mut driven = std::collections::HashSet::new();
    for script in scripts {
        // Generous timeout: five spawn→wait chains under nextest load can
        // exceed the 5s budget the single-agent tests use; success paths
        // complete in milliseconds.
        let thread_id = timeout(Duration::from_secs(30), async {
            loop {
                if let Some(thread_id) =
                    next_pending_child_thread_id(&manager, session.thread_id, &driven).await
                {
                    return thread_id;
                }
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("flow fan-out should spawn the next sub-agent");
        assert!(driven.insert(thread_id), "each child should be driven once");
        drive_child_to_completion(&manager, thread_id, script).await;
    }

    let items = run.await.expect("flow run task should finish");
    assert_eq!(items.len(), 1, "one result item per flow skill");
    let text = response_item_text(&items[0]);
    assert!(text.contains("Flow 'game-create' completed."), "missing outcome: {text}");
    // Every phase output is recorded back into the conversation: the parsed
    // GDD, both pipeline results (in item order), and both review results.
    assert!(text.contains("constraints"), "missing gdd output: {text}");
    let jump = text.find("impl 跳跃 done").expect("missing pipeline output: {text}");
    let double_jump = text.find("impl 二段跳 done").expect("missing pipeline output: {text}");
    assert!(jump < double_jump, "pipeline outputs should keep item order: {text}");
    assert!(text.contains("边界审查通过"), "missing review output: {text}");
    assert!(text.contains("测试全绿"), "missing review output: {text}");

    // Drain the unbounded event stream: progress runs begin → step → end per
    // phase under one shared call_id, and every spawn carries the
    // fully-interpolated prompt (args/GDD bindings for fan-out steps).
    let mut progress = Vec::new();
    let mut spawn_prompts = Vec::new();
    while let Ok(event) = rx.try_recv() {
        match event.msg {
            EventMsg::FlowPhaseBegin(ev) => {
                assert_eq!(ev.flow_name, "game-create");
                progress.push(format!("begin:{}:{}", ev.phase_id, ev.total_steps));
            }
            EventMsg::FlowStepCompleted(ev) => {
                progress.push(format!("step:{}:{}/{}", ev.phase_id, ev.step_index, ev.total_steps));
            }
            EventMsg::FlowPhaseEnd(ev) => {
                progress.push(format!("end:{}:{}/{}", ev.phase_id, ev.completed_steps, ev.total_steps));
            }
            EventMsg::CollabAgentSpawnBegin(ev) => spawn_prompts.push(ev.prompt),
            _ => {}
        }
    }
    assert_eq!(
        progress,
        vec![
            "begin:design:1",
            "step:design:1/1",
            "end:design:1/1",
            "begin:implement:1",
            "step:implement:1/1",
            "end:implement:1/1",
            "begin:verify:1",
            "step:verify:1/1",
            "end:verify:1/1",
        ]
    );
    assert_eq!(spawn_prompts.len(), 5, "five fan-out agents should spawn: {spawn_prompts:?}");
    assert!(spawn_prompts[0].contains("太空跳跃"), "args binding missing: {}", spawn_prompts[0]);
    assert!(spawn_prompts[1].contains("实现机制 跳跃，"), "pipeline item missing: {}", spawn_prompts[1]);
    assert!(!spawn_prompts[1].contains("二段跳"), "pipeline items must not leak: {}", spawn_prompts[1]);
    assert!(spawn_prompts[1].contains("60fps 像素风"), "gdd binding missing: {}", spawn_prompts[1]);
    assert!(spawn_prompts[2].contains("实现机制 二段跳，"), "pipeline item missing: {}", spawn_prompts[2]);
    assert!(spawn_prompts[3].contains("impl 跳跃 done"), "implementations binding missing: {}", spawn_prompts[3]);
    assert!(spawn_prompts[4].contains("运行测试"), "verify prompt missing: {}", spawn_prompts[4]);
}


// ---- M2.1 checkpoint/replay tests ----

use super::checkpoint::plan_fingerprint;
use super::checkpoint::prompt_key;
use super::checkpoint::CheckpointStore;

/// Wraps MockHost with an in-memory checkpoint map so kernel tests can
/// assert hit/record behavior without touching the filesystem. The cache is
/// shared across clones: a re-run host replays entries recorded by the
/// previous (failed) run, mirroring the on-disk `CheckpointStore`.
#[derive(Default)]
struct MemCheckpointHost {
    inner: MockHost,
    cache: Arc<std::sync::Mutex<std::collections::HashMap<String, String>>>,
}

impl FlowAgentHost for MemCheckpointHost {
    async fn run_agent(&self, prompt: String) -> Result<String, FlowHostError> {
        self.inner.run_agent(prompt).await
    }

    fn checkpoint_read(
        &self,
        prompt: &str,
    ) -> impl std::future::Future<Output = Option<String>> + Send {
        let cached = self.cache.lock().unwrap().get(prompt).cloned();
        async move { cached }
    }

    fn checkpoint_write(
        &self,
        prompt: &str,
        output: &str,
    ) -> impl std::future::Future<Output = ()> + Send {
        // Test impl mutates eagerly (the real impl awaits the store lock).
        self.cache
            .lock()
            .unwrap()
            .insert(prompt.to_string(), output.to_string());
        async {}
    }
}

#[test]
fn checkpoint_fingerprint_is_stable_and_source_sensitive() {
    let src = "phases:\n  - id: x\n    steps: []\n";
    let fp = plan_fingerprint(src);
    assert_eq!(fp, plan_fingerprint(src));
    assert_ne!(fp, plan_fingerprint("phases:\n  - id: y\n    steps: []\n"));
    assert_eq!(fp.len(), 16);
    assert_ne!(prompt_key("One"), prompt_key("Two"));
}

#[tokio::test]
async fn checkpoint_hit_skips_agent_and_replays_output() {
    let host = MemCheckpointHost::default();
    host.cache.lock().unwrap().insert("Generate".to_string(), "{\"cached\":true}".to_string());
    let plan = validate(
        &YamlFlowRuntime,
        "phases:\n  - id: design\n    steps:\n      - agent: Generate\n        output: gdd\n      - agent: Use ${{ gdd }}\n",
    );
    let outcome = YamlFlowRuntime.run(plan, FlowContext::default(), &host).await.unwrap();
    // Cached value replays as parsed JSON and feeds later interpolation.
    assert_eq!(outcome.outputs["gdd"], json!({"cached": true}));
    // Only the second step spawned an agent.
    assert_eq!(
        *host.inner.prompts.lock().unwrap(),
        vec!["Use {\"cached\":true}".to_string()]
    );
}

#[tokio::test]
async fn failed_run_records_entries_and_rerun_resumes_without_respawn() {
    let plan = validate(
        &YamlFlowRuntime,
        "phases:\n  - id: p\n    steps:\n      - agent: One\n        output: a\n      - agent: Two\n        output: b\n      - agent: Three\n",
    );
    // First run: One/Two succeed, Three fails → their entries are retained.
    let failing = MemCheckpointHost {
        inner: MockHost::with(vec![Ok("1".into()), Ok("2".into()), Err("boom".into())]),
        ..Default::default()
    };
    let err = YamlFlowRuntime
        .run(plan.clone(), FlowContext::default(), &failing)
        .await
        .unwrap_err();
    assert!(matches!(err, FlowError::Agent { .. }));
    assert_eq!(failing.inner.prompts.lock().unwrap().len(), 3);

    // Second run with a succeeding host sharing the recorded cache: One/Two
    // hit the cache; only Three spawns.
    let succeeding = MemCheckpointHost {
        inner: MockHost::default(),
        cache: failing.cache.clone(),
    };
    let outcome = YamlFlowRuntime
        .run(plan, FlowContext::default(), &succeeding)
        .await
        .unwrap();
    assert_eq!(outcome.outputs["a"], json!(1));
    assert_eq!(outcome.outputs["b"], json!(2));
    assert_eq!(*succeeding.inner.prompts.lock().unwrap(), vec!["Three".to_string()]);
}

#[tokio::test]
async fn checkpoint_keys_follow_rendered_prompt_not_step_position() {
    let plan = validate(
        &YamlFlowRuntime,
        "phases:\n  - id: p\n    steps:\n      - agent: Do ${{ args.task }}\n        output: r\n",
    );
    let args_alpha = serde_json::Map::from_iter([("task".to_string(), json!("alpha"))]);
    let first = MemCheckpointHost::default();
    YamlFlowRuntime
        .run(plan.clone(), FlowContext { args: args_alpha }, &first)
        .await
        .unwrap();
    // Same plan, different args → different rendered prompt → no cache hit.
    let second = MemCheckpointHost::default();
    let args_beta = serde_json::Map::from_iter([("task".to_string(), json!("beta"))]);
    let outcome = YamlFlowRuntime
        .run(plan, FlowContext { args: args_beta }, &second)
        .await
        .unwrap();
    assert_eq!(*second.inner.prompts.lock().unwrap(), vec!["Do beta".to_string()]);
    assert_eq!(outcome.outputs["r"], json!("done: Do beta"));
}

#[tokio::test]
async fn checkpoint_store_roundtrip_hit_and_discard() {
    let dir = tempfile::tempdir().expect("temp ody_home");
    let home = ody_utils_absolute_path::AbsolutePathBuf::try_from(dir.path().to_path_buf())
        .expect("absolute path");
    let fp = plan_fingerprint("phases:\n  - id: x\n    steps: []\n");
    let store = CheckpointStore::open(&home, "game-create", &fp).await;
    assert_eq!(store.lookup("prompt A").await, None);
    store.record("prompt A", "out A").await;
    assert_eq!(store.lookup("prompt A").await, Some("out A".to_string()));
    // Reopen (simulates re-trigger): entries persist across store instances.
    let reopened = CheckpointStore::open(&home, "game-create", &fp).await;
    assert_eq!(reopened.lookup("prompt A").await, Some("out A".to_string()));
    // Fingerprint mismatch starts clean (changed plan ⇒ full re-run).
    let other_fp = plan_fingerprint("phases:\n  - id: y\n    steps: []\n");
    let other = CheckpointStore::open(&home, "game-create", &other_fp).await;
    assert_eq!(other.lookup("prompt A").await, None);
    // Discard removes the run file; a fresh store sees nothing.
    reopened.discard().await;
    let after = CheckpointStore::open(&home, "game-create", &fp).await;
    assert_eq!(after.lookup("prompt A").await, None);
    assert!(!dir.path().join("flow-checkpoints").join(format!("game-create-{fp}.json")).exists());
}
