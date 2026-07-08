use super::ContextualUserFragment;
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
    ) -> Option<Self> {
        let instructions = collaboration_mode
            .settings
            .developer_instructions
            .as_ref()
            .filter(|instructions| !instructions.is_empty())?;

        let rendered = if collaboration_mode.mode == ModeKind::Plan
            && instructions.contains(SPLIT_THRESHOLD_TEMPLATE_KEY)
        {
            render_plan_instructions(instructions, split_threshold)
        } else {
            instructions.clone()
        };

        Some(Self { instructions: rendered })
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

    pub(crate) fn with_rigor_scope(self) -> Self {
        let fragment = ody_collaboration_mode_templates::PLAN_RIGOR_SCOPE;
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
        let instructions = CollaborationModeInstructions::from_collaboration_mode(&mode, Some(8))
            .expect("should produce instructions");
        assert_eq!(instructions.body(), "Split plans larger than 8 tasks.");
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
        let instructions = CollaborationModeInstructions::from_collaboration_mode(&mode, Some(8))
            .expect("should produce instructions");
        // Default mode should not attempt to render the plan placeholder.
        assert_eq!(instructions.body(), "Hello {{ split_threshold }}");
    }

    #[test]
    fn no_placeholder_passes_through_unchanged() {
        let mode = plan_mode_with_instructions("Stay focused.");
        let instructions = CollaborationModeInstructions::from_collaboration_mode(&mode, Some(8))
            .expect("should produce instructions");
        assert_eq!(instructions.body(), "Stay focused.");
    }

    #[test]
    fn composes_rigor_coverage_fragment() {
        let mode = plan_mode_with_instructions("Plan the work.");
        let instructions = CollaborationModeInstructions::from_collaboration_mode(&mode, Some(8))
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
        let instructions = CollaborationModeInstructions::from_collaboration_mode(&mode, Some(8))
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
        let instructions = CollaborationModeInstructions::from_collaboration_mode(&mode, Some(8))
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
        let instructions = CollaborationModeInstructions::from_collaboration_mode(&mode, Some(8))
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
    fn composes_rigor_scope_fragment() {
        let mode = plan_mode_with_instructions("Plan the work.");
        let instructions = CollaborationModeInstructions::from_collaboration_mode(&mode, Some(8))
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
}
