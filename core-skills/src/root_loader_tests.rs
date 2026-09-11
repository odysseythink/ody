use super::SkillDependencyIssueKind;
use super::validate_skill_dependencies;
use crate::model::SkillDependencies;
use crate::model::SkillDependency;
use crate::model::SkillMetadata;

fn skill_with_deps(name: &str, deps: &[&str]) -> SkillMetadata {
    SkillMetadata {
        name: name.to_string(),
        dependencies: if deps.is_empty() {
            None
        } else {
            Some(SkillDependencies {
                tools: vec![],
                skills: deps
                    .iter()
                    .map(|dep| SkillDependency {
                        name: dep.to_string(),
                    })
                    .collect(),
            })
        },
        ..Default::default()
    }
}

#[test]
fn no_declared_dependencies_produces_no_issues() {
    let skills = vec![skill_with_deps("a", &[]), skill_with_deps("b", &[])];
    assert_eq!(validate_skill_dependencies(&skills), vec![]);
}

#[test]
fn fully_qualified_dependency_resolves_without_issue() {
    let skills = vec![
        skill_with_deps("consumer", &["ns:provider"]),
        skill_with_deps("ns:provider", &[]),
    ];
    assert_eq!(validate_skill_dependencies(&skills), vec![]);
}

#[test]
fn unqualified_name_resolves_when_unique() {
    let skills = vec![
        skill_with_deps("consumer", &["provider"]),
        skill_with_deps("ns:provider", &[]),
    ];
    assert_eq!(validate_skill_dependencies(&skills), vec![]);
}

#[test]
fn missing_dependency_is_reported() {
    let skills = vec![skill_with_deps("consumer", &["ghost"])];
    assert_eq!(
        validate_skill_dependencies(&skills),
        vec![super::SkillDependencyIssue {
            kind: SkillDependencyIssueKind::Missing,
            skill: "consumer".to_string(),
            dependency: "ghost".to_string(),
        }]
    );
}

#[test]
fn ambiguous_unqualified_name_is_reported() {
    let skills = vec![
        skill_with_deps("consumer", &["provider"]),
        skill_with_deps("a:provider", &[]),
        skill_with_deps("b:provider", &[]),
    ];
    assert_eq!(
        validate_skill_dependencies(&skills),
        vec![super::SkillDependencyIssue {
            kind: SkillDependencyIssueKind::Ambiguous,
            skill: "consumer".to_string(),
            dependency: "provider".to_string(),
        }]
    );
}

#[test]
fn two_node_cycle_is_reported_with_cycle_path() {
    let skills = vec![skill_with_deps("a", &["b"]), skill_with_deps("b", &["a"])];
    let issues = validate_skill_dependencies(&skills);
    assert_eq!(issues.len(), 1, "expected one cycle issue: {issues:?}");
    assert_eq!(issues[0].kind, SkillDependencyIssueKind::Cycle);
    assert_eq!(issues[0].skill, "a");
    assert!(issues[0].dependency.contains("a"));
    assert!(issues[0].dependency.contains("b"));
}

#[test]
fn self_dependency_is_reported_as_cycle() {
    let skills = vec![skill_with_deps("a", &["a"])];
    let issues = validate_skill_dependencies(&skills);
    assert_eq!(issues.len(), 1, "expected one cycle issue: {issues:?}");
    assert_eq!(issues[0].kind, SkillDependencyIssueKind::Cycle);
    assert_eq!(issues[0].skill, "a");
    assert_eq!(issues[0].dependency, "a -> a");
}

#[test]
fn diamond_dependency_shape_produces_no_issues() {
    let skills = vec![
        skill_with_deps("top", &["left", "right"]),
        skill_with_deps("left", &["base"]),
        skill_with_deps("right", &["base"]),
        skill_with_deps("base", &[]),
    ];
    assert_eq!(validate_skill_dependencies(&skills), vec![]);
}
