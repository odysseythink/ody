use ody_protocol::config_types::ModeKind;
use ody_utils_absolute_path::AbsolutePathBuf;

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
