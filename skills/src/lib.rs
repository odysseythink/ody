pub mod remote_sync;

use include_dir::Dir;
use ody_utils_absolute_path::AbsolutePathBuf;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::Hash;
use std::hash::Hasher;

use thiserror::Error;

const SYSTEM_SKILLS_DIR: Dir = include_dir::include_dir!("$CARGO_MANIFEST_DIR/src/assets/embedded");

const SYSTEM_SKILLS_DIR_NAME: &str = ".system";
const SKILLS_DIR_NAME: &str = "skills";
const SYSTEM_SKILLS_MARKER_FILENAME: &str = ".ody-system-skills.marker";
const SYSTEM_SKILLS_MARKER_SALT: &str = "v1";

pub use remote_sync::remote_builtin_cache_root_dir;

/// Names of the skills bundled in the embedded system-skills set.
/// Remote builtin-skill sync skips these so a backend cannot shadow or
/// duplicate embedded content.
pub fn embedded_system_skill_names() -> Vec<String> {
    SYSTEM_SKILLS_DIR
        .dirs()
        .map(|dir| {
            dir.path()
                .file_name()
                .expect("embedded skill dir has a name")
                .to_string_lossy()
                .to_string()
        })
        .collect()
}

/// Returns the on-disk cache location for embedded system skills from an absolute ODY_HOME.
pub fn system_cache_root_dir(ody_home: &AbsolutePathBuf) -> AbsolutePathBuf {
    ody_home.join(SKILLS_DIR_NAME).join(SYSTEM_SKILLS_DIR_NAME)
}

/// Installs embedded system skills into `ODY_HOME/skills/.system`.
///
/// Embedded skills live under `src/assets/embedded` and include both system
/// helper skills (skill-creator, skill-installer, plugin-creator) and the
/// ody-code builtin skills (debt-ledger, dispatching-parallel-agents, etc.).
/// They are copied to disk on first run and then discovered as a system-scope
/// skill root, so they are available even when `~/.ody-code/skills` is empty
/// or missing.
///
/// `ody-core-skills` treats `.system` as a system-scope skill root, so skills
/// installed here are automatically discovered by the unified registry through
/// the host provider (`HostSkillProvider`) and appear in the host authority
/// catalog.
///
/// Clears any existing system skills directory first and then writes the embedded
/// skills directory into place.
///
/// To avoid doing unnecessary work on every startup, a marker file is written
/// with a fingerprint of the embedded directory. When the marker matches, the
/// install is skipped.
pub fn install_system_skills(ody_home: &AbsolutePathBuf) -> Result<(), SystemSkillsError> {
    let skills_root_dir = ody_home.join(SKILLS_DIR_NAME);
    fs::create_dir_all(skills_root_dir.as_path())
        .map_err(|source| SystemSkillsError::io("create skills root dir", source))?;

    let dest_system = system_cache_root_dir(ody_home);

    let marker_path = dest_system.join(SYSTEM_SKILLS_MARKER_FILENAME);
    let expected_fingerprint = embedded_system_skills_fingerprint();
    if dest_system.as_path().is_dir()
        && read_marker(&marker_path).is_ok_and(|marker| marker == expected_fingerprint)
    {
        return Ok(());
    }

    if dest_system.as_path().exists() {
        fs::remove_dir_all(dest_system.as_path())
            .map_err(|source| SystemSkillsError::io("remove existing system skills dir", source))?;
    }

    write_embedded_dir(&SYSTEM_SKILLS_DIR, &dest_system)?;
    fs::write(marker_path.as_path(), format!("{expected_fingerprint}\n"))
        .map_err(|source| SystemSkillsError::io("write system skills marker", source))?;
    Ok(())
}

fn read_marker(path: &AbsolutePathBuf) -> Result<String, SystemSkillsError> {
    Ok(fs::read_to_string(path.as_path())
        .map_err(|source| SystemSkillsError::io("read system skills marker", source))?
        .trim()
        .to_string())
}

fn embedded_system_skills_fingerprint() -> String {
    let mut items = Vec::new();
    collect_fingerprint_items(&SYSTEM_SKILLS_DIR, &mut items);
    items.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));

    let mut hasher = DefaultHasher::new();
    SYSTEM_SKILLS_MARKER_SALT.hash(&mut hasher);
    for (path, contents_hash) in items {
        path.hash(&mut hasher);
        contents_hash.hash(&mut hasher);
    }
    format!("{:x}", hasher.finish())
}

fn collect_fingerprint_items(dir: &Dir<'_>, items: &mut Vec<(String, Option<u64>)>) {
    for entry in dir.entries() {
        match entry {
            include_dir::DirEntry::Dir(subdir) => {
                items.push((subdir.path().to_string_lossy().to_string(), None));
                collect_fingerprint_items(subdir, items);
            }
            include_dir::DirEntry::File(file) => {
                let mut file_hasher = DefaultHasher::new();
                file.contents().hash(&mut file_hasher);
                items.push((
                    file.path().to_string_lossy().to_string(),
                    Some(file_hasher.finish()),
                ));
            }
        }
    }
}

/// Writes the embedded `include_dir::Dir` to disk under `dest`.
///
/// Preserves the embedded directory structure.
fn write_embedded_dir(dir: &Dir<'_>, dest: &AbsolutePathBuf) -> Result<(), SystemSkillsError> {
    fs::create_dir_all(dest.as_path())
        .map_err(|source| SystemSkillsError::io("create system skills dir", source))?;

    for entry in dir.entries() {
        match entry {
            include_dir::DirEntry::Dir(subdir) => {
                let subdir_dest = dest.join(subdir.path());
                fs::create_dir_all(subdir_dest.as_path()).map_err(|source| {
                    SystemSkillsError::io("create system skills subdir", source)
                })?;
                write_embedded_dir(subdir, dest)?;
            }
            include_dir::DirEntry::File(file) => {
                let path = dest.join(file.path());
                if let Some(parent) = path.as_path().parent() {
                    fs::create_dir_all(parent).map_err(|source| {
                        SystemSkillsError::io("create system skills file parent", source)
                    })?;
                }
                fs::write(path.as_path(), file.contents())
                    .map_err(|source| SystemSkillsError::io("write system skill file", source))?;
            }
        }
    }

    Ok(())
}

#[derive(Debug, Error)]
pub enum SystemSkillsError {
    #[error("io error while {action}: {source}")]
    Io {
        action: &'static str,
        #[source]
        source: std::io::Error,
    },
}

impl SystemSkillsError {
    fn io(action: &'static str, source: std::io::Error) -> Self {
        Self::Io { action, source }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use ody_exec_server::LocalFileSystem;
    use ody_protocol::protocol::SkillScope;

    use super::SYSTEM_SKILLS_DIR;
    use super::collect_fingerprint_items;
    use super::install_system_skills;
    use super::system_cache_root_dir;

    #[test]
    fn fingerprint_traverses_nested_entries() {
        let mut items = Vec::new();
        collect_fingerprint_items(&SYSTEM_SKILLS_DIR, &mut items);
        let mut paths: Vec<String> = items.into_iter().map(|(path, _)| path).collect();
        paths.sort_unstable();

        assert!(
            paths
                .binary_search_by(|probe| probe.as_str().cmp("skill-creator/SKILL.md"))
                .is_ok()
        );
        assert!(
            paths
                .binary_search_by(|probe| probe.as_str().cmp("skill-creator/scripts/init_skill.py"))
                .is_ok()
        );
    }

    #[tokio::test]
    async fn installed_system_skills_are_loadable_by_core_skills() {
        let temp_dir = tempfile::tempdir().unwrap();
        let ody_home = ody_utils_absolute_path::AbsolutePathBuf::try_from(temp_dir.path())
            .expect("absolute temp dir");
        install_system_skills(&ody_home).expect("install system skills");

        let system_root = system_cache_root_dir(&ody_home);
        let outcome = ody_core_skills::loader::load_skills_from_roots(
            [ody_core_skills::loader::SkillRoot {
                path: system_root,
                scope: SkillScope::System,
                file_system: Arc::new(LocalFileSystem::unsandboxed()),
                plugin_id: None,
                plugin_namespace: None,
                plugin_root: None,
            }],
            /*plugin_skill_snapshots*/ None,
        )
        .await;

        assert!(outcome.errors.is_empty(), "errors: {:?}", outcome.errors);
        assert!(
            !outcome.skills.is_empty(),
            "system skills should be discovered"
        );
    }

    #[tokio::test]
    async fn embedded_flow_sample_loads_as_flow_skill() {
        // M1.5: the bundled game-create sample is the reference `type: flow`
        // skill; the loader must classify it as Flow, validate flow.yaml
        // eagerly, and record the artifact path for the runtime re-read.
        let temp_dir = tempfile::tempdir().unwrap();
        let ody_home = ody_utils_absolute_path::AbsolutePathBuf::try_from(temp_dir.path())
            .expect("absolute temp dir");
        install_system_skills(&ody_home).expect("install system skills");

        let system_root = system_cache_root_dir(&ody_home);
        let outcome = ody_core_skills::loader::load_skills_from_roots(
            [ody_core_skills::loader::SkillRoot {
                path: system_root,
                scope: SkillScope::System,
                file_system: Arc::new(LocalFileSystem::unsandboxed()),
                plugin_id: None,
                plugin_namespace: None,
                plugin_root: None,
            }],
            /*plugin_skill_snapshots*/ None,
        )
        .await;

        assert!(outcome.errors.is_empty(), "errors: {:?}", outcome.errors);
        let skill = outcome
            .skills
            .iter()
            .find(|skill| skill.name == "game-create")
            .expect("game-create should be discovered");
        assert!(
            matches!(skill.skill_type, ody_core_skills::model::SkillType::Flow),
            "game-create should load as a flow skill, got {:?}",
            skill.skill_type
        );
        let artifact = skill
            .flow_artifact
            .as_ref()
            .expect("flow skill should record its flow.yaml");
        assert!(artifact.as_path().ends_with("flow.yaml"));
        let plan = ody_core_skills::flow::parse_flow_plan(
            &std::fs::read_to_string(artifact.as_path()).expect("flow.yaml should be readable"),
        )
        .expect("flow.yaml should parse");
        assert_eq!(plan.phases.len(), 3);
        assert_eq!(plan.phases[0].id, "design");
        assert_eq!(plan.phases[1].id, "implement");
        assert_eq!(plan.phases[2].id, "verify");
    }

    #[tokio::test]
    async fn embedded_system_skills_respect_product_restrictions() {
        // Product separation contract (odyBox decision 3B): ody-only builtin
        // skills must be invisible when the runtime restricts to Product::OdyBox
        // and vice versa. Untagged skills (empty products) stay visible under
        // every restriction, so each embedded skill must be tagged explicitly.
        let temp_dir = tempfile::tempdir().unwrap();
        let ody_home = ody_utils_absolute_path::AbsolutePathBuf::try_from(temp_dir.path())
            .expect("absolute temp dir");
        install_system_skills(&ody_home).expect("install system skills");

        let system_root = system_cache_root_dir(&ody_home);
        let load = |system_root: ody_utils_absolute_path::AbsolutePathBuf| {
            async move {
                ody_core_skills::loader::load_skills_from_roots(
                    [ody_core_skills::loader::SkillRoot {
                        path: system_root,
                        scope: SkillScope::System,
                        file_system: Arc::new(LocalFileSystem::unsandboxed()),
                        plugin_id: None,
                        plugin_namespace: None,
                        plugin_root: None,
                    }],
                    /*plugin_skill_snapshots*/ None,
                )
                .await
            }
        };

        let ody_only = [
            "debt-ledger",
            "game-create",
            "dispatching-parallel-agents",
            "executing-plans",
            "finishing-a-development-branch",
            "idea-evaluator",
            "idea-generator",
            "roadmap-architect",
            "subagent-driven-development",
            "systematic-debugging",
            "test-driven-development",
            "using-git-worktrees",
            "verification-before-completion",
            "plugin-creator",
            "simplicity-first",
            "skill-creator",
            "skill-installer",
        ];
        let odybox_only = ["vibedrop"];
        let shared = ["data-analysis", "frontend-design", "legal-contract"];

        let outcome = load(system_root).await;
        assert!(outcome.errors.is_empty(), "errors: {:?}", outcome.errors);

        let ody_filtered = ody_core_skills::filter_skill_load_outcome_for_product(
            outcome.clone(),
            Some(ody_protocol::protocol::Product::Ody),
        );
        let odybox_filtered = ody_core_skills::filter_skill_load_outcome_for_product(
            outcome,
            Some(ody_protocol::protocol::Product::OdyBox),
        );

        for name in ody_only {
            assert!(
                ody_filtered.skills.iter().any(|skill| skill.name == name),
                "ody-only {name} must be visible under Product::Ody"
            );
            assert!(
                !odybox_filtered.skills.iter().any(|skill| skill.name == name),
                "ody-only {name} must be hidden under Product::OdyBox"
            );
        }
        for name in odybox_only {
            assert!(
                odybox_filtered.skills.iter().any(|skill| skill.name == name),
                "odybox-only {name} must be visible under Product::OdyBox"
            );
            assert!(
                !ody_filtered.skills.iter().any(|skill| skill.name == name),
                "odybox-only {name} must be hidden under Product::Ody"
            );
        }
        for name in shared {
            assert!(
                ody_filtered.skills.iter().any(|skill| skill.name == name),
                "shared {name} must be visible under Product::Ody"
            );
            assert!(
                odybox_filtered.skills.iter().any(|skill| skill.name == name),
                "shared {name} must be visible under Product::OdyBox"
            );
        }
    }

    #[tokio::test]
    async fn idea_skills_are_hidden_in_product_mode() {
        // Product mode's P0/P1 workflow already covers direction evaluation
        // (Path A) and idea screening; letting the funnel skills fire inside
        // Product mode would double-gate the user and break the one-question
        // rhythm. Their frontmatter must therefore name `product` in
        // hiddenInModes and the loader must actually honor it.
        let temp_dir = tempfile::tempdir().unwrap();
        let ody_home = ody_utils_absolute_path::AbsolutePathBuf::try_from(temp_dir.path())
            .expect("absolute temp dir");
        install_system_skills(&ody_home).expect("install system skills");

        let system_root = system_cache_root_dir(&ody_home);
        let outcome = ody_core_skills::loader::load_skills_from_roots(
            [ody_core_skills::loader::SkillRoot {
                path: system_root,
                scope: SkillScope::System,
                file_system: Arc::new(LocalFileSystem::unsandboxed()),
                plugin_id: None,
                plugin_namespace: None,
                plugin_root: None,
            }],
            /*plugin_skill_snapshots*/ None,
        )
        .await;

        assert!(outcome.errors.is_empty(), "errors: {:?}", outcome.errors);
        for name in ["idea-evaluator", "idea-generator"] {
            let skill = outcome
                .skills
                .iter()
                .find(|s| s.name == name)
                .unwrap_or_else(|| panic!("{name} should be installed and loadable"));
            assert!(
                skill.hidden_in_modes.contains(&ody_protocol::config_types::ModeKind::Product),
                "{name} must be hidden in Product mode; got {:?}",
                skill.hidden_in_modes
            );
            assert!(
                skill.is_model_invocable(ody_protocol::config_types::ModeKind::Default),
                "{name} must stay invocable in Default mode"
            );
        }
    }
}
