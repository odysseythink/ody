use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::VecDeque;

use ody_core_skills::SkillType;
use ody_core_skills::injection::extract_tool_mentions;
use ody_protocol::user_input::UserInput;

use crate::catalog::SkillAuthority;
use crate::catalog::SkillCatalog;
use crate::catalog::SkillCatalogEntry;
use crate::catalog::SkillPackageId;

const SKILL_PATH_PREFIX: &str = "skill://";

#[tracing::instrument(
    level = "trace",
    skip_all,
    fields(
        input_count = inputs.len(),
        catalog_entry_count = catalog.entries.len()
    )
)]
pub(crate) fn collect_explicit_skill_mentions(
    inputs: &[UserInput],
    catalog: &SkillCatalog,
) -> Vec<SelectedSkill> {
    let mut selected = Vec::new();
    let mut seen = HashSet::new();
    let mut blocked_plain_names = HashSet::new();

    let selectable_entries: Vec<&SkillCatalogEntry> = catalog
        .entries
        .iter()
        .filter(|entry| entry.skill_type != SkillType::Knowledge)
        .collect();

    for input in inputs {
        match input {
            UserInput::Skill { name, path } => {
                blocked_plain_names.insert(name.clone());
                select_by_path(
                    &selectable_entries,
                    &path.to_string_lossy(),
                    &mut seen,
                    &mut selected,
                );
            }
            UserInput::Mention { name, path } if path_is_skill(path) => {
                blocked_plain_names.insert(name.clone());
                select_by_path(&selectable_entries, path, &mut seen, &mut selected);
            }
            UserInput::Text { .. } | UserInput::Image { .. } | UserInput::LocalImage { .. } => {}
            UserInput::Mention { .. } => {}
            _ => {}
        }
    }

    for input in inputs {
        let UserInput::Text { text, .. } = input else {
            continue;
        };

        let mentions = extract_tool_mentions(text);
        for path in mentions.paths() {
            if path_is_skill(path) {
                select_by_path(
                    &selectable_entries,
                    normalize_skill_path(path),
                    &mut seen,
                    &mut selected,
                );
            }
        }
        for name in mentions.plain_names() {
            if blocked_plain_names.contains(name) {
                continue;
            }
            if let Some(entry) = selectable_entries
                .iter()
                .find(|entry| entry.enabled && entry.name == name)
            {
                push_selected(entry, &mut seen, &mut selected);
            }
        }
    }

    expand_selected_with_dependencies(catalog, &mut selected);

    selected
}

// ody: 名字解析规则与 core-skills/src/root_loader.rs 的加载期校验手工保持一致,
// 升级触发条件: 引入 optional 依赖/版本语义, 或出现第三处需要解析依赖名时, 抽共享模块
/// Expands explicitly selected skills with their declared skill dependencies
/// and orders the result so dependencies come before dependents.
///
/// Dependency names resolve against enabled, non-Knowledge catalog entries by
/// qualified name, then by unique unqualified base name. Unresolvable names
/// are skipped here; the loader already warns about them at load time.
/// Cyclic declarations keep their original relative order instead of hanging.
fn expand_selected_with_dependencies(
    catalog: &SkillCatalog,
    selected: &mut Vec<SelectedSkill>,
) {
    if selected.is_empty() {
        return;
    }

    let candidates: Vec<&SkillCatalogEntry> = catalog
        .entries
        .iter()
        .filter(|entry| entry.enabled && entry.skill_type != SkillType::Knowledge)
        .collect();
    let mut full_name_index = HashMap::new();
    let mut base_name_indexes: HashMap<&str, Vec<usize>> = HashMap::new();
    for (index, entry) in candidates.iter().enumerate() {
        full_name_index.entry(entry.name.as_str()).or_insert(index);
        base_name_indexes
            .entry(entry.name.rsplit(':').next().unwrap_or(entry.name.as_str()))
            .or_default()
            .push(index);
    }
    let resolve = |name: &str| -> Option<usize> {
        if let Some(index) = full_name_index.get(name) {
            return Some(*index);
        }
        match base_name_indexes.get(name) {
            Some(indexes) if indexes.len() == 1 => Some(indexes[0]),
            _ => None,
        }
    };
    let dep_targets: Vec<Vec<usize>> = candidates
        .iter()
        .map(|entry| {
            entry
                .dependencies
                .as_ref()
                .map(|dependencies| {
                    dependencies
                        .skills
                        .iter()
                        .filter_map(|dependency| resolve(&dependency.name))
                        .collect()
                })
                .unwrap_or_default()
        })
        .collect();

    let key_to_candidate: HashMap<SkillCatalogEntryKey, usize> = candidates
        .iter()
        .enumerate()
        .map(|(index, entry)| (SkillCatalogEntryKey::from(*entry), index))
        .collect();

    // Breadth-first closure over resolved dependency edges, starting from the
    // explicitly selected skills. Explicitly selected targets are already in
    // `ordered`; `visited` keeps cyclic declarations from looping forever.
    let mut ordered: Vec<SelectedSkill> = selected.clone();
    let mut visited: HashSet<usize> = selected
        .iter()
        .filter_map(|selected| {
            key_to_candidate
                .get(&SkillCatalogEntryKey::from(&selected.entry))
                .copied()
        })
        .collect();
    let mut queue: VecDeque<usize> = visited.iter().copied().collect();
    while let Some(index) = queue.pop_front() {
        for &target in &dep_targets[index] {
            if visited.insert(target) {
                queue.push_back(target);
                ordered.push(SelectedSkill {
                    entry: candidates[target].clone(),
                    dependency_of: Some(candidates[index].name.clone()),
                });
            }
        }
    }

    // Stable topological sort: repeatedly emit the first entry whose in-set
    // dependencies have all been emitted. On a cycle, append the remaining
    // entries in their original order.
    let mut emitted_keys: HashSet<SkillCatalogEntryKey> = HashSet::new();
    let mut emitted: Vec<SelectedSkill> = Vec::with_capacity(ordered.len());
    while emitted.len() < ordered.len() {
        let mut progress = false;
        for selected_entry in &ordered {
            let key = SkillCatalogEntryKey::from(&selected_entry.entry);
            if emitted_keys.contains(&key) {
                continue;
            }
            let deps_satisfied = key_to_candidate
                .get(&key)
                .map(|&index| {
                    dep_targets[index].iter().all(|&target| {
                        emitted_keys.contains(&SkillCatalogEntryKey::from(candidates[target]))
                    })
                })
                .unwrap_or(true);
            if deps_satisfied {
                emitted_keys.insert(key);
                emitted.push(selected_entry.clone());
                progress = true;
            }
        }
        if !progress {
            for selected_entry in &ordered {
                let key = SkillCatalogEntryKey::from(&selected_entry.entry);
                if emitted_keys.insert(key) {
                    emitted.push(selected_entry.clone());
                }
            }
        }
    }

    *selected = emitted;
}

fn select_by_path(
    entries: &[&SkillCatalogEntry],
    path: &str,
    seen: &mut HashSet<SkillCatalogEntryKey>,
    selected: &mut Vec<SelectedSkill>,
) {
    let normalized_path = normalize_skill_path(path);
    for entry in entries.iter().filter(|entry| entry.enabled) {
        if entry_matches_path(entry, normalized_path) {
            push_selected(entry, seen, selected);
        }
    }
}

fn push_selected(
    entry: &SkillCatalogEntry,
    seen: &mut HashSet<SkillCatalogEntryKey>,
    selected: &mut Vec<SelectedSkill>,
) {
    let key = SkillCatalogEntryKey::from(entry);
    if seen.insert(key) {
        selected.push(SelectedSkill {
            entry: entry.clone(),
            dependency_of: None,
        });
    }
}

/// A skill selected for injection, either explicitly by the user/model or
/// automatically because another selected skill declared it as a dependency.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SelectedSkill {
    pub entry: SkillCatalogEntry,
    /// Name of the skill that pulled this entry in via `dependencies.skills`.
    /// `None` when the skill was explicitly activated.
    pub dependency_of: Option<String>,
}

fn entry_matches_path(entry: &SkillCatalogEntry, path: &str) -> bool {
    entry.main_prompt.as_str() == path
        || entry.id.0 == path
        || entry
            .display_path
            .as_deref()
            .is_some_and(|display_path| normalize_skill_path(display_path) == path)
}

fn path_is_skill(path: &str) -> bool {
    path.starts_with(SKILL_PATH_PREFIX)
        || path
            .rsplit(['/', '\\'])
            .next()
            .is_some_and(|file_name| file_name.eq_ignore_ascii_case("SKILL.md"))
}

fn normalize_skill_path(path: &str) -> &str {
    path.strip_prefix(SKILL_PATH_PREFIX).unwrap_or(path)
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct SkillCatalogEntryKey {
    authority: SkillAuthority,
    package: SkillPackageId,
}

impl From<&SkillCatalogEntry> for SkillCatalogEntryKey {
    fn from(entry: &SkillCatalogEntry) -> Self {
        Self {
            authority: entry.authority.clone(),
            package: entry.id.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use ody_core_skills::SkillType;
    use ody_core_skills::model::SkillDependencies;
    use ody_core_skills::model::SkillDependency;
    use ody_protocol::user_input::UserInput;

    use super::collect_explicit_skill_mentions;
    use crate::catalog::SkillAuthority;
    use crate::catalog::SkillCatalog;
    use crate::catalog::SkillCatalogEntry;
    use crate::catalog::SkillPackageId;
    use crate::catalog::SkillResourceId;
    use crate::catalog::SkillSourceKind;

    fn entry(name: &str, deps: &[&str]) -> SkillCatalogEntry {
        let entry = SkillCatalogEntry::new(
            SkillPackageId(name.to_string()),
            SkillAuthority::new(SkillSourceKind::Host, "host"),
            name,
            format!("{name} description"),
            SkillResourceId::new(format!("{name}/SKILL.md")),
        );
        if deps.is_empty() {
            entry
        } else {
            entry.with_dependencies(Some(SkillDependencies {
                tools: vec![],
                skills: deps
                    .iter()
                    .map(|dep| SkillDependency {
                        name: dep.to_string(),
                    })
                    .collect(),
            }))
        }
    }

    fn catalog(entries: Vec<SkillCatalogEntry>) -> SkillCatalog {
        SkillCatalog {
            entries,
            warnings: Vec::new(),
        }
    }

    fn select(catalog: &SkillCatalog, text: &str) -> Vec<(String, Option<String>)> {
        let inputs = vec![UserInput::Text {
            text: text.to_string(),
            text_elements: Vec::new(),
        }];
        collect_explicit_skill_mentions(&inputs, catalog)
            .into_iter()
            .map(|selected| (selected.entry.name, selected.dependency_of))
            .collect()
    }

    fn names(selected: &[(String, Option<String>)]) -> Vec<&str> {
        selected.iter().map(|(name, _)| name.as_str()).collect()
    }

    #[test]
    fn explicit_selection_pulls_in_transitive_dependency_closure_first() {
        let catalog = catalog(vec![
            entry("consumer", &["provider"]),
            entry("provider", &["base"]),
            entry("base", &[]),
            entry("unrelated", &[]),
        ]);
        let selected = select(&catalog, "run $consumer");
        assert_eq!(names(&selected), vec!["base", "provider", "consumer"]);
        assert_eq!(
            selected,
            vec![
                ("base".to_string(), Some("provider".to_string())),
                ("provider".to_string(), Some("consumer".to_string())),
                ("consumer".to_string(), None),
            ]
        );
    }

    #[test]
    fn dependency_resolves_by_unique_unqualified_name() {
        let catalog = catalog(vec![
            entry("consumer", &["provider"]),
            entry("ns:provider", &[]),
        ]);
        assert_eq!(
            names(&select(&catalog, "run $consumer")),
            vec!["ns:provider", "consumer"]
        );
    }

    #[test]
    fn missing_dependency_is_skipped_without_blocking_selection() {
        let catalog = catalog(vec![entry("consumer", &["ghost"])]);
        assert_eq!(
            names(&select(&catalog, "run $consumer")),
            vec!["consumer"]
        );
    }

    #[test]
    fn cyclic_dependencies_keep_stable_order_without_hanging() {
        let catalog = catalog(vec![entry("a", &["b"]), entry("b", &["a"])]);
        assert_eq!(names(&select(&catalog, "run $a")), vec!["a", "b"]);
    }

    #[test]
    fn knowledge_skill_is_not_pulled_in_as_dependency() {
        let knowledge = entry("facts", &[]).with_skill_type(SkillType::Knowledge);
        let catalog = catalog(vec![entry("consumer", &["facts"]), knowledge]);
        assert_eq!(
            names(&select(&catalog, "run $consumer")),
            vec!["consumer"]
        );
    }

    #[test]
    fn disabled_skill_is_not_pulled_in_as_dependency() {
        let catalog = catalog(vec![
            entry("consumer", &["provider"]),
            entry("provider", &[]).disabled(),
        ]);
        assert_eq!(
            names(&select(&catalog, "run $consumer")),
            vec!["consumer"]
        );
    }

    #[test]
    fn explicitly_selected_dependencies_appear_once_before_dependent() {
        let catalog = catalog(vec![
            entry("consumer", &["provider"]),
            entry("provider", &[]),
        ]);
        assert_eq!(
            names(&select(&catalog, "run $consumer and $provider")),
            vec!["provider", "consumer"]
        );
    }

    #[test]
    fn unrelated_selection_includes_all_mentions() {
        // Mention extraction collects names via a HashSet, so mention order in
        // the text is not guaranteed upstream; only set membership is.
        let catalog = catalog(vec![entry("a", &[]), entry("b", &[])]);
        let selected = select(&catalog, "run $b then $a");
        let mut selected = names(&selected);
        selected.sort_unstable();
        assert_eq!(selected, vec!["a", "b"]);
    }
}
