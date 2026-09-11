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
