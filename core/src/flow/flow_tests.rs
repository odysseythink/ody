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
