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
}
