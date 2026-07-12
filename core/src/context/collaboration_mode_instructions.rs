use super::ContextualUserFragment;
use crate::plan_artifact::PlanArtifact;
use crate::plan_mode_tier_selector::PlanModeTierSelector;
use ody_config::config_toml::{PlanModeConfigToml, PlanModeTier};
use ody_protocol::config_types::CollaborationMode;
use ody_protocol::config_types::ModeKind;
use ody_protocol::protocol::COLLABORATION_MODE_CLOSE_TAG;
use ody_protocol::protocol::COLLABORATION_MODE_OPEN_TAG;

const SPLIT_THRESHOLD_TEMPLATE_KEY: &str = "split_threshold";

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CollaborationModeInstructions {
    instructions: String,
}

impl CollaborationModeInstructions {
    pub(crate) fn from_collaboration_mode(
        collaboration_mode: &CollaborationMode,
        split_threshold: Option<usize>,
        user_prompt: Option<&str>,
        plan_mode_config: Option<&PlanModeConfigToml>,
        plan_artifact: Option<&PlanArtifact>,
    ) -> Option<Self> {
        let instructions = collaboration_mode
            .settings
            .developer_instructions
            .as_ref()
            .filter(|instructions| !instructions.is_empty())?;

        let rendered = if matches!(
            collaboration_mode.mode,
            ModeKind::Plan | ModeKind::Design
        ) && instructions.contains(SPLIT_THRESHOLD_TEMPLATE_KEY)
        {
            render_plan_instructions(instructions, split_threshold)
        } else {
            instructions.clone()
        };

        let mut this = Self { instructions: rendered };

        if collaboration_mode.mode == ModeKind::Plan {
            let tier = resolve_plan_mode_tier(user_prompt, plan_mode_config, plan_artifact);
            if tier == PlanModeTier::Rigor {
                this = this
                    .with_rigor_workflow()
                    .with_rigor_coverage()
                    .with_rigor_task_skeleton()
                    .with_rigor_selfreview()
                    .with_rigor_invariants()
                    .with_rigor_grounding()
                    .with_rigor_scope()
                    .with_rigor_rename()
                    .with_rigor_risks()
                    .with_rigor_split()
                    .with_rigor_turn_discipline();
            }
        }

        Some(this)
    }

    pub(crate) fn with_rigor_coverage(self) -> Self {
        let fragment = ody_collaboration_mode_templates::PLAN_RIGOR_COVERAGE;
        Self {
            instructions: format!("{}\n\n{}", self.instructions, fragment),
        }
    }

    pub(crate) fn with_rigor_selfreview(self) -> Self {
        let fragment = ody_collaboration_mode_templates::PLAN_RIGOR_SELFREVIEW;
        Self {
            instructions: format!("{}\n\n{}", self.instructions, fragment),
        }
    }

    pub(crate) fn with_rigor_invariants(self) -> Self {
        let fragment = ody_collaboration_mode_templates::PLAN_RIGOR_INVARIANTS;
        Self {
            instructions: format!("{}\n\n{}", self.instructions, fragment),
        }
    }

    pub(crate) fn with_rigor_grounding(self) -> Self {
        let fragment = ody_collaboration_mode_templates::PLAN_RIGOR_GROUNDING;
        Self {
            instructions: format!("{}\n\n{}", self.instructions, fragment),
        }
    }

    pub(crate) fn with_rigor_rename(self) -> Self {
        let fragment = ody_collaboration_mode_templates::PLAN_RIGOR_RENAME;
        Self {
            instructions: format!("{}\n\n{}", self.instructions, fragment),
        }
    }

    pub(crate) fn with_rigor_scope(self) -> Self {
        let fragment = ody_collaboration_mode_templates::PLAN_RIGOR_SCOPE;
        Self {
            instructions: format!("{}\n\n{}", self.instructions, fragment),
        }
    }

    pub(crate) fn with_rigor_workflow(self) -> Self {
        let fragment = ody_collaboration_mode_templates::PLAN_RIGOR_WORKFLOW;
        Self {
            instructions: format!("{}\n\n{}", self.instructions, fragment),
        }
    }

    pub(crate) fn with_rigor_task_skeleton(self) -> Self {
        let fragment = ody_collaboration_mode_templates::PLAN_RIGOR_TASK_SKELETON;
        Self {
            instructions: format!("{}\n\n{}", self.instructions, fragment),
        }
    }

    pub(crate) fn with_rigor_risks(self) -> Self {
        let fragment = ody_collaboration_mode_templates::PLAN_RIGOR_RISKS;
        Self {
            instructions: format!("{}\n\n{}", self.instructions, fragment),
        }
    }

    pub(crate) fn with_rigor_split(self) -> Self {
        let fragment = ody_collaboration_mode_templates::PLAN_RIGOR_SPLIT;
        Self {
            instructions: format!("{}\n\n{}", self.instructions, fragment),
        }
    }

    pub(crate) fn with_rigor_turn_discipline(self) -> Self {
        let fragment = ody_collaboration_mode_templates::PLAN_RIGOR_TURN_DISCIPLINE;
        Self {
            instructions: format!("{}\n\n{}", self.instructions, fragment),
        }
    }
}

fn render_plan_instructions(instructions: &str, split_threshold: Option<usize>) -> String {
    let template = match ody_utils_template::Template::parse(instructions) {
        Ok(template) => template,
        Err(err) => {
            tracing::warn!("plan mode instructions template parse error: {err}");
            return instructions.to_string();
        }
    };

    if !template
        .placeholders()
        .any(|name| name == SPLIT_THRESHOLD_TEMPLATE_KEY)
    {
        return instructions.to_string();
    }

    let value = split_threshold.map_or_else(|| "8".to_string(), |v| v.to_string());
    match template.render([(SPLIT_THRESHOLD_TEMPLATE_KEY, value.as_str())]) {
        Ok(rendered) => rendered,
        Err(err) => {
            tracing::warn!("plan mode instructions template render error: {err}");
            instructions.to_string()
        }
    }
}

fn resolve_plan_mode_tier(
    user_prompt: Option<&str>,
    plan_mode_config: Option<&PlanModeConfigToml>,
    plan_artifact: Option<&PlanArtifact>,
) -> PlanModeTier {
    if let Some(tier) = plan_artifact.and_then(|a| a.plan_mode_tier()) {
        return tier;
    }

    if let Some(prompt) = user_prompt {
        let selector = PlanModeTierSelector::new(plan_mode_config);
        let selection = selector.select_tier(prompt);
        if let Some(artifact) = plan_artifact {
            artifact.set_plan_mode_tier(selection.tier);
        }
        return selection.tier;
    }

    plan_mode_config
        .and_then(|c| c.tier)
        .filter(|t| *t != PlanModeTier::Auto)
        .unwrap_or(PlanModeTier::Concise)
}

impl ContextualUserFragment for CollaborationModeInstructions {
    fn role(&self) -> &'static str {
        "developer"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        (COLLABORATION_MODE_OPEN_TAG, COLLABORATION_MODE_CLOSE_TAG)
    }

    fn body(&self) -> String {
        self.instructions.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ody_protocol::config_types::CollaborationMode;
    use ody_protocol::config_types::ModeKind;
    use ody_protocol::config_types::Settings;

    fn plan_mode_with_instructions(instructions: &str) -> CollaborationMode {
        CollaborationMode {
            mode: ModeKind::Plan,
            settings: Settings {
                model: "test-model".to_string(),
                reasoning_effort: None,
                developer_instructions: Some(instructions.to_string()),
            },
        }
    }

    #[test]
    fn renders_split_threshold_placeholder() {
        let mode = plan_mode_with_instructions(
            "Split plans larger than {{ split_threshold }} tasks."
        );
        let instructions = CollaborationModeInstructions::from_collaboration_mode(&mode, Some(8), None, None, None)
            .expect("should produce instructions");
        assert_eq!(instructions.body(), "Split plans larger than 8 tasks.");
    }

    #[test]
    fn renders_split_threshold_placeholder_for_design_mode() {
        let mode = CollaborationMode {
            mode: ModeKind::Design,
            settings: Settings {
                model: "test-model".to_string(),
                reasoning_effort: None,
                developer_instructions: Some(
                    "Split designs larger than {{ split_threshold }} subsystems.".to_string(),
                ),
            },
        };
        let instructions = CollaborationModeInstructions::from_collaboration_mode(
            &mode,
            Some(8),
            None,
            None,
            None,
        )
        .expect("should produce instructions");
        assert_eq!(
            instructions.body(),
            "Split designs larger than 8 subsystems."
        );
    }

    #[test]
    fn design_split_threshold_uses_default_when_config_absent() {
        let mode = CollaborationMode {
            mode: ModeKind::Design,
            settings: Settings {
                model: "test-model".to_string(),
                reasoning_effort: None,
                developer_instructions: Some(
                    "Split designs larger than {{ split_threshold }} subsystems.".to_string(),
                ),
            },
        };
        let instructions = CollaborationModeInstructions::from_collaboration_mode(
            &mode,
            None,
            None,
            None,
            None,
        )
        .expect("should produce instructions");
        assert_eq!(
            instructions.body(),
            "Split designs larger than 8 subsystems.",
            "when split_threshold is None, Design mode should fall back to the same default as Plan mode"
        );
    }

    #[test]
    fn design_mode_does_not_compose_plan_rigor_fragments() {
        let mode = CollaborationMode {
            mode: ModeKind::Design,
            settings: Settings {
                model: "test-model".to_string(),
                reasoning_effort: None,
                developer_instructions: Some("Design the work.".to_string()),
            },
        };
        let config = PlanModeConfigToml {
            tier: Some(PlanModeTier::Rigor),
            ..Default::default()
        };
        let instructions = CollaborationModeInstructions::from_collaboration_mode(
            &mode,
            Some(8),
            None,
            Some(&config),
            None,
        )
        .expect("should produce instructions");
        let body = instructions.body();
        assert_eq!(body, "Design the work.");
        assert!(
            !body.contains("## Dependency Overview"),
            "design mode must not compose plan rigor fragments:\n{body}"
        );
    }

    #[test]
    fn leaves_non_plan_instructions_unrendered() {
        let mode = CollaborationMode {
            mode: ModeKind::Default,
            settings: Settings {
                model: "test-model".to_string(),
                reasoning_effort: None,
                developer_instructions: Some("Hello {{ split_threshold }}".to_string()),
            },
        };
        let instructions = CollaborationModeInstructions::from_collaboration_mode(&mode, Some(8), None, None, None)
            .expect("should produce instructions");
        // Default mode should not attempt to render the plan placeholder.
        assert_eq!(instructions.body(), "Hello {{ split_threshold }}");
    }

    #[test]
    fn no_placeholder_passes_through_unchanged() {
        let mode = plan_mode_with_instructions("Stay focused.");
        let instructions = CollaborationModeInstructions::from_collaboration_mode(&mode, Some(8), None, None, None)
            .expect("should produce instructions");
        assert_eq!(instructions.body(), "Stay focused.");
    }

    #[test]
    fn composes_rigor_coverage_fragment() {
        let mode = plan_mode_with_instructions("Plan the work.");
        let instructions = CollaborationModeInstructions::from_collaboration_mode(&mode, Some(8), None, None, None)
            .expect("should produce instructions")
            .with_rigor_coverage();
        let body = instructions.body();
        assert!(
            body.contains("## Dependency Overview"),
            "body should contain Dependency Overview section:\n{body}"
        );
        assert!(
            body.contains("## Spec-coverage table"),
            "body should contain Spec-coverage table section:\n{body}"
        );
    }

    #[test]
    fn composes_rigor_selfreview_fragment() {
        let mode = plan_mode_with_instructions("Plan the work.");
        let instructions = CollaborationModeInstructions::from_collaboration_mode(&mode, Some(8), None, None, None)
            .expect("should produce instructions")
            .with_rigor_selfreview();
        let body = instructions.body();

        for (n, label) in [
            (1, "Spec-coverage table"),
            (2, "Placeholder scan"),
            (3, "No phantom tasks"),
            (4, "Dependency soundness"),
            (5, "Caller & build soundness"),
            (6, "Test-the-risk"),
            (7, "Type consistency"),
        ] {
            assert!(
                body.contains(&format!("{n}. {label}")),
                "body should contain self-review item {n} ({label}):\n{body}"
            );
        }

        assert!(
            body.contains("trace one concrete value"),
            "body should contain the end-to-end trace requirement:\n{body}"
        );
    }

    #[test]
    fn composes_rigor_invariants_fragment() {
        let mode = plan_mode_with_instructions("Plan the work.");
        let instructions = CollaborationModeInstructions::from_collaboration_mode(&mode, Some(8), None, None, None)
            .expect("should produce instructions")
            .with_rigor_invariants();
        let body = instructions.body();

        assert!(
            body.contains("## Shared-signature build-green invariant"),
            "body should contain Shared-signature section:\n{body}"
        );
        assert!(
            body.contains("## No-placeholders rule"),
            "body should contain No-placeholders section:\n{body}"
        );
        assert!(
            body.contains("cargo check --workspace --all-targets"),
            "body should contain whole-tree typecheck command:\n{body}"
        );
        assert!(
            body.contains("TODO"),
            "body should list TODO as a forbidden placeholder:\n{body}"
        );
        assert!(
            body.contains("TBD"),
            "body should list TBD as a forbidden placeholder:\n{body}"
        );
    }

    #[test]
    fn composes_rigor_grounding_fragment() {
        let mode = plan_mode_with_instructions("Plan the work.");
        let instructions = CollaborationModeInstructions::from_collaboration_mode(&mode, Some(8), None, None, None)
            .expect("should produce instructions")
            .with_rigor_grounding();
        let body = instructions.body();

        assert!(
            body.contains("## Source-grounding mandate"),
            "body should contain Source-grounding mandate section:\n{body}"
        );
        assert!(
            body.contains("Read") && body.contains("Grep"),
            "body should mandate Read and Grep verification:\n{body}"
        );
        assert!(
            body.contains("Never infer scope from names"),
            "body should contain the anti-name-inference rule:\n{body}"
        );
        assert!(
            body.contains("[C:INFERRED]"),
            "body should contain the inferred-assumption label:\n{body}"
        );
        assert!(
            body.contains("creator_account_user_id"),
            "body should include the external-schema carve-out example:\n{body}"
        );
    }

    #[test]
    fn composes_rigor_rename_fragment() {
        let mode = plan_mode_with_instructions("Plan the work.");
        let instructions = CollaborationModeInstructions::from_collaboration_mode(&mode, Some(8), None, None, None)
            .expect("should produce instructions")
            .with_rigor_rename();
        let body = instructions.body();

        assert!(
            body.contains("## Rename-vs-delete decision prompt"),
            "body should contain Rename-vs-delete section:\n{body}"
        );
        assert!(
            body.contains("Delete") && body.contains("Rename") && body.contains("Carve out"),
            "body should list all three per-hit actions:\n{body}"
        );
        assert!(
            body.contains("one-line reason"),
            "body should require a one-line reason:\n{body}"
        );
        assert!(
            body.contains("No silent survivors"),
            "body should contain the no-silent-survivors rule:\n{body}"
        );
    }

    #[test]
    fn composes_rigor_scope_fragment() {
        let mode = plan_mode_with_instructions("Plan the work.");
        let instructions = CollaborationModeInstructions::from_collaboration_mode(&mode, Some(8), None, None, None)
            .expect("should produce instructions")
            .with_rigor_scope();
        let body = instructions.body();

        assert!(
            body.contains("## Out-of-scope / false-positive discipline"),
            "body should contain Out-of-scope / false-positive discipline section:\n{body}"
        );
        assert!(
            body.contains("## Out-of-scope"),
            "body should contain the mandatory Out-of-scope section heading:\n{body}"
        );
        assert!(
            body.contains("false-positive"),
            "body should mention false-positive discipline:\n{body}"
        );
        assert!(
            body.contains("windows-sandbox-rs"),
            "body should include the windows-sandbox carve-out example:\n{body}"
        );
        assert!(
            body.contains("ext/goal/accounting.rs"),
            "body should include the token-accounting carve-out example:\n{body}"
        );
        assert!(
            body.contains("creator_account_user_id"),
            "body should include the external-schema carve-out example:\n{body}"
        );
    }

#[test]
fn rigor_tier_composes_all_fragments() {
    let mode = plan_mode_with_instructions("Plan the work.");
    let prompt = r#"
1. Refactor auth
2. Migrate payments
3. Remove account concept
4. Delete legacy schema
5. Rename user_id
6. Extract helpers
7. Merge account types
8. Redesign session
9. Update connectors/ody-mcp
10. Sweep insta snapshots
"#;
    let instructions = CollaborationModeInstructions::from_collaboration_mode(
        &mode,
        Some(8),
        Some(prompt),
        None,
        None,
    )
    .expect("should produce instructions");
    let body = instructions.body();
    assert!(body.contains("## Dependency Overview"));
    assert!(body.contains("## Spec-coverage table"));
    assert!(body.contains("## Shared-signature build-green invariant"));
    assert!(body.contains("## Source-grounding mandate"));
    assert!(body.contains("## Out-of-scope / false-positive discipline"));
    assert!(body.contains("## Rename-vs-delete decision prompt"));
    assert!(body.contains(
        "## Rigor tier addendum: Large plan splitting & Parts manifest"
    ));
    assert!(body.contains(
        "## Rigor tier addendum: Turn discipline (when to submit the plan)"
    ));
}

#[test]
fn all_prompts_now_generate_rigor_tier_with_workflow() {
    let mode = plan_mode_with_instructions("Plan the work.");
    let instructions = CollaborationModeInstructions::from_collaboration_mode(
        &mode,
        Some(8),
        Some("fix typo in README"),
        None,
        None,
    )
    .expect("should produce instructions");
    let body = instructions.body();
    // After removing heuristic tier selector, all prompts generate Rigor tier
    assert!(body.contains("## Rigor tier addendum: Structured workflow"));
    assert!(body.contains("## Dependency Overview"));
}

#[test]
fn split_threshold_rendered_with_rigor_fragments() {
    let mode = plan_mode_with_instructions(
        "Split plans larger than {{ split_threshold }} tasks."
    );

    let instructions = CollaborationModeInstructions::from_collaboration_mode(
        &mode,
        Some(12),
        Some("fix typo"),
        None,
        None,
    )
    .expect("should produce instructions");
    let body = instructions.body();
    // Both cases now include split_threshold rendering + rigor fragments
    assert!(body.contains("Split plans larger than 12 tasks."));
    assert!(body.contains("## Rigor tier addendum: Structured workflow"));
}

#[test]
fn config_override_rigor_applies_without_prompt() {
    let mode = plan_mode_with_instructions("Plan the work.");
    let config = PlanModeConfigToml {
        tier: Some(PlanModeTier::Rigor),
        ..Default::default()
    };
    let instructions = CollaborationModeInstructions::from_collaboration_mode(
        &mode,
        Some(8),
        None,
        Some(&config),
        None,
    )
    .expect("should produce instructions");
    assert!(instructions.body().contains("## Dependency Overview"));
}

#[test]
fn artifact_tier_reused_without_prompt() {
    use ody_protocol::ThreadId;
    use ody_utils_absolute_path::AbsolutePathBuf;

    let mode = plan_mode_with_instructions("Plan the work.");
    let tmp = tempfile::tempdir().unwrap();
    let plans_base_dir = AbsolutePathBuf::from_absolute_path(tmp.path()).unwrap();
    let thread_id = ThreadId::from_string("00000000-0000-0000-0000-000000000001").unwrap();
    let artifact = crate::plan_artifact::PlanArtifact::new_temp(
        plans_base_dir,
        thread_id,
        "2026-07-04",
    );
    artifact.set_plan_mode_tier(PlanModeTier::Rigor);

    let instructions = CollaborationModeInstructions::from_collaboration_mode(
        &mode,
        Some(8),
        None,
        None,
        Some(&artifact),
    )
    .expect("should produce instructions");
    assert!(instructions.body().contains("## Dependency Overview"));
}

}
