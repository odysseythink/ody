use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;
use std::sync::Mutex;

use ody_utils_plugins::PluginSkillRoot;

use crate::SkillLoadOutcome;
use crate::loader::SkillRoot;
use crate::loader::SkillRootSnapshot;
use crate::loader::load_skill_root;
use crate::model::SkillFileSystemsByPath;
use crate::model::SkillMetadata;

/// Parsed plugin skill-root snapshots produced by one plugin load.
///
/// Clones share the same snapshots. The plugins manager stores them with the corresponding loaded
/// plugins and passes a clone to skill loading as an optional preload.
#[derive(Clone)]
pub struct PluginSkillSnapshots {
    snapshots_by_root: Arc<Mutex<HashMap<PluginSkillRoot, SkillRootSnapshot>>>,
}

impl PluginSkillSnapshots {
    /// Creates an empty snapshot collection for a plugin load to populate.
    pub fn for_plugin_load() -> Self {
        Self {
            snapshots_by_root: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl fmt::Debug for PluginSkillSnapshots {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PluginSkillSnapshots")
            .finish_non_exhaustive()
    }
}

pub(crate) async fn load_and_merge_skill_roots<I>(
    roots: I,
    plugin_skill_snapshots: Option<&PluginSkillSnapshots>,
) -> SkillLoadOutcome
where
    I: IntoIterator<Item = SkillRoot>,
{
    let mut root_snapshots = Vec::new();
    for root in roots {
        let cache_key = match (
            root.plugin_id.clone(),
            root.plugin_namespace.clone(),
            root.plugin_root.clone(),
        ) {
            (Some(plugin_id), Some(plugin_namespace), Some(plugin_root)) => Some(PluginSkillRoot {
                path: root.path.clone(),
                plugin_id,
                plugin_namespace,
                plugin_root,
            }),
            _ => None,
        };
        let cached_snapshot = cache_key.as_ref().and_then(|cache_key| {
            let plugin_skill_snapshots = plugin_skill_snapshots?;
            plugin_skill_snapshots
                .snapshots_by_root
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(cache_key)
                .cloned()
        });
        let snapshot = match cached_snapshot {
            Some(snapshot) => snapshot,
            None => {
                let snapshot = load_skill_root(root).await;
                if let Some(plugin_skill_snapshots) = plugin_skill_snapshots
                    && let Some(cache_key) = cache_key
                {
                    plugin_skill_snapshots
                        .snapshots_by_root
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .insert(cache_key, snapshot.clone());
                }
                snapshot
            }
        };
        root_snapshots.push(snapshot);
    }

    merge_skill_root_snapshots(root_snapshots)
}

fn merge_skill_root_snapshots(snapshots: Vec<SkillRootSnapshot>) -> SkillLoadOutcome {
    fn scope_rank(scope: ody_protocol::protocol::SkillScope) -> u8 {
        use ody_protocol::protocol::SkillScope;

        // Higher-priority scopes first (matches root scan order for dedupe).
        match scope {
            SkillScope::Repo => 0,
            SkillScope::User => 1,
            SkillScope::System => 2,
            SkillScope::Admin => 3,
        }
    }

    let mut outcome = SkillLoadOutcome::default();
    let mut skill_roots = Vec::new();
    let mut skill_root_by_path = HashMap::new();
    let mut file_systems_by_skill_path = HashMap::new();

    for snapshot in snapshots {
        let SkillRootSnapshot {
            root,
            skills,
            errors,
            file_system,
        } = snapshot;
        if !skills.is_empty() && !skill_roots.contains(&root) {
            skill_roots.push(root.clone());
        }
        for skill in &skills {
            skill_root_by_path
                .entry(skill.path_to_skills_md.clone())
                .or_insert_with(|| root.clone());
            file_systems_by_skill_path
                .entry(skill.path_to_skills_md.clone())
                .or_insert_with(|| Arc::clone(&file_system));
        }
        outcome.skills.extend(skills);
        outcome.errors.extend(errors);
    }

    let mut seen = HashSet::new();
    outcome
        .skills
        .retain(|skill| seen.insert(skill.path_to_skills_md.clone()));
    let retained_skill_paths = outcome
        .skills
        .iter()
        .map(|skill| skill.path_to_skills_md.clone())
        .collect::<HashSet<_>>();
    skill_root_by_path.retain(|path, _| retained_skill_paths.contains(path));
    let used_roots = skill_root_by_path.values().cloned().collect::<HashSet<_>>();
    skill_roots.retain(|root| used_roots.contains(root));
    file_systems_by_skill_path.retain(|path, _| retained_skill_paths.contains(path));
    outcome.skill_roots = skill_roots;
    outcome.skill_root_by_path = Arc::new(skill_root_by_path);
    outcome.file_systems_by_skill_path = SkillFileSystemsByPath::new(file_systems_by_skill_path);

    outcome.skills.sort_by(|a, b| {
        scope_rank(a.scope)
            .cmp(&scope_rank(b.scope))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.path_to_skills_md.cmp(&b.path_to_skills_md))
    });

    for issue in validate_skill_dependencies(&outcome.skills) {
        match issue.kind {
            SkillDependencyIssueKind::Missing => tracing::warn!(
                "skill '{}' declares missing dependency '{}'",
                issue.skill,
                issue.dependency
            ),
            SkillDependencyIssueKind::Ambiguous => tracing::warn!(
                "skill '{}' declares ambiguous dependency '{}'",
                issue.skill,
                issue.dependency
            ),
            SkillDependencyIssueKind::Cycle => tracing::warn!(
                "cyclic skill dependency involving '{}': {}",
                issue.skill,
                issue.dependency
            ),
        }
    }

    outcome
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SkillDependencyIssueKind {
    Missing,
    Ambiguous,
    Cycle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SkillDependencyIssue {
    pub kind: SkillDependencyIssueKind,
    /// Qualified name of the skill whose declaration has the issue.
    pub skill: String,
    /// Declared dependency name, or the cycle path ("a -> b -> a") for cycles.
    pub dependency: String,
}

/// Resolves declared skill dependencies against the loaded catalog and reports
/// missing, ambiguous, and cyclic declarations. Warning-only: nothing here
/// rejects or reorders skills.
pub(crate) fn validate_skill_dependencies(skills: &[SkillMetadata]) -> Vec<SkillDependencyIssue> {
    let mut full_name_index = HashMap::new();
    let mut base_name_indexes: HashMap<&str, Vec<usize>> = HashMap::new();
    for (index, skill) in skills.iter().enumerate() {
        full_name_index.entry(skill.name.as_str()).or_insert(index);
        base_name_indexes
            .entry(skill.name.rsplit(':').next().unwrap_or(skill.name.as_str()))
            .or_default()
            .push(index);
    }

    let mut issues = Vec::new();
    let mut edges: Vec<Vec<usize>> = vec![Vec::new(); skills.len()];
    for (index, skill) in skills.iter().enumerate() {
        let Some(dependencies) = skill.dependencies.as_ref() else {
            continue;
        };
        for dependency in &dependencies.skills {
            let resolved = if let Some(index) = full_name_index.get(dependency.name.as_str()) {
                Ok(*index)
            } else {
                match base_name_indexes.get(dependency.name.as_str()) {
                    Some(indexes) if indexes.len() == 1 => Ok(indexes[0]),
                    Some(_) => Err(SkillDependencyIssueKind::Ambiguous),
                    None => Err(SkillDependencyIssueKind::Missing),
                }
            };
            match resolved {
                Ok(target) => edges[index].push(target),
                Err(kind) => issues.push(SkillDependencyIssue {
                    kind,
                    skill: skill.name.clone(),
                    dependency: dependency.name.clone(),
                }),
            }
        }
    }

    let mut color = vec![CycleColor::White; skills.len()];
    let mut path = Vec::new();
    let mut reported = HashSet::new();
    for index in 0..skills.len() {
        if color[index] == CycleColor::White {
            find_dependency_cycles(
                index,
                &edges,
                skills,
                &mut color,
                &mut path,
                &mut reported,
                &mut issues,
            );
        }
    }

    issues
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CycleColor {
    White,
    Gray,
    Black,
}

#[allow(clippy::too_many_arguments)]
fn find_dependency_cycles(
    node: usize,
    edges: &[Vec<usize>],
    skills: &[SkillMetadata],
    color: &mut [CycleColor],
    path: &mut Vec<usize>,
    reported: &mut HashSet<(usize, usize)>,
    issues: &mut Vec<SkillDependencyIssue>,
) {
    color[node] = CycleColor::Gray;
    path.push(node);
    for &next in &edges[node] {
        if reported.contains(&(node, next)) {
            continue;
        }
        match color[next] {
            CycleColor::White => {
                find_dependency_cycles(next, edges, skills, color, path, reported, issues)
            }
            CycleColor::Gray => {
                reported.insert((node, next));
                let cycle_start = path
                    .iter()
                    .position(|&member| member == next)
                    .unwrap_or(node);
                let cycle_path = path[cycle_start..]
                    .iter()
                    .map(|&member| skills[member].name.as_str())
                    .chain(std::iter::once(skills[next].name.as_str()))
                    .collect::<Vec<_>>()
                    .join(" -> ");
                issues.push(SkillDependencyIssue {
                    kind: SkillDependencyIssueKind::Cycle,
                    skill: skills[cycle_start].name.clone(),
                    dependency: cycle_path,
                });
            }
            CycleColor::Black => {}
        }
    }
    path.pop();
    color[node] = CycleColor::Black;
}

#[cfg(test)]
#[path = "root_loader_tests.rs"]
mod tests;
