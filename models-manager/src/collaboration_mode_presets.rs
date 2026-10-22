use ody_collaboration_mode_templates::DEFAULT as COLLABORATION_MODE_DEFAULT;
use ody_collaboration_mode_templates::DESIGN as COLLABORATION_MODE_DESIGN;
use ody_collaboration_mode_templates::FRONTEND as COLLABORATION_MODE_FRONTEND;
use ody_collaboration_mode_templates::GAME as COLLABORATION_MODE_GAME;
use ody_collaboration_mode_templates::PLAN as COLLABORATION_MODE_PLAN;
use ody_collaboration_mode_templates::PRODUCT as COLLABORATION_MODE_PRODUCT;
use ody_protocol::config_types::CollaborationModeMask;
use ody_protocol::config_types::ModeAvailability;
use ody_protocol::config_types::ModeKind;
use ody_protocol::config_types::TUI_VISIBLE_COLLABORATION_MODES;
use ody_protocol::model_metadata::ReasoningEffort;
use ody_utils_template::Template;
use std::sync::LazyLock;

const KNOWN_MODE_NAMES_TEMPLATE_KEY: &str = "KNOWN_MODE_NAMES";
static COLLABORATION_MODE_DEFAULT_TEMPLATE: LazyLock<Template> = LazyLock::new(|| {
    Template::parse(COLLABORATION_MODE_DEFAULT)
        .unwrap_or_else(|err| panic!("collaboration mode default template must parse: {err}"))
});

/// The always-on presets. Opt-in modes are returned separately by
/// [`opt_in_collaboration_mode_presets`] so a caller that has no availability
/// information cannot accidentally advertise a mode the user cannot enter.
pub fn builtin_collaboration_mode_presets() -> Vec<CollaborationModeMask> {
    vec![
        plan_preset(),
        design_preset(),
        product_preset(),
        default_preset(),
    ]
}

/// Presets for modes gated behind an explicit opt-in (`--game`, `--frontend`).
pub fn opt_in_collaboration_mode_presets() -> Vec<CollaborationModeMask> {
    vec![game_preset(), frontend_preset()]
}

/// Every preset this invocation may offer, in ring order.
pub fn collaboration_mode_presets_for(
    availability: ModeAvailability,
) -> Vec<CollaborationModeMask> {
    let mut presets = builtin_collaboration_mode_presets();
    presets.extend(
        opt_in_collaboration_mode_presets()
            .into_iter()
            .filter(|mask| {
                mask.mode
                    .is_some_and(|mode| availability.is_available(mode))
            }),
    );
    presets
}

/// The preset for `mode`, regardless of whether the invocation enabled it.
///
/// Callers that build a mode directly from configuration (the session's initial
/// mode) need the preset's developer instructions even though no client ever
/// listed it, so this deliberately ignores availability.
pub fn preset_for_mode(mode: ModeKind) -> Option<CollaborationModeMask> {
    builtin_collaboration_mode_presets()
        .into_iter()
        .chain(opt_in_collaboration_mode_presets())
        .find(|mask| mask.mode == Some(mode))
}

fn frontend_preset() -> CollaborationModeMask {
    CollaborationModeMask {
        name: ModeKind::Frontend.display_name().to_string(),
        mode: Some(ModeKind::Frontend),
        model: None,
        reasoning_effort: Some(Some(ReasoningEffort::Medium)),
        developer_instructions: Some(Some(COLLABORATION_MODE_FRONTEND.to_string())),
        design_audit_level: None,
    }
}

fn game_preset() -> CollaborationModeMask {
    CollaborationModeMask {
        name: ModeKind::Game.display_name().to_string(),
        mode: Some(ModeKind::Game),
        model: None,
        reasoning_effort: Some(Some(ReasoningEffort::Medium)),
        developer_instructions: Some(Some(COLLABORATION_MODE_GAME.to_string())),
        design_audit_level: None,
    }
}

fn product_preset() -> CollaborationModeMask {
    CollaborationModeMask {
        name: ModeKind::Product.display_name().to_string(),
        mode: Some(ModeKind::Product),
        model: None,
        reasoning_effort: Some(Some(ReasoningEffort::Medium)),
        developer_instructions: Some(Some(COLLABORATION_MODE_PRODUCT.to_string())),
        design_audit_level: None,
    }
}

fn plan_preset() -> CollaborationModeMask {
    CollaborationModeMask {
        name: ModeKind::Plan.display_name().to_string(),
        mode: Some(ModeKind::Plan),
        model: None,
        reasoning_effort: Some(Some(ReasoningEffort::Medium)),
        developer_instructions: Some(Some(COLLABORATION_MODE_PLAN.to_string())),
        design_audit_level: None,
    }
}

fn design_preset() -> CollaborationModeMask {
    CollaborationModeMask {
        name: ModeKind::Design.display_name().to_string(),
        mode: Some(ModeKind::Design),
        model: None,
        reasoning_effort: Some(Some(ReasoningEffort::Medium)),
        developer_instructions: Some(Some(COLLABORATION_MODE_DESIGN.to_string())),
        design_audit_level: None,
    }
}

fn default_preset() -> CollaborationModeMask {
    CollaborationModeMask {
        name: ModeKind::Default.display_name().to_string(),
        mode: Some(ModeKind::Default),
        model: None,
        reasoning_effort: None,
        developer_instructions: Some(Some(default_mode_instructions())),
        design_audit_level: None,
    }
}

fn default_mode_instructions() -> String {
    let known_mode_names = format_mode_names(&TUI_VISIBLE_COLLABORATION_MODES);
    COLLABORATION_MODE_DEFAULT_TEMPLATE
        .render([(KNOWN_MODE_NAMES_TEMPLATE_KEY, known_mode_names.as_str())])
        .unwrap_or_else(|err| panic!("collaboration mode default template must render: {err}"))
}

fn format_mode_names(modes: &[ModeKind]) -> String {
    let mode_names: Vec<&str> = modes.iter().map(|mode| mode.display_name()).collect();
    match mode_names.as_slice() {
        [] => "none".to_string(),
        [mode_name] => (*mode_name).to_string(),
        [first, second] => format!("{first} and {second}"),
        [..] => mode_names.join(", "),
    }
}

#[cfg(test)]
#[path = "collaboration_mode_presets_tests.rs"]
mod tests;
