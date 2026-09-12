use super::*;

#[test]
fn parses_phases_and_steps() {
    let plan = parse_flow_plan(
        r#"
phases:
  - id: design
    steps:
      - agent: Generate a GDD.
        output: gdd
  - id: implement
    steps:
      - pipeline: ${{ gdd.mechanics }}
        each: Implement ${item}.
        output: implementations
      - parallel:
          - agent: Review the code.
          - agent: Run the tests.
        output: review
"#,
    )
    .expect("valid flow plan should parse");

    assert_eq!(plan.phases.len(), 2);
    assert_eq!(plan.phases[0].id, "design");
    assert_eq!(plan.phases[0].steps.len(), 1);
    assert!(matches!(
        &plan.phases[0].steps[0],
        FlowStep::Agent { agent, output, schema }
            if agent == "Generate a GDD." && output.as_deref() == Some("gdd") && schema.is_none()
    ));
    assert_eq!(plan.phases[1].steps.len(), 2);
    assert!(matches!(
        &plan.phases[1].steps[0],
        FlowStep::Pipeline { pipeline, each, .. }
            if pipeline == "${{ gdd.mechanics }}" && each == "Implement ${item}."
    ));
    assert!(matches!(
        &plan.phases[1].steps[1],
        FlowStep::Parallel { parallel, .. } if parallel.len() == 2
    ));
}

#[test]
fn rejects_empty_phases() {
    assert!(parse_flow_plan("phases: []\n").is_err());
}

#[test]
fn rejects_plan_without_phases() {
    assert!(parse_flow_plan("name: nothing\n").is_err());
}

#[test]
fn rejects_unknown_step_kind() {
    assert!(
        parse_flow_plan("phases:\n  - id: x\n    steps:\n      - bogus: do something\n").is_err()
    );
}

#[test]
fn rejects_pipeline_step_without_each() {
    assert!(parse_flow_plan("phases:\n  - id: x\n    steps:\n      - pipeline: items\n").is_err());
}

#[test]
fn rejects_step_with_unknown_field() {
    assert!(
        parse_flow_plan(
            "phases:\n  - id: x\n    steps:\n      - agent: do it\n        agnet: typo\n"
        )
        .is_err()
    );
}


#[test]
fn accepts_optional_top_level_name_and_description() {
    let plan = parse_flow_plan(
        "name: game-create\ndescription: 从概念到可运行原型\nphases:\n  - id: x\n    steps:\n      - agent: do it\n",
    )
    .expect("top-level name/description should be accepted as informational fields");
    assert_eq!(plan.name.as_deref(), Some("game-create"));
    assert_eq!(plan.description.as_deref(), Some("从概念到可运行原型"));
    assert_eq!(plan.phases.len(), 1);
}

#[test]
fn rejects_unknown_top_level_field() {
    assert!(
        parse_flow_plan("name: x\nbogus: y\nphases:\n  - id: x\n    steps:\n      - agent: do it\n")
            .is_err()
    );
}

#[test]
fn parses_agent_step_with_schema() {
    // M2.4: an agent step may carry an optional JSON-schema subset used by
    // the runtime for reply validation and retry.
    let plan = parse_flow_plan(
        r#"
phases:
  - id: design
    steps:
      - agent: Generate a GDD.
        output: gdd
        schema:
          type: object
          required: [mechanics]
          properties:
            mechanics:
              type: array
              items:
                type: object
                required: [name]
                properties:
                  name: { type: string }
"#,
    )
    .expect("agent step with schema should parse");
    let FlowStep::Agent { schema, .. } = &plan.phases[0].steps[0] else {
        panic!("expected an agent step");
    };
    let schema = schema.as_ref().expect("schema should be present");
    assert_eq!(schema["type"], serde_json::json!("object"));
    assert_eq!(
        schema["properties"]["mechanics"]["items"]["required"][0],
        serde_json::json!("name")
    );
}

#[test]
fn rejects_agent_step_with_malformed_schema_shape() {
    // `schema` must be a JSON object; scalar shapes are rejected at parse.
    assert!(
        parse_flow_plan(
            "phases:\n  - id: x\n    steps:\n      - agent: do it\n        schema: not-an-object\n"
        )
        .is_err()
    );
}
