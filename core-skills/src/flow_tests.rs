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
        FlowStep::Agent { agent, output }
            if agent == "Generate a GDD." && output.as_deref() == Some("gdd")
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
