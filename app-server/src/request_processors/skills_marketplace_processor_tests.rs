use super::*;

fn skills_root() -> (tempfile::TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("skills");
    std::fs::create_dir_all(&root).expect("create skills root");
    (temp, root)
}

fn install_fake_skill(root: &std::path::Path, name: &str) -> std::path::PathBuf {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).expect("create skill dir");
    std::fs::write(dir.join("SKILL.md"), "---\nname: x\ndescription: y\n---\n").expect("write SKILL.md");
    dir
}

#[test]
fn delete_skill_dir_removes_installed_skill() {
    let (_temp, root) = skills_root();
    install_fake_skill(&root, "my-skill");
    delete_skill_dir(&root, "my-skill").expect("delete should succeed");
    assert!(!root.join("my-skill").exists());
}

#[test]
fn delete_skill_dir_rejects_missing_skill() {
    let (_temp, root) = skills_root();
    let result = delete_skill_dir(&root, "ghost");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not installed"));
}

#[test]
fn delete_skill_dir_rejects_path_traversal() {
    let (_temp, root) = skills_root();
    let result = delete_skill_dir(&root, "../outside");
    assert!(result.is_err());
}

#[test]
fn delete_skill_dir_rejects_nested_and_absolute_paths() {
    let (_temp, root) = skills_root();
    assert!(delete_skill_dir(&root, "a/b").is_err());
    #[cfg(unix)]
    assert!(delete_skill_dir(&root, "/etc/passwd").is_err());
}

#[test]
fn write_then_read_upgrade_source_round_trips() {
    let (_temp, root) = skills_root();
    let dir = install_fake_skill(&root, "my-skill");
    write_skill_source(&dir, "owner/repo", Some("skills/my-skill")).expect("write source");
    let source = read_upgrade_source(&dir).expect("read source");
    assert_eq!(source, "https://github.com/owner/repo");
}

#[test]
fn read_upgrade_source_accepts_legacy_odybox_source_json() {
    let (_temp, root) = skills_root();
    let dir = install_fake_skill(&root, "legacy-skill");
    // odyBox's legacy installer writes {type, repo, skillPath, ...}.
    std::fs::write(
        dir.join("source.json"),
        r#"{"type":"github","repo":"owner/legacy","skillPath":"skills/legacy-skill","treeSha":"abc","commitHash":"def","installedAt":"2026-01-01T00:00:00Z"}"#,
    )
    .expect("write legacy source.json");
    let source = read_upgrade_source(&dir).expect("read legacy source");
    assert_eq!(source, "https://github.com/owner/legacy");
}

#[test]
fn read_upgrade_source_rejects_missing_or_sourceless_skill() {
    let (_temp, root) = skills_root();
    let bare = install_fake_skill(&root, "bare");
    let result = read_upgrade_source(&bare);
    assert!(result.is_err());

    std::fs::write(bare.join("source.json"), r#"{"type":"local"}"#).expect("write sourceless");
    assert!(read_upgrade_source(&bare).is_err());
}

#[test]
fn resolve_upgrade_source_prefers_params_over_recorded_source() {
    let (_temp, root) = skills_root();
    let dir = install_fake_skill(&root, "my-skill");
    write_skill_source(&dir, "owner/recorded", None).expect("write source");
    let resolved = resolve_upgrade_source(Some("https://github.com/owner/params".to_string()), &dir)
        .expect("resolve params source");
    assert_eq!(resolved, "https://github.com/owner/params");
    let resolved = resolve_upgrade_source(None, &dir).expect("resolve recorded source");
    assert_eq!(resolved, "https://github.com/owner/recorded");
    let dir2 = install_fake_skill(&root, "no-source");
    assert!(resolve_upgrade_source(None, &dir2).is_err());
}
