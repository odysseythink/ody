use super::*;
use crate::SkillMetadata;
use crate::config_rules::resolve_disabled_skill_paths;
use crate::config_rules::skill_config_rules_from_stack;
use ody_app_server_protocol::ConfigLayerSource;
use ody_config::CONFIG_TOML_FILE;
use ody_config::ConfigLayerEntry;
use ody_config::ConfigLayerStack;
use ody_config::ConfigRequirementsToml;
use ody_exec_server::LOCAL_FS;
use ody_utils_absolute_path::AbsolutePathBuf;
use ody_utils_absolute_path::test_support::PathBufExt;
use ody_utils_absolute_path::test_support::PathExt;
use ody_utils_plugins::PluginSkillRoot;
use pretty_assertions::assert_eq;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

fn write_user_skill(ody_home: &TempDir, dir: &str, name: &str, description: &str) {
    let skill_dir = ody_home.path().join("skills").join(dir);
    fs::create_dir_all(&skill_dir).unwrap();
    let content = format!("---\nname: {name}\ndescription: {description}\n---\n\n# Body\n");
    fs::write(skill_dir.join("SKILL.md"), content).unwrap();
}

fn write_plugin_skill(
    ody_home: &TempDir,
    marketplace: &str,
    plugin_name: &str,
    dir: &str,
    name: &str,
    description: &str,
) -> PathBuf {
    let plugin_root = ody_home
        .path()
        .join("plugins/cache")
        .join(marketplace)
        .join(plugin_name)
        .join("local");
    let skill_dir = plugin_root.join("skills").join(dir);
    fs::create_dir_all(plugin_root.join(".ody-plugin")).unwrap();
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        plugin_root.join(".ody-plugin/plugin.json"),
        format!(r#"{{"name":"{plugin_name}"}}"#),
    )
    .unwrap();
    let content = format!("---\nname: {name}\ndescription: {description}\n---\n\n# Body\n");
    let skill_path = skill_dir.join("SKILL.md");
    fs::write(&skill_path, content).unwrap();
    skill_path
}

fn plugin_skill_root_for_skill_path(
    skill_path: &Path,
    plugin_id: &str,
    plugin_namespace: &str,
) -> PluginSkillRoot {
    let skills_root = skill_path
        .parent()
        .and_then(Path::parent)
        .expect("plugin skill should live under a skills root");
    let plugin_root = skills_root
        .parent()
        .expect("plugin skills root should live under a plugin root");
    PluginSkillRoot {
        path: skills_root.abs(),
        plugin_id: plugin_id.to_string(),
        plugin_namespace: plugin_namespace.to_string(),
        plugin_root: plugin_root.abs(),
    }
}

fn test_skill(name: &str, path: PathBuf) -> SkillMetadata {
    SkillMetadata {
        name: name.to_string(),
        description: "test".to_string(),
        short_description: None,
        interface: None,
        dependencies: None,
        policy: None,
        path_to_skills_md: path
            .abs()
            .canonicalize()
            .expect("skill path should canonicalize"),
        scope: SkillScope::User,
        plugin_id: None,
        ..Default::default()
    }
}

fn write_demo_skill(tempdir: &TempDir) -> PathBuf {
    let skill_path = tempdir.path().join("skills").join("demo").join("SKILL.md");
    fs::create_dir_all(skill_path.parent().expect("skill path should have parent"))
        .expect("create skill dir");
    fs::write(
        &skill_path,
        "---\nname: demo-skill\ndescription: demo description\n---\n\n# Body\n",
    )
    .expect("write skill");
    skill_path
}

fn user_config_layer(ody_home: &TempDir, config_toml: &str) -> ConfigLayerEntry {
    let config_path = AbsolutePathBuf::try_from(ody_home.path().join(CONFIG_TOML_FILE))
        .expect("user config path should be absolute");
    ConfigLayerEntry::new(
        ConfigLayerSource::User {
            file: config_path,
            profile: None,
        },
        toml::from_str(config_toml).expect("user layer toml"),
    )
}

fn config_stack(ody_home: &TempDir, user_config_toml: &str) -> ConfigLayerStack {
    ConfigLayerStack::new(
        vec![user_config_layer(ody_home, user_config_toml)],
        Default::default(),
        ConfigRequirementsToml::default(),
    )
    .expect("valid config layer stack")
}

fn config_stack_with_session_flags(
    ody_home: &TempDir,
    user_config_toml: &str,
    session_flags_toml: &str,
) -> ConfigLayerStack {
    ConfigLayerStack::new(
        vec![
            user_config_layer(ody_home, user_config_toml),
            ConfigLayerEntry::new(
                ConfigLayerSource::SessionFlags,
                toml::from_str(session_flags_toml).expect("session layer toml"),
            ),
        ],
        Default::default(),
        ConfigRequirementsToml::default(),
    )
    .expect("valid config layer stack")
}

fn path_toggle_config(path: &std::path::Path, enabled: bool) -> String {
    format!(
        r#"[[skills.config]]
path = "{}"
enabled = {enabled}
"#,
        path.display()
    )
}

fn name_toggle_config(name: &str, enabled: bool) -> String {
    format!(
        r#"[[skills.config]]
name = "{name}"
enabled = {enabled}
"#
    )
}

async fn skills_for_config_with_stack(
    skills_service: &SkillsService,
    cwd: &TempDir,
    config_layer_stack: &ConfigLayerStack,
    effective_skill_roots: &[PluginSkillRoot],
) -> SkillLoadOutcome {
    let skills_input = SkillsLoadInput::new(
        cwd.path().abs(),
        effective_skill_roots.to_vec(),
        config_layer_stack.clone(),
        bundled_skills_enabled_from_stack(config_layer_stack),
    );
    skills_service
        .snapshot_for_config(&skills_input, Some(Arc::clone(&LOCAL_FS)))
        .await
        .outcome()
        .clone()
}

#[test]
fn new_with_disabled_bundled_skills_removes_stale_cached_system_skills() {
    let ody_home = tempfile::tempdir().expect("tempdir");
    let stale_system_skill_dir = ody_home.path().join("skills/.system/stale-skill");
    fs::create_dir_all(&stale_system_skill_dir).expect("create stale system skill dir");
    fs::write(stale_system_skill_dir.join("SKILL.md"), "# stale\n")
        .expect("write stale system skill");

    let _skills_service =
        SkillsService::new(ody_home.path().abs(), /*bundled_skills_enabled*/ false);

    assert!(
        !ody_home.path().join("skills/.system").exists(),
        "expected disabling system skills to remove stale cached bundled skills"
    );
}

#[tokio::test]
async fn skills_for_config_reuses_cache_for_same_effective_config() {
    let ody_home = tempfile::tempdir().expect("tempdir");
    let cwd = tempfile::tempdir().expect("tempdir");
    let config_layer_stack = config_stack(&ody_home, "");
    let skills_service =
        SkillsService::new(ody_home.path().abs(), /*bundled_skills_enabled*/ true);

    write_user_skill(&ody_home, "a", "skill-a", "from a");
    let outcome1 =
        skills_for_config_with_stack(&skills_service, &cwd, &config_layer_stack, &[]).await;
    assert!(
        outcome1.skills.iter().any(|s| s.name == "skill-a"),
        "expected skill-a to be discovered"
    );

    // Write a new skill after the first call; the second call should reuse the config-aware cache
    // entry because the effective skill config is unchanged.
    write_user_skill(&ody_home, "b", "skill-b", "from b");
    let outcome2 =
        skills_for_config_with_stack(&skills_service, &cwd, &config_layer_stack, &[]).await;
    assert_eq!(outcome2.errors, outcome1.errors);
    assert_eq!(outcome2.skills, outcome1.skills);
}

#[tokio::test]
async fn set_extra_roots_replaces_runtime_roots_and_clears_cache() {
    let ody_home = tempfile::tempdir().expect("tempdir");
    let cwd = tempfile::tempdir().expect("tempdir");
    let extra_root = tempfile::tempdir().expect("tempdir");
    let config_layer_stack = config_stack(&ody_home, "");
    let skills_service =
        SkillsService::new(ody_home.path().abs(), /*bundled_skills_enabled*/ true);

    let skills_input = SkillsLoadInput::new(
        cwd.path().abs(),
        Vec::new(),
        config_layer_stack.clone(),
        bundled_skills_enabled_from_stack(&config_layer_stack),
    );
    let empty_snapshot = skills_service
        .snapshot_for_cwd(
            &skills_input,
            /*force_reload*/ false,
            Some(Arc::clone(&LOCAL_FS)),
        )
        .await;
    let empty_outcome = empty_snapshot.outcome();
    assert!(
        empty_outcome
            .skills
            .iter()
            .all(|skill| skill.name != "runtime-skill")
    );

    let extra_skills_root = extra_root.path().join("skills");
    let skill_dir = extra_skills_root.join("runtime-skill");
    fs::create_dir_all(&skill_dir).expect("create skill dir");
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: runtime-skill\ndescription: runtime skill\n---\n\n# Body\n",
    )
    .expect("write skill");
    skills_service.set_extra_roots(vec![extra_skills_root.abs()]);

    let runtime_snapshot = skills_service
        .snapshot_for_cwd(
            &skills_input,
            /*force_reload*/ false,
            Some(Arc::clone(&LOCAL_FS)),
        )
        .await;
    let runtime_outcome = runtime_snapshot.outcome();
    assert!(
        runtime_outcome
            .skills
            .iter()
            .any(|skill| skill.name == "runtime-skill")
    );

    skills_service.set_extra_roots(vec![extra_root.path().join("missing-skills").abs()]);
    let replaced_snapshot = skills_service
        .snapshot_for_cwd(
            &skills_input,
            /*force_reload*/ false,
            Some(Arc::clone(&LOCAL_FS)),
        )
        .await;
    let replaced_outcome = replaced_snapshot.outcome();
    assert_eq!(replaced_outcome.errors, Vec::new());
    assert!(
        replaced_outcome
            .skills
            .iter()
            .all(|skill| skill.name != "runtime-skill")
    );
}

#[tokio::test]
async fn set_extra_roots_applies_to_config_loads_and_empty_clears() {
    let ody_home = tempfile::tempdir().expect("tempdir");
    let cwd = tempfile::tempdir().expect("tempdir");
    let extra_root = tempfile::tempdir().expect("tempdir");
    let config_layer_stack = config_stack(&ody_home, "");
    let skills_service =
        SkillsService::new(ody_home.path().abs(), /*bundled_skills_enabled*/ true);

    let empty_outcome =
        skills_for_config_with_stack(&skills_service, &cwd, &config_layer_stack, &[]).await;
    assert!(
        empty_outcome
            .skills
            .iter()
            .all(|skill| skill.name != "runtime-skill")
    );

    let extra_skills_root = extra_root.path().join("skills");
    let skill_dir = extra_skills_root.join("runtime-skill");
    fs::create_dir_all(&skill_dir).expect("create skill dir");
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: runtime-skill\ndescription: runtime skill\n---\n\n# Body\n",
    )
    .expect("write skill");
    skills_service.set_extra_roots(vec![extra_skills_root.abs()]);

    let runtime_outcome =
        skills_for_config_with_stack(&skills_service, &cwd, &config_layer_stack, &[]).await;
    assert!(
        runtime_outcome
            .skills
            .iter()
            .any(|skill| skill.name == "runtime-skill")
    );

    skills_service.set_extra_roots(Vec::new());
    let cleared_outcome =
        skills_for_config_with_stack(&skills_service, &cwd, &config_layer_stack, &[]).await;
    assert!(
        cleared_outcome
            .skills
            .iter()
            .all(|skill| skill.name != "runtime-skill")
    );
}

#[tokio::test]
async fn skills_for_config_disables_plugin_skills_by_name() {
    let ody_home = tempfile::tempdir().expect("tempdir");
    let cwd = tempfile::tempdir().expect("tempdir");
    let skill_path = write_plugin_skill(
        &ody_home,
        "test",
        "sample",
        "sample-search",
        "sample-search",
        "search sample data",
    );
    let config_layer_stack = config_stack(
        &ody_home,
        &name_toggle_config("sample:sample-search", /*enabled*/ false),
    );
    let plugin_skill_root =
        plugin_skill_root_for_skill_path(&skill_path, "test-plugin@test", "sample");
    let skills_service =
        SkillsService::new(ody_home.path().abs(), /*bundled_skills_enabled*/ true);

    let outcome = skills_for_config_with_stack(
        &skills_service,
        &cwd,
        &config_layer_stack,
        &[plugin_skill_root],
    )
    .await;
    let skill = outcome
        .skills
        .iter()
        .find(|skill| skill.name == "sample:sample-search")
        .expect("plugin skill should load");
    let skill_path = dunce::canonicalize(skill_path)
        .expect("skill path should canonicalize")
        .abs();

    assert_eq!(skill.path_to_skills_md, skill_path);
    assert!(outcome.disabled_paths.contains(&skill.path_to_skills_md));
    assert!(
        !outcome
            .allowed_skills_for_implicit_invocation()
            .iter()
            .any(|allowed_skill| allowed_skill.path_to_skills_md == skill.path_to_skills_md)
    );
}

#[tokio::test]
async fn skills_for_cwd_loads_repo_and_user_roots_with_local_fs() {
    let ody_home = tempfile::tempdir().expect("tempdir");
    let cwd = tempfile::tempdir().expect("tempdir");
    let repo_dot_ody = cwd.path().join(".ody");
    fs::create_dir_all(&repo_dot_ody).expect("create repo config dir");

    write_user_skill(&ody_home, "user", "user-skill", "from local user root");
    let repo_skill_dir = repo_dot_ody.join("skills/repo");
    fs::create_dir_all(&repo_skill_dir).expect("create repo skill dir");
    fs::write(
        repo_skill_dir.join("SKILL.md"),
        "---\nname: repo-skill\ndescription: from repo root\n---\n\n# Body\n",
    )
    .expect("write repo skill");

    let config_layer_stack = ConfigLayerStack::new(
        vec![
            user_config_layer(&ody_home, ""),
            ConfigLayerEntry::new(
                ConfigLayerSource::Project {
                    dot_ody_folder: repo_dot_ody.abs(),
                },
                toml::Value::Table(toml::map::Map::new()),
            ),
        ],
        Default::default(),
        ConfigRequirementsToml::default(),
    )
    .expect("valid config layer stack");
    let skills_input = SkillsLoadInput::new(
        cwd.path().abs(),
        Vec::new(),
        config_layer_stack.clone(),
        bundled_skills_enabled_from_stack(&config_layer_stack),
    );
    let skills_service =
        SkillsService::new(ody_home.path().abs(), /*bundled_skills_enabled*/ true);

    let snapshot = skills_service
        .snapshot_for_cwd(
            &skills_input,
            /*force_reload*/ true,
            Some(Arc::clone(&LOCAL_FS)),
        )
        .await;
    let outcome = snapshot.outcome();

    assert!(
        outcome.errors.is_empty(),
        "unexpected errors: {:?}",
        outcome.errors
    );
    let loaded_names = outcome
        .skills
        .iter()
        .map(|skill| skill.name.as_str())
        .collect::<HashSet<_>>();
    assert!(loaded_names.contains("user-skill"));
    assert!(loaded_names.contains("repo-skill"));
}

#[tokio::test]
async fn skills_for_cwd_without_fs_skips_repo_roots() {
    let ody_home = tempfile::tempdir().expect("tempdir");
    let cwd = tempfile::tempdir().expect("tempdir");
    let repo_dot_ody = cwd.path().join(".ody");
    fs::create_dir_all(&repo_dot_ody).expect("create repo config dir");

    write_user_skill(&ody_home, "user", "user-skill", "from local user root");
    let repo_skill_dir = repo_dot_ody.join("skills/repo");
    fs::create_dir_all(&repo_skill_dir).expect("create repo skill dir");
    fs::write(
        repo_skill_dir.join("SKILL.md"),
        "---\nname: repo-skill\ndescription: from repo root\n---\n\n# Body\n",
    )
    .expect("write repo skill");

    let config_layer_stack = ConfigLayerStack::new(
        vec![
            user_config_layer(&ody_home, ""),
            ConfigLayerEntry::new(
                ConfigLayerSource::Project {
                    dot_ody_folder: repo_dot_ody.abs(),
                },
                toml::Value::Table(toml::map::Map::new()),
            ),
        ],
        Default::default(),
        ConfigRequirementsToml::default(),
    )
    .expect("valid config layer stack");
    let skills_input = SkillsLoadInput::new(
        cwd.path().abs(),
        Vec::new(),
        config_layer_stack.clone(),
        bundled_skills_enabled_from_stack(&config_layer_stack),
    );
    let skills_service =
        SkillsService::new(ody_home.path().abs(), /*bundled_skills_enabled*/ true);

    let snapshot = skills_service
        .snapshot_for_cwd(&skills_input, /*force_reload*/ true, /*fs*/ None)
        .await;
    let outcome = snapshot.outcome();

    assert!(
        outcome.errors.is_empty(),
        "unexpected errors: {:?}",
        outcome.errors
    );
    let loaded_names = outcome
        .skills
        .iter()
        .map(|skill| skill.name.as_str())
        .collect::<HashSet<_>>();
    assert!(loaded_names.contains("user-skill"));
    assert!(!loaded_names.contains("repo-skill"));
}

#[tokio::test]
async fn skills_for_config_excludes_bundled_skills_when_disabled_in_config() {
    let ody_home = tempfile::tempdir().expect("tempdir");
    let cwd = tempfile::tempdir().expect("tempdir");
    let bundled_skill_dir = ody_home.path().join("skills/.system/bundled-skill");
    fs::create_dir_all(&bundled_skill_dir).expect("create bundled skill dir");
    fs::write(
        bundled_skill_dir.join("SKILL.md"),
        "---\nname: bundled-skill\ndescription: from bundled root\n---\n\n# Body\n",
    )
    .expect("write bundled skill");
    let config_layer_stack = config_stack(&ody_home, "[skills.bundled]\nenabled = false\n");
    let skills_service =
        SkillsService::new(ody_home.path().abs(), /*bundled_skills_enabled*/ false);

    // Recreate the cached bundled skill after startup cleanup so this assertion exercises
    // root selection rather than relying on directory removal succeeding.
    fs::create_dir_all(&bundled_skill_dir).expect("recreate bundled skill dir");
    fs::write(
        bundled_skill_dir.join("SKILL.md"),
        "---\nname: bundled-skill\ndescription: from bundled root\n---\n\n# Body\n",
    )
    .expect("rewrite bundled skill");

    let outcome =
        skills_for_config_with_stack(&skills_service, &cwd, &config_layer_stack, &[]).await;
    assert!(
        outcome
            .skills
            .iter()
            .all(|skill| skill.name != "bundled-skill")
    );
    assert!(
        outcome
            .skills
            .iter()
            .all(|skill| skill.scope != SkillScope::System)
    );
}

#[tokio::test]
async fn skills_for_cwd_uses_cached_result_until_force_reload() {
    let ody_home = tempfile::tempdir().expect("tempdir");
    let cwd = tempfile::tempdir().expect("tempdir");
    let config_layer_stack = config_stack(&ody_home, "");
    let skills_service =
        SkillsService::new(ody_home.path().abs(), /*bundled_skills_enabled*/ true);
    let _ = skills_for_config_with_stack(&skills_service, &cwd, &config_layer_stack, &[]).await;
    let base_input = SkillsLoadInput::new(
        cwd.path().abs(),
        Vec::new(),
        config_layer_stack.clone(),
        bundled_skills_enabled_from_stack(&config_layer_stack),
    );
    let snapshot_a = skills_service
        .snapshot_for_cwd(
            &base_input,
            /*force_reload*/ false,
            Some(Arc::clone(&LOCAL_FS)),
        )
        .await;
    let outcome_a = snapshot_a.outcome();
    assert!(
        outcome_a
            .skills
            .iter()
            .all(|skill| skill.name != "late-skill")
    );

    write_user_skill(&ody_home, "late", "late-skill", "added after cache");

    let snapshot_b = skills_service
        .snapshot_for_cwd(
            &base_input,
            /*force_reload*/ false,
            Some(Arc::clone(&LOCAL_FS)),
        )
        .await;
    let outcome_b = snapshot_b.outcome();
    assert!(
        outcome_b
            .skills
            .iter()
            .all(|skill| skill.name != "late-skill")
    );

    let snapshot_reloaded = skills_service
        .snapshot_for_cwd(
            &base_input,
            /*force_reload*/ true,
            Some(Arc::clone(&LOCAL_FS)),
        )
        .await;
    let outcome_reloaded = snapshot_reloaded.outcome();
    assert!(
        outcome_reloaded
            .skills
            .iter()
            .any(|skill| skill.name == "late-skill")
    );
}

#[cfg_attr(windows, ignore)]
#[test]
fn disabled_paths_for_skills_allows_session_flags_to_override_user_layer() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let skill_path = write_demo_skill(&tempdir);
    let skill = test_skill("demo-skill", skill_path.clone());
    let user_file = AbsolutePathBuf::try_from(tempdir.path().join("config.toml"))
        .expect("user config path should be absolute");
    let user_layer = ConfigLayerEntry::new(
        ConfigLayerSource::User {
            file: user_file,
            profile: None,
        },
        toml::from_str(&path_toggle_config(&skill_path, /*enabled*/ false))
            .expect("user layer toml"),
    );
    let session_layer = ConfigLayerEntry::new(
        ConfigLayerSource::SessionFlags,
        toml::from_str(&path_toggle_config(&skill_path, /*enabled*/ true))
            .expect("session layer toml"),
    );
    let stack = ConfigLayerStack::new(
        vec![user_layer, session_layer],
        Default::default(),
        ConfigRequirementsToml::default(),
    )
    .expect("valid config layer stack");

    let skill_config_rules = skill_config_rules_from_stack(&stack);
    assert_eq!(
        resolve_disabled_skill_paths(&[skill], &skill_config_rules),
        HashSet::new()
    );
}

#[cfg_attr(windows, ignore)]
#[test]
fn disabled_paths_for_skills_allows_session_flags_to_disable_user_enabled_skill() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let skill_path = write_demo_skill(&tempdir);
    let skill = test_skill("demo-skill", skill_path.clone());
    let user_file = AbsolutePathBuf::try_from(tempdir.path().join("config.toml"))
        .expect("user config path should be absolute");
    let user_layer = ConfigLayerEntry::new(
        ConfigLayerSource::User {
            file: user_file,
            profile: None,
        },
        toml::from_str(&path_toggle_config(&skill_path, /*enabled*/ true))
            .expect("user layer toml"),
    );
    let session_layer = ConfigLayerEntry::new(
        ConfigLayerSource::SessionFlags,
        toml::from_str(&path_toggle_config(&skill_path, /*enabled*/ false))
            .expect("session layer toml"),
    );
    let stack = ConfigLayerStack::new(
        vec![user_layer, session_layer],
        Default::default(),
        ConfigRequirementsToml::default(),
    )
    .expect("valid config layer stack");

    let skill_config_rules = skill_config_rules_from_stack(&stack);
    assert_eq!(
        resolve_disabled_skill_paths(&[skill], &skill_config_rules),
        HashSet::from([skill_path
            .abs()
            .canonicalize()
            .expect("skill path should canonicalize")])
    );
}

#[cfg_attr(windows, ignore)]
#[test]
fn disabled_paths_for_skills_disables_matching_name_selectors() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let skill_path = write_demo_skill(&tempdir);
    let skill = test_skill("github:yeet", skill_path.clone());
    let user_file = AbsolutePathBuf::try_from(tempdir.path().join("config.toml"))
        .expect("user config path should be absolute");
    let user_layer = ConfigLayerEntry::new(
        ConfigLayerSource::User {
            file: user_file,
            profile: None,
        },
        toml::from_str(&name_toggle_config("github:yeet", /*enabled*/ false))
            .expect("user layer toml"),
    );
    let stack = ConfigLayerStack::new(
        vec![user_layer],
        Default::default(),
        ConfigRequirementsToml::default(),
    )
    .expect("valid config layer stack");

    let skill_config_rules = skill_config_rules_from_stack(&stack);
    assert_eq!(
        resolve_disabled_skill_paths(&[skill], &skill_config_rules),
        HashSet::from([skill_path
            .abs()
            .canonicalize()
            .expect("skill path should canonicalize")])
    );
}

#[cfg_attr(windows, ignore)]
#[test]
fn disabled_paths_for_skills_allows_name_selector_to_override_path_selector() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let skill_path = write_demo_skill(&tempdir);
    let skill = test_skill("github:yeet", skill_path.clone());
    let user_file = AbsolutePathBuf::try_from(tempdir.path().join("config.toml"))
        .expect("user config path should be absolute");
    let user_layer = ConfigLayerEntry::new(
        ConfigLayerSource::User {
            file: user_file,
            profile: None,
        },
        toml::from_str(&path_toggle_config(&skill_path, /*enabled*/ false))
            .expect("user layer toml"),
    );
    let session_layer = ConfigLayerEntry::new(
        ConfigLayerSource::SessionFlags,
        toml::from_str(&name_toggle_config("github:yeet", /*enabled*/ true))
            .expect("session layer toml"),
    );
    let stack = ConfigLayerStack::new(
        vec![user_layer, session_layer],
        Default::default(),
        ConfigRequirementsToml::default(),
    )
    .expect("valid config layer stack");

    let skill_config_rules = skill_config_rules_from_stack(&stack);
    assert_eq!(
        resolve_disabled_skill_paths(&[skill], &skill_config_rules),
        HashSet::new()
    );
}

#[cfg_attr(windows, ignore)]
#[tokio::test]
async fn skills_for_config_ignores_cwd_cache_when_session_flags_reenable_skill() {
    let ody_home = tempfile::tempdir().expect("tempdir");
    let cwd = tempfile::tempdir().expect("tempdir");
    let skill_dir = ody_home.path().join("skills").join("demo");
    fs::create_dir_all(&skill_dir).expect("create skill dir");
    let skill_path = skill_dir.join("SKILL.md");
    fs::write(
        &skill_path,
        "---\nname: demo-skill\ndescription: demo description\n---\n\n# Body\n",
    )
    .expect("write skill");
    let disabled_skill_config = path_toggle_config(&skill_path, /*enabled*/ false);
    let enabled_skill_config = path_toggle_config(&skill_path, /*enabled*/ true);
    let parent_stack = config_stack(&ody_home, &disabled_skill_config);
    let child_stack =
        config_stack_with_session_flags(&ody_home, &disabled_skill_config, &enabled_skill_config);
    let skills_service =
        SkillsService::new(ody_home.path().abs(), /*bundled_skills_enabled*/ true);
    let parent_input = SkillsLoadInput::new(
        cwd.path().abs(),
        Vec::new(),
        parent_stack.clone(),
        bundled_skills_enabled_from_stack(&parent_stack),
    );

    let parent_snapshot = skills_service
        .snapshot_for_cwd(
            &parent_input,
            /*force_reload*/ true,
            Some(Arc::clone(&LOCAL_FS)),
        )
        .await;
    let parent_outcome = parent_snapshot.outcome();
    let parent_skill = parent_outcome
        .skills
        .iter()
        .find(|skill| skill.name == "demo-skill")
        .expect("demo skill should be discovered");
    assert_eq!(parent_outcome.is_skill_enabled(parent_skill), false);

    let child_outcome =
        skills_for_config_with_stack(&skills_service, &cwd, &child_stack, &[]).await;
    let child_skill = child_outcome
        .skills
        .iter()
        .find(|skill| skill.name == "demo-skill")
        .expect("demo skill should be discovered");
    assert_eq!(child_outcome.is_skill_enabled(child_skill), true);
}

#[test]
fn game_skills_enabled_from_stack_defaults_to_false() {
    let ody_home = tempfile::tempdir().expect("tempdir");
    let default_stack = config_stack(&ody_home, "");
    assert!(!game_skills_enabled_from_stack(&default_stack));

    let enabled_stack = config_stack(&ody_home, "[skills.bundled]\ngame = true\n");
    assert!(game_skills_enabled_from_stack(&enabled_stack));

    let disabled_stack = config_stack(
        &ody_home,
        "[skills.bundled]\nenabled = true\ngame = false\n",
    );
    assert!(!game_skills_enabled_from_stack(&disabled_stack));
}

#[tokio::test]
async fn embedded_game_skills_hidden_by_default_and_shown_when_enabled() {
    let ody_home = tempfile::tempdir().expect("tempdir");
    let cwd = tempfile::tempdir().expect("tempdir");
    let skills_service =
        SkillsService::new(ody_home.path().abs(), /*bundled_skills_enabled*/ true);

    let default_stack = config_stack(&ody_home, "");
    let default_outcome =
        skills_for_config_with_stack(&skills_service, &cwd, &default_stack, &[]).await;
    assert!(
        default_outcome
            .skills
            .iter()
            .all(|skill| skill.name != "game" && skill.name != "godot-shaders"),
        "game bundle skills must be hidden unless the game flag is on"
    );
    assert!(
        default_outcome
            .skills
            .iter()
            .any(|skill| skill.name == "debt-ledger"),
        "non-game bundled skills must stay visible"
    );

    // Same roots and skill rules, only the game flag differs: the config cache
    // must not serve the game-hidden snapshot for a game-enabled stack.
    let game_stack = config_stack(&ody_home, "[skills.bundled]\ngame = true\n");
    let game_outcome = skills_for_config_with_stack(&skills_service, &cwd, &game_stack, &[]).await;
    assert!(
        game_outcome.skills.iter().any(|skill| skill.name == "game"),
        "/game skill must be visible when the game flag is on"
    );
    assert!(
        game_outcome
            .skills
            .iter()
            .any(|skill| skill.name == "godot-shaders"),
        "vendored gamedev skills must be visible when the game flag is on"
    );
}

#[tokio::test]
async fn snapshot_for_cwd_cache_separates_game_flag() {
    let ody_home = tempfile::tempdir().expect("tempdir");
    let cwd = tempfile::tempdir().expect("tempdir");
    let skills_service =
        SkillsService::new(ody_home.path().abs(), /*bundled_skills_enabled*/ true);

    let default_stack = config_stack(&ody_home, "");
    let default_input = SkillsLoadInput::new(
        cwd.path().abs(),
        Vec::new(),
        default_stack.clone(),
        bundled_skills_enabled_from_stack(&default_stack),
    );
    let default_snapshot = skills_service
        .snapshot_for_cwd(
            &default_input,
            /*force_reload*/ false,
            Some(Arc::clone(&LOCAL_FS)),
        )
        .await;
    assert!(
        default_snapshot
            .outcome()
            .skills
            .iter()
            .all(|skill| skill.name != "game")
    );

    // A second session in the same cwd with the game flag on must not reuse
    // the cached game-hidden snapshot.
    let game_stack = config_stack(&ody_home, "[skills.bundled]\ngame = true\n");
    let game_input = SkillsLoadInput::new(
        cwd.path().abs(),
        Vec::new(),
        game_stack.clone(),
        bundled_skills_enabled_from_stack(&game_stack),
    );
    let game_snapshot = skills_service
        .snapshot_for_cwd(
            &game_input,
            /*force_reload*/ false,
            Some(Arc::clone(&LOCAL_FS)),
        )
        .await;
    assert!(
        game_snapshot
            .outcome()
            .skills
            .iter()
            .any(|skill| skill.name == "game"),
        "cwd cache must be keyed by the game flag"
    );
}

#[test]
fn frontend_mode_enabled_from_stack_defaults_to_false() {
    let ody_home = tempfile::tempdir().expect("tempdir");
    let default_stack = config_stack(&ody_home, "");
    assert!(!frontend_mode_enabled_from_stack(&default_stack));

    let enabled_stack = config_stack(&ody_home, "[skills.bundled]\nfrontend = true\n");
    assert!(frontend_mode_enabled_from_stack(&enabled_stack));

    let disabled_stack = config_stack(
        &ody_home,
        "[skills.bundled]\nenabled = true\nfrontend = false\n",
    );
    assert!(!frontend_mode_enabled_from_stack(&disabled_stack));

    // The game flag must not switch frontend mode on.
    let game_only = config_stack(&ody_home, "[skills.bundled]\ngame = true\n");
    assert!(!frontend_mode_enabled_from_stack(&game_only));
}

#[tokio::test]
async fn frontend_mode_hides_every_skill_but_the_craft_bundle() {
    let ody_home = tempfile::tempdir().expect("tempdir");
    let cwd = tempfile::tempdir().expect("tempdir");
    write_user_skill(&ody_home, "my-notes", "my-notes", "user skill");
    let skills_service =
        SkillsService::new(ody_home.path().abs(), /*bundled_skills_enabled*/ true);

    // Baseline: the catalog is broad.
    let default_stack = config_stack(&ody_home, "");
    let default_outcome =
        skills_for_config_with_stack(&skills_service, &cwd, &default_stack, &[]).await;
    assert!(
        default_outcome
            .skills
            .iter()
            .any(|skill| skill.name == "debt-ledger"),
        "bundled skills are visible by default"
    );
    assert!(
        default_outcome
            .skills
            .iter()
            .any(|skill| skill.name == "my-notes"),
        "user skills are visible by default"
    );

    // Frontend mode is exclusive: only the two craft skills survive, and the
    // user-installed skill is hidden even though no config rule targeted it.
    let frontend_stack = config_stack(&ody_home, "[skills.bundled]\nfrontend = true\n");
    let frontend_outcome =
        skills_for_config_with_stack(&skills_service, &cwd, &frontend_stack, &[]).await;
    let mut visible: Vec<&str> = frontend_outcome
        .skills
        .iter()
        .map(|skill| skill.name.as_str())
        .collect();
    visible.sort_unstable();
    let mut expected = ody_skills::embedded_frontend_skill_names().to_vec();
    expected.sort_unstable();
    assert_eq!(
        visible, expected,
        "frontend mode exposes exactly make-interfaces-feel-better and motion-design"
    );
    assert!(
        frontend_outcome
            .skills
            .iter()
            .all(|skill| skill.name != "frontend-design"),
        "frontend-design stays out of the frontend bundle"
    );
}

#[tokio::test]
async fn frontend_mode_keeps_the_system_root_when_bundled_skills_are_disabled() {
    // `--frontend` takes its skills from the bundled system root; honouring
    // `bundled.enabled = false` would leave the mode with an empty catalog.
    let ody_home = tempfile::tempdir().expect("tempdir");
    let cwd = tempfile::tempdir().expect("tempdir");
    let skills_service =
        SkillsService::new(ody_home.path().abs(), /*bundled_skills_enabled*/ true);

    let stack = config_stack(
        &ody_home,
        "[skills.bundled]\nenabled = false\nfrontend = true\n",
    );
    let outcome = skills_for_config_with_stack(&skills_service, &cwd, &stack, &[]).await;
    assert_eq!(
        outcome.skills.len(),
        ody_skills::embedded_frontend_skill_names().len(),
        "frontend mode must still load its two skills: {:?}",
        outcome
            .skills
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn snapshot_for_cwd_cache_separates_frontend_mode() {
    let ody_home = tempfile::tempdir().expect("tempdir");
    let cwd = tempfile::tempdir().expect("tempdir");
    let skills_service =
        SkillsService::new(ody_home.path().abs(), /*bundled_skills_enabled*/ true);

    let default_stack = config_stack(&ody_home, "");
    let default_input = SkillsLoadInput::new(
        cwd.path().abs(),
        Vec::new(),
        default_stack.clone(),
        bundled_skills_enabled_from_stack(&default_stack),
    );
    let default_snapshot = skills_service
        .snapshot_for_cwd(
            &default_input,
            /*force_reload*/ false,
            Some(Arc::clone(&LOCAL_FS)),
        )
        .await;
    assert!(
        default_snapshot
            .outcome()
            .skills
            .iter()
            .any(|skill| skill.name == "debt-ledger")
    );

    // A second session in the same cwd with the frontend flag on must not
    // reuse the unrestricted snapshot.
    let frontend_stack = config_stack(&ody_home, "[skills.bundled]\nfrontend = true\n");
    let frontend_input = SkillsLoadInput::new(
        cwd.path().abs(),
        Vec::new(),
        frontend_stack.clone(),
        bundled_skills_enabled_from_stack(&frontend_stack),
    );
    let frontend_snapshot = skills_service
        .snapshot_for_cwd(
            &frontend_input,
            /*force_reload*/ false,
            Some(Arc::clone(&LOCAL_FS)),
        )
        .await;
    let mut visible: Vec<&str> = frontend_snapshot
        .outcome()
        .skills
        .iter()
        .map(|skill| skill.name.as_str())
        .collect();
    visible.sort_unstable();
    let mut expected = ody_skills::embedded_frontend_skill_names().to_vec();
    expected.sort_unstable();
    assert_eq!(
        visible, expected,
        "cwd cache must be keyed by the frontend flag"
    );
}
