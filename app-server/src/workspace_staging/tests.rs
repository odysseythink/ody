use super::*;
use std::path::PathBuf;
use ody_app_server_protocol::WorkspaceRoot;
use ody_app_server_protocol::WorkspaceRootRole;

fn fixture_stores(
    root: &Path,
) -> (
    Arc<Mutex<WorkspaceProjectStore>>,
    Arc<Mutex<WorkspaceSourceStore>>,
    Arc<WorkspaceAuditLog>,
    tempfile::TempDir,
) {
    let ody_home = tempfile::tempdir().expect("ody home tempdir");
    let project_store = Arc::new(Mutex::new(WorkspaceProjectStore::default()));
    project_store
        .lock()
        .expect("lock")
        .projects
        .insert(
            "ws-1".to_owned(),
            WorkspaceProjectRef {
                id: "ws-1".to_owned(),
                name: "fixture".to_owned(),
                schema_version: 1,
                roots: vec![WorkspaceRoot {
                    path: root.to_string_lossy().into_owned(),
                    role: WorkspaceRootRole::Primary,
                    auth_source: "user_selected".to_owned(),
                }],
                created_at_ms: 0,
                updated_at_ms: 0,
                locked_by: None,
            },
        );
    let store = Arc::new(Mutex::new(WorkspaceSourceStore::load(
        ody_home.path().join("workspace-source").join("v1.json"),
    )));
    let audit = Arc::new(WorkspaceAuditLog::new(
        ody_home.path().join("workspace-audit").join("v1.jsonl"),
    ));
    (project_store, store, audit, ody_home)
}

fn write(old: Option<&str>, new: Option<&str>, path: PathBuf) -> StagedWrite {
    StagedWrite {
        path,
        old_content: old.map(str::to_owned),
        new_content: new.map(str::to_owned),
    }
}

#[test]
fn find_project_matches_longest_root_prefix_with_path_boundary() {
    let outer = WorkspaceProjectRef {
        id: "outer".to_owned(),
        name: "outer".to_owned(),
        schema_version: 1,
        roots: vec![WorkspaceRoot {
            path: "/repo/app".to_owned(),
            role: WorkspaceRootRole::Primary,
            auth_source: "user_selected".to_owned(),
        }],
        created_at_ms: 0,
        updated_at_ms: 0,
        locked_by: None,
    };
    let inner = WorkspaceProjectRef {
        id: "inner".to_owned(),
        name: "inner".to_owned(),
        schema_version: 1,
        roots: vec![WorkspaceRoot {
            path: "/repo/app/packages/web".to_owned(),
            role: WorkspaceRootRole::Primary,
            auth_source: "user_selected".to_owned(),
        }],
        created_at_ms: 0,
        updated_at_ms: 0,
        locked_by: None,
    };
    let projects = vec![outer, inner];

    let (project, index, relative) =
        find_project_for_path(&projects, Path::new("/repo/app/packages/web/src/a.ts"))
            .expect("inner root wins");
    assert_eq!(project.id, "inner");
    assert_eq!(index, 0);
    assert_eq!(relative, "src/a.ts");

    let (project, _, relative) =
        find_project_for_path(&projects, Path::new("/repo/app/src/b.ts")).expect("outer matches");
    assert_eq!(project.id, "outer");
    assert_eq!(relative, "src/b.ts");

    // 路径边界：/repo/app-other 不属于 /repo/app
    assert!(find_project_for_path(&projects, Path::new("/repo/app-other/x.ts")).is_none());
}

#[test]
fn stage_write_creates_pending_changeset_with_base_and_content() {
    let dir = tempfile::tempdir().expect("root tempdir");
    let (project_store, store, audit, _home) = fixture_stores(dir.path());
    let collector = WorkspaceStagingCollector::new(project_store, store.clone(), audit);
    let path = dir.path().join("src").join("a.ts");
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");

    collector.stage_write(
        "thread-1",
        write(Some("old\n"), Some("new\n"), path.clone()),
    );

    let store = store.lock().expect("store lock");
    assert_eq!(store.changesets.len(), 1);
    let stored = store.changesets.values().next().expect("one changeset");
    assert_eq!(stored.change_set.project_id, "ws-1");
    assert!(stored.change_set.title.starts_with(STAGING_TITLE_PREFIX));
    assert_eq!(
        stored.change_set.status,
        WorkspaceChangeSetStatus::Pending
    );
    assert_eq!(stored.change_set.changes.len(), 1);
    let change = &stored.change_set.changes[0];
    assert_eq!(change.root_index, 0);
    assert_eq!(change.path, "src/a.ts");
    assert_eq!(change.kind, WorkspaceFileChangeKind::Update);
    assert_eq!(
        change.base_hash.as_deref(),
        Some(crate::workspace_changeset::sha256_hex(b"old\n").as_str())
    );
    assert_eq!(change.content.as_deref(), Some("new\n"));
    assert_eq!(
        stored.base_contents.get("0:src/a.ts").map(String::as_str),
        Some("old\n")
    );
    // diff 来自 base 快照而非磁盘
    assert!(stored.change_set.unified_diff.contains("-old"));
    assert!(stored.change_set.unified_diff.contains("+new"));
    assert!(stored.change_set.unified_diff.contains("src/a.ts"));
}

#[test]
#[cfg(unix)]
fn stage_write_resolves_symlinked_parent_against_canonical_roots() {
    // bind 落库的 root 是 canonical 形式；写路径来自 core 的 cwd 拼接，
    // 可能带符号链接分量（macOS /tmp -> /private/tmp 同源问题），
    // 必须仍能归属。文件不存在（删除写入）时走父目录解析分支。
    let dir = tempfile::tempdir().expect("real root tempdir");
    let real_root = dir.path().canonicalize().expect("canonical root");
    let link = dir
        .path()
        .parent()
        .expect("parent")
        .join(format!("staging-link-{}", std::process::id()));
    let _ = std::fs::remove_file(&link);
    std::os::unix::fs::symlink(&real_root, &link).expect("symlink");
    let (project_store, store, audit, _home) = fixture_stores(&real_root);
    let collector = WorkspaceStagingCollector::new(project_store, store.clone(), audit);
    std::fs::create_dir_all(real_root.join("src")).expect("mkdir");
    let path_via_link = link.join("src").join("a.ts");

    collector.stage_write(
        "thread-1",
        write(Some("old\n"), Some("new\n"), path_via_link),
    );

    let store = store.lock().expect("store lock");
    assert_eq!(store.changesets.len(), 1);
    let change = &store
        .changesets
        .values()
        .next()
        .expect("one changeset")
        .change_set
        .changes[0];
    assert_eq!(change.path, "src/a.ts");
    assert_eq!(change.kind, WorkspaceFileChangeKind::Update);
    let _ = std::fs::remove_file(&link);
}

#[test]
fn stage_write_ignores_paths_outside_project_roots() {
    let dir = tempfile::tempdir().expect("root tempdir");
    let (project_store, store, audit, _home) = fixture_stores(dir.path());
    let collector = WorkspaceStagingCollector::new(project_store, store.clone(), audit);

    collector.stage_write(
        "thread-1",
        write(Some("old\n"), Some("new\n"), PathBuf::from("/elsewhere/a.ts")),
    );

    assert!(store.lock().expect("store lock").changesets.is_empty());
}

#[test]
fn repeated_writes_keep_first_base_and_roll_content_in_one_changeset() {
    let dir = tempfile::tempdir().expect("root tempdir");
    let (project_store, store, audit, _home) = fixture_stores(dir.path());
    let collector = WorkspaceStagingCollector::new(project_store, store.clone(), audit);
    let path = dir.path().join("src").join("a.ts");
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");

    collector.stage_write("thread-1", write(Some("v0\n"), Some("v1\n"), path.clone()));
    collector.stage_write("thread-1", write(Some("v1\n"), Some("v2\n"), path.clone()));

    let store = store.lock().expect("store lock");
    assert_eq!(store.changesets.len(), 1, "同一 project 复用打开中的暂存");
    let stored = store.changesets.values().next().expect("one");
    assert_eq!(stored.change_set.changes.len(), 1);
    let change = &stored.change_set.changes[0];
    // base 保持首次触碰前内容
    assert_eq!(
        change.base_hash.as_deref(),
        Some(crate::workspace_changeset::sha256_hex(b"v0\n").as_str())
    );
    assert_eq!(change.content.as_deref(), Some("v2\n"));
    assert!(stored.change_set.unified_diff.contains("-v0"));
    assert!(stored.change_set.unified_diff.contains("+v2"));
}

#[test]
fn new_write_after_changeset_closed_creates_a_new_changeset() {
    let dir = tempfile::tempdir().expect("root tempdir");
    let (project_store, store, audit, _home) = fixture_stores(dir.path());
    let collector = WorkspaceStagingCollector::new(project_store, store.clone(), audit);
    let path = dir.path().join("src").join("a.ts");
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");

    collector.stage_write("thread-1", write(Some("v0\n"), Some("v1\n"), path.clone()));
    let first_id = {
        let mut store = store.lock().expect("store lock");
        let id = store.changesets.keys().next().expect("id").clone();
        store
            .changesets
            .get_mut(&id)
            .expect("stored")
            .change_set
            .status = WorkspaceChangeSetStatus::Applied;
        id
    };

    collector.stage_write("thread-1", write(Some("v1\n"), Some("v2\n"), path.clone()));

    let store = store.lock().expect("store lock");
    assert_eq!(store.changesets.len(), 2);
    assert!(store.changesets.contains_key(&first_id));
}

#[test]
fn add_and_delete_kinds_map_from_presence_of_contents() {
    let dir = tempfile::tempdir().expect("root tempdir");
    let (project_store, store, audit, _home) = fixture_stores(dir.path());
    let collector = WorkspaceStagingCollector::new(project_store, store.clone(), audit);
    let added = dir.path().join("new.ts");
    let deleted = dir.path().join("gone.ts");
    std::fs::write(&deleted, "bye\n").expect("seed deleted file");

    collector.stage_write("thread-1", write(None, Some("added\n"), added));
    collector.stage_write("thread-1", write(Some("bye\n"), None, deleted.clone()));

    let store = store.lock().expect("store lock");
    let stored = store.changesets.values().next().expect("one changeset");
    let add = stored
        .change_set
        .changes
        .iter()
        .find(|c| c.path == "new.ts")
        .expect("add change");
    assert_eq!(add.kind, WorkspaceFileChangeKind::Add);
    assert_eq!(add.base_hash, None);
    let delete = stored
        .change_set
        .changes
        .iter()
        .find(|c| c.path == "gone.ts")
        .expect("delete change");
    assert_eq!(delete.kind, WorkspaceFileChangeKind::Delete);
    assert_eq!(delete.content, None);
}

#[test]
fn is_recent_staged_tracks_staged_hashes_within_cap() {
    let dir = tempfile::tempdir().expect("root tempdir");
    let (project_store, store, audit, _home) = fixture_stores(dir.path());
    let collector = WorkspaceStagingCollector::new(project_store, store, audit);

    for i in 0..(RECENT_STAGED_CAP + 10) {
        collector.record_recent_staged(
            format!("0:file_{i}.ts"),
            crate::workspace_changeset::sha256_hex(format!("v{i}\n").as_bytes()),
        );
    }

    let last = RECENT_STAGED_CAP + 9;
    let recent_hash =
        crate::workspace_changeset::sha256_hex(format!("v{last}\n").as_bytes());
    assert!(collector.is_recent_staged(&format!("0:file_{last}.ts"), &recent_hash));
    // 超出容量的旧记录被逐出
    let evicted_hash =
        crate::workspace_changeset::sha256_hex(b"v0\n");
    assert!(!collector.is_recent_staged("0:file_0.ts", &evicted_hash));
    assert!(!collector.is_recent_staged("0:file_137.ts", &evicted_hash));
}

#[test]
fn stage_write_attributes_to_project_added_to_disk_after_load() {
    // C1-fix2 回归：归属必须以磁盘为准，不能信内存快照。磁盘在 load 之后
    // 新增了 ws-2(rootB)（外部进程/接管窗口写入），写 rootB 下的文件必须
    // 归属 ws-2——内存快照只有 ws-1(rootA) 时若无 reload 会归属失败、
    // renderer 确认卡片恒空。
    let root_a = tempfile::tempdir().expect("root a");
    let root_b = tempfile::tempdir().expect("root b");
    let ody_home = tempfile::tempdir().expect("ody home");
    let project_path = ody_home.path().join("workspace-project").join("v1.json");
    std::fs::create_dir_all(project_path.parent().expect("parent")).expect("mkdir");
    let project_json = |extra: &str| {
        format!(
            r#"{{
  "schema_version": 1,
  "projects": {{
    "ws-1": {{
      "id": "ws-1",
      "name": "a",
      "schemaVersion": 1,
      "roots": [{{
        "path": {:?},
        "role": "primary",
        "authSource": "user_selected"
      }}],
      "createdAtMs": 0,
      "updatedAtMs": 0
    }}{extra}
  }},
  "idempotency": {{}}
}}"#,
            root_a.path().display()
        )
    };
    std::fs::write(&project_path, project_json("")).expect("initial store");
    let project_store = Arc::new(Mutex::new(WorkspaceProjectStore::load(project_path.clone())));
    let store = Arc::new(Mutex::new(WorkspaceSourceStore::load(
        ody_home.path().join("workspace-source").join("v1.json"),
    )));
    let audit = Arc::new(WorkspaceAuditLog::new(
        ody_home.path().join("workspace-audit").join("v1.jsonl"),
    ));
    let collector = WorkspaceStagingCollector::new(project_store, store, audit);

    // 外部视角：磁盘加 ws-2(rootB)，内存快照不知情。
    let ws2 = format!(
        r#",
    "ws-2": {{
      "id": "ws-2",
      "name": "b",
      "schemaVersion": 1,
      "roots": [{{
        "path": {:?},
        "role": "primary",
        "authSource": "user_selected"
      }}],
      "createdAtMs": 0,
      "updatedAtMs": 0
    }}"#,
        root_b.path().display()
    );
    std::fs::write(&project_path, project_json(&ws2)).expect("external store update");

    collector.stage_write(
        "sess-1",
        write(
            None,
            Some("hello"),
            root_b.path().join("note.txt"),
        ),
    );

    let store = collector.store.lock().expect("store lock");
    let staged = store
        .changesets
        .values()
        .filter(|stored| stored.change_set.title.starts_with(STAGING_TITLE_PREFIX))
        .collect::<Vec<_>>();
    assert_eq!(staged.len(), 1, "exactly one staging changeset expected");
    assert_eq!(staged[0].change_set.project_id, "ws-2");
}
