use ody_protocol::config_types::ModeKind;
use ody_protocol::protocol::SkillScope;
use ody_utils_absolute_path::AbsolutePathBuf;

use crate::model::SkillLoadOutcome;
use crate::model::SkillMetadata;
use crate::model::SkillType;

#[test]
fn skill_metadata_supports_ody_code_aligned_fields() {
    let path = AbsolutePathBuf::try_from("/tmp/skills/review/SKILL.md").unwrap();
    let meta = SkillMetadata {
        name: "security-review".to_string(),
        description: "Review code for security issues.".to_string(),
        short_description: Some("Security review".to_string()),
        interface: None,
        dependencies: None,
        policy: None,
        path_to_skills_md: path,
        flow_artifact: None,
        scope: ody_protocol::protocol::SkillScope::User,
        plugin_id: None,
        skill_type: SkillType::Knowledge,
        triggers: vec!["review".to_string(), "security".to_string()],
        hidden_in_modes: vec![ModeKind::Plan],
        disable_model_invocation: true,
        mermaid: None,
        d2: None,
    };
    assert!(matches!(meta.skill_type, SkillType::Knowledge));
    assert_eq!(meta.triggers, vec!["review", "security"]);
    assert!(meta.hidden_in_modes.contains(&ModeKind::Plan));
    assert!(meta.disable_model_invocation);
    assert!(!meta.is_model_invocable(ModeKind::Default));
    assert!(!meta.is_model_invocable(ModeKind::Plan));
}

#[test]
fn skill_metadata_inline_is_model_invocable_by_default() {
    let path = AbsolutePathBuf::try_from("/tmp/skills/inline/SKILL.md").unwrap();
    let meta = SkillMetadata {
        name: "inline-skill".to_string(),
        description: "An inline skill.".to_string(),
        short_description: None,
        interface: None,
        dependencies: None,
        policy: None,
        path_to_skills_md: path,
        flow_artifact: None,
        scope: ody_protocol::protocol::SkillScope::User,
        plugin_id: None,
        skill_type: SkillType::Inline,
        triggers: Vec::new(),
        hidden_in_modes: Vec::new(),
        disable_model_invocation: false,
        mermaid: None,
        d2: None,
    };
    assert!(meta.is_model_invocable(ModeKind::Default));
    assert!(meta.is_model_invocable(ModeKind::Plan));
}

fn skill_with_scope(name: &str, scope: SkillScope) -> SkillMetadata {
    SkillMetadata {
        name: name.to_string(),
        path_to_skills_md: AbsolutePathBuf::try_from(format!("/tmp/skills/{name}/SKILL.md"))
            .unwrap(),
        scope,
        ..Default::default()
    }
}

#[test]
fn game_filter_hides_only_system_scope_game_skills() {
    let outcome = SkillLoadOutcome {
        skills: vec![
            skill_with_scope("game", SkillScope::System),
            skill_with_scope("godot-shaders", SkillScope::System),
            skill_with_scope("debt-ledger", SkillScope::System),
            // A user-installed skill that happens to share a game skill name
            // must survive: the gate only applies to the embedded bundle.
            skill_with_scope("game", SkillScope::User),
        ],
        ..Default::default()
    };

    let filtered = crate::model::filter_skill_load_outcome_for_game(
        outcome.clone(),
        /*game_skills_enabled*/ false,
    );
    let names: Vec<(&str, SkillScope)> = filtered
        .skills
        .iter()
        .map(|skill| (skill.name.as_str(), skill.scope))
        .collect();
    assert_eq!(
        names,
        vec![
            ("debt-ledger", SkillScope::System),
            ("game", SkillScope::User),
        ]
    );

    let unfiltered =
        crate::model::filter_skill_load_outcome_for_game(outcome, /*game_skills_enabled*/ true);
    assert_eq!(unfiltered.skills.len(), 4);
}
