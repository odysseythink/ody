use super::*;
use pretty_assertions::assert_eq;

#[test]
fn preset_names_use_mode_display_names() {
    assert_eq!(plan_preset().name, ModeKind::Plan.display_name());
    assert_eq!(default_preset().name, ModeKind::Default.display_name());
    assert_eq!(plan_preset().model, None);
    assert_eq!(
        plan_preset().reasoning_effort,
        Some(Some(ReasoningEffort::Medium))
    );
    assert_eq!(default_preset().model, None);
    assert_eq!(default_preset().reasoning_effort, None);
}

#[test]
fn plan_mode_instructions_anchor_plan_file_to_plans_directory() {
    let plan_instructions = plan_preset()
        .developer_instructions
        .expect("plan preset should include instructions")
        .expect("plan instructions should be set");

    assert!(plan_instructions.contains(".ody-code/plans/"));
    assert!(plan_instructions.contains("YYYY-MM-DD-<topic>.md"));
    assert!(plan_instructions.contains("Do NOT place plan files under `.ody-code/roadmaps/"));
}

#[test]
fn design_preset_uses_design_mode_and_name() {
    let preset = design_preset();
    assert_eq!(preset.name, ModeKind::Design.display_name());
    assert_eq!(preset.name, "Design");
    assert_eq!(preset.mode, Some(ModeKind::Design));
    assert_eq!(preset.model, None);
    assert_eq!(
        preset.reasoning_effort,
        Some(Some(ReasoningEffort::Medium))
    );
}

#[test]
fn design_mode_instructions_anchor_design_file_to_designs_directory() {
    let design_instructions = design_preset()
        .developer_instructions
        .expect("design preset should include instructions")
        .expect("design instructions should be set");

    assert!(design_instructions.contains(".ody-code/designs/"));
    assert!(design_instructions.contains("YYYY-MM-DD-<topic>.md"));
    assert!(
        !design_instructions.contains(".ody-code/roadmaps/"),
        "design instructions must not point at the roadmaps directory"
    );
    // Hard exit gates must be referenced so the model cannot skip them.
    assert!(design_instructions.contains("<HARD-GATE>"));
    assert!(design_instructions.contains("## Reuse Analysis"));
    assert!(design_instructions.contains("## Self-Review"));
    assert!(design_instructions.contains("[C:UPSTREAM]"));
}

#[test]
fn builtin_presets_include_design_between_plan_and_default() {
    let presets = builtin_collaboration_mode_presets();
    let modes: Vec<Option<ModeKind>> = presets.into_iter().map(|preset| preset.mode).collect();
    assert_eq!(
        modes,
        vec![
            Some(ModeKind::Plan),
            Some(ModeKind::Design),
            Some(ModeKind::Default),
        ]
    );
}

#[test]
fn default_mode_instructions_replace_mode_names_placeholder() {
    let default_instructions = default_preset()
        .developer_instructions
        .expect("default preset should include instructions")
        .expect("default instructions should be set");

    assert!(!default_instructions.contains("{{KNOWN_MODE_NAMES}}"));

    let known_mode_names = format_mode_names(&TUI_VISIBLE_COLLABORATION_MODES);
    let expected_snippet = format!("Known mode names are {known_mode_names}.");
    assert!(default_instructions.contains(&expected_snippet));

    assert!(default_instructions.contains(
        "Use the `request_user_input` tool only when it is listed in the available tools"
    ));
    assert!(
        default_instructions.contains("ask the user directly with a concise plain-text question")
    );
}
