use ody_core_skills::model::SkillType;
use ody_protocol::config_types::ModeKind;

use crate::catalog::SkillAuthority;
use crate::catalog::SkillCatalog;
use crate::catalog::SkillCatalogEntry;
use crate::catalog::SkillPackageId;
use crate::catalog::SkillResourceId;
use crate::catalog::SkillSourceKind;

fn host_authority() -> SkillAuthority {
    SkillAuthority::new(SkillSourceKind::Host, "host")
}

fn test_entry(name: &str, skill_type: SkillType) -> SkillCatalogEntry {
    SkillCatalogEntry::new(
        SkillPackageId(format!("pkg/{name}")),
        host_authority(),
        name.to_string(),
        format!("{name} skill"),
        SkillResourceId::new(format!("{name}/SKILL.md")),
    )
    .with_skill_type(skill_type)
}

#[test]
fn catalog_entry_stores_unified_fields() {
    let entry = test_entry("unified", SkillType::Inline)
        .with_triggers(vec!["trigger".to_string()])
        .with_hidden_in_modes(vec![ModeKind::Plan])
        .with_disable_model_invocation(true);

    assert!(matches!(entry.skill_type, SkillType::Inline));
    assert_eq!(entry.triggers, vec!["trigger"]);
    assert!(entry.hidden_in_modes.contains(&ModeKind::Plan));
    assert!(entry.disable_model_invocation);
}

#[test]
fn knowledge_skills_are_not_prompt_visible_by_default() {
    let knowledge = test_entry("knowledge", SkillType::Knowledge);
    assert!(!knowledge.prompt_visible);

    let flow = test_entry("flow", SkillType::Flow);
    assert!(!flow.prompt_visible);

    let inline = test_entry("inline", SkillType::Inline);
    assert!(inline.prompt_visible);

    let prompt = test_entry("prompt", SkillType::Prompt);
    assert!(prompt.prompt_visible);
}

#[test]
fn flow_skills_are_not_prompt_visible_by_default() {
    let entry = SkillCatalogEntry::new(
        SkillPackageId("host/flow".to_string()),
        host_authority(),
        "flow",
        "Flow skill.",
        SkillResourceId::new("host/flow/SKILL.md"),
    )
    .with_skill_type(SkillType::Flow);
    assert!(!entry.prompt_visible);
}

#[test]
fn catalog_filters_by_mode_and_type() {
    let inline = test_entry("inline", SkillType::Inline);
    let knowledge = test_entry("knowledge", SkillType::Knowledge);
    let hidden_in_plan = test_entry("hidden-in-plan", SkillType::Inline)
        .with_hidden_in_modes(vec![ModeKind::Plan]);
    let disabled = test_entry("disabled", SkillType::Inline).disabled();

    let mut catalog = SkillCatalog {
        entries: vec![inline.clone(), knowledge, hidden_in_plan.clone(), disabled],
        warnings: Vec::new(),
    };

    catalog.filter_for_mode(Some(ModeKind::Plan));

    assert_eq!(catalog.entries.len(), 1);
    assert_eq!(catalog.entries[0].id.0, "pkg/inline");

    assert!(inline.is_visible_in_mode(ModeKind::Default));
    assert!(!hidden_in_plan.is_visible_in_mode(ModeKind::Plan));
    assert!(hidden_in_plan.is_visible_in_mode(ModeKind::Default));

    let invocable = test_entry("invocable", SkillType::Inline);
    let not_invocable = test_entry("not-invocable", SkillType::Inline)
        .with_disable_model_invocation(true);
    assert!(invocable.is_model_invocable(ModeKind::Default));
    assert!(!not_invocable.is_model_invocable(ModeKind::Default));

    let knowledge = test_entry("knowledge", SkillType::Knowledge);
    let flow = test_entry("flow", SkillType::Flow);
    assert!(!knowledge.is_model_invocable(ModeKind::Default));
    assert!(!flow.is_model_invocable(ModeKind::Default));
}
