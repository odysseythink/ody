use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use app_test_support::to_response;
use app_test_support::TestAppServer;
use ody_app_server_protocol::ClientInfo;
use ody_app_server_protocol::InitializeCapabilities;
use ody_app_server_protocol::JSONRPCMessage;
use ody_app_server_protocol::RequestId;
use ody_app_server_protocol::WorkspaceChangeSet;
use ody_app_server_protocol::WorkspaceChangeSetApplyParams;
use ody_app_server_protocol::WorkspaceChangeSetCheckpoint;
use ody_app_server_protocol::WorkspaceChangeSetCreateParams;
use ody_app_server_protocol::WorkspaceChangeSetListParams;
use ody_app_server_protocol::WorkspaceChangeSetRejectParams;
use ody_app_server_protocol::WorkspaceChangeSetRestoreParams;
use ody_app_server_protocol::WorkspaceChangeSetStatus;
use ody_app_server_protocol::WorkspaceFileChange;
use ody_app_server_protocol::WorkspaceFileChangeKind;
use ody_app_server_protocol::WorkspaceProjectBindParams;
use ody_app_server_protocol::WorkspaceProjectRef;
use ody_app_server_protocol::WorkspaceSourceIndexParams;
use ody_app_server_protocol::WorkspaceSourceIndexResponse;
use ody_app_server_protocol::WorkspaceSourceQueryKind;
use ody_app_server_protocol::WorkspaceSourceResolveParams;
use ody_app_server_protocol::WorkspaceSourceDiffParams;
use ody_app_server_protocol::WorkspaceSourceDiffResponse;
use ody_app_server_protocol::WorkspaceSourceResolveResponse;
use ody_utils_absolute_path::test_support::PathBufExt;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

fn default_client_info() -> ClientInfo {
    ClientInfo {
        name: app_test_support::DEFAULT_CLIENT_NAME.to_string(),
        title: None,
        version: "0.1.0".to_string(),
    }
}

async fn init_experimental(mcp: &mut TestAppServer) -> Result<()> {
    let init = mcp
        .initialize_with_capabilities(
            default_client_info(),
            Some(InitializeCapabilities {
                experimental_api: true,
                request_attestation: false,
                opt_out_notification_methods: None,
                mcp_server_form_elicitation: false,
            }),
        )
        .await?;
    let JSONRPCMessage::Response(_) = init else {
        anyhow::bail!("expected initialize response, got {init:?}");
    };
    Ok(())
}

fn bind_params(id: &str, roots: Vec<PathBuf>) -> WorkspaceProjectBindParams {
    WorkspaceProjectBindParams {
        id: id.to_owned(),
        name: "fixture project".to_owned(),
        roots: roots.into_iter().map(|path| path.abs()).collect(),
        idempotency_key: format!("key-{id}"),
    }
}

#[derive(serde::Deserialize)]
struct ProjectEnvelope {
    project: WorkspaceProjectRef,
}

async fn read_project(mcp: &mut TestAppServer, request_id: i64) -> Result<WorkspaceProjectRef> {
    let message = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let envelope: ProjectEnvelope = to_response(message)?;
    Ok(envelope.project)
}

/// React/Vite-style fixture: one page, one component, one module.exports util.
fn react_fixture() -> Result<TempDir> {
    let dir = TempDir::new()?;
    fs::write(
        dir.path().join("package.json"),
        serde_json::json!({
            "name": "react-fixture",
            "dependencies": { "react": "^19.0.0", "react-dom": "^19.0.0" },
            "devDependencies": { "vite": "^6.0.0" }
        })
        .to_string(),
    )?;
    let pages = dir.path().join("src/pages");
    fs::create_dir_all(&pages)?;
    fs::write(
        pages.join("HomePage.tsx"),
        "export default function HomePage() {\n  return <main>home</main>;\n}\n",
    )?;
    fs::write(
        pages.join("index.tsx"),
        "export default function Index() {\n  return <main>index</main>;\n}\n",
    )?;
    let components = dir.path().join("src/components");
    fs::create_dir_all(&components)?;
    fs::write(
        components.join("Button.tsx"),
        "export function Button() {\n  return <button>ok</button>;\n}\nexport default Button;\n",
    )?;
    fs::write(
        components.join("format.ts"),
        "module.exports = { format: (x: unknown) => String(x) };\n",
    )?;
    Ok(dir)
}

/// Next app-router style fixture with a self-registering route.
fn next_fixture() -> Result<TempDir> {
    let dir = TempDir::new()?;
    fs::write(
        dir.path().join("package.json"),
        serde_json::json!({
            "name": "next-fixture",
            "dependencies": { "next": "^15.0.0", "react": "^19.0.0" }
        })
        .to_string(),
    )?;
    let blog = dir.path().join("app/blog");
    fs::create_dir_all(&blog)?;
    fs::write(
        blog.join("page.tsx"),
        "export default function BlogPage() {\n  return <main>blog</main>;\n}\n",
    )?;
    Ok(dir)
}

/// FNV-1a checksum for read-only tree snapshots (std-only, no new dev-deps).
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// (len, checksum) per file under root; proves index/resolve never writes.
fn snapshot_tree(root: &Path) -> Result<BTreeMap<PathBuf, (u64, u64)>> {
    let mut map = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_symlink() {
                continue;
            }
            if entry.file_type()?.is_dir() {
                // Checkpoint baselines (.git) are intentional runtime
                // artifacts, not source writes; exclude from snapshots.
                if entry.file_name() != ".git" {
                    stack.push(path);
                }
            } else {
                let bytes = fs::read(&path)?;
                map.insert(path, (bytes.len() as u64, fnv1a(&bytes)));
            }
        }
    }
    Ok(map)
}

#[tokio::test]
async fn index_returns_artifacts_and_refs_with_symbol_range_and_hash() -> Result<()> {
    let ody_home = TempDir::new()?;
    let fixture = react_fixture()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    let bind_id = mcp
        .send_workspace_project_bind_request(bind_params(
            "ws-src-1",
            vec![fixture.path().to_path_buf()],
        ))
        .await?;
    read_project(&mut mcp, bind_id).await?;

    let index_id = mcp
        .send_workspace_source_index_request(WorkspaceSourceIndexParams {
            project_id: "ws-src-1".to_owned(),
        })
        .await?;
    let message = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(index_id)),
    )
    .await??;
    let response: WorkspaceSourceIndexResponse = to_response(message)?;
    let index = response.index;
    assert!(!index.truncated, "errors: {:?}", index.errors);
    assert_eq!(index.artifacts.len(), 4);
    assert_eq!(index.artifacts.len(), index.refs.len(), "1:1 parallel vectors");

    let home = index
        .artifacts
        .iter()
        .find(|a| a.name == "HomePage")
        .expect("HomePage artifact");
    assert_eq!(home.path, "src/pages/HomePage.tsx");
    assert_eq!(home.framework.as_deref(), Some("react"));

    let home_ref = index
        .refs
        .iter()
        .find(|r| r.artifact_id == home.id)
        .expect("HomePage ref");
    assert_eq!(home_ref.symbol, "HomePage");
    let range = home_ref.range.expect("export default function gives range");
    assert_eq!((range.start_line, range.end_line), (1, 3));
    assert_eq!(home_ref.file_hash.len(), 64);
    assert_eq!(home_ref.content_hash.len(), 64);
    assert_ne!(
        home_ref.content_hash, home_ref.file_hash,
        "range is a strict prefix of the file"
    );

    // Determinism: a second index of unchanged content agrees on hashes.
    let index_id2 = mcp
        .send_workspace_source_index_request(WorkspaceSourceIndexParams {
            project_id: "ws-src-1".to_owned(),
        })
        .await?;
    let message2 = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(index_id2)),
    )
    .await??;
    let response2: WorkspaceSourceIndexResponse = to_response(message2)?;
    assert_eq!(index.refs, response2.index.refs);
    Ok(())
}

#[tokio::test]
async fn index_falls_back_to_file_level_for_module_exports_and_sfc() -> Result<()> {
    let ody_home = TempDir::new()?;
    let fixture = react_fixture()?;
    let vue_dir = fixture.path().join("src/views");
    fs::create_dir_all(&vue_dir)?;
    fs::write(
        vue_dir.join("HomeView.vue"),
        "<template><div>hi</div></template>\n<script setup>\nconst x = 1;\n</script>\n",
    )?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    let bind_id = mcp
        .send_workspace_project_bind_request(bind_params(
            "ws-src-2",
            vec![fixture.path().to_path_buf()],
        ))
        .await?;
    read_project(&mut mcp, bind_id).await?;

    let index_id = mcp
        .send_workspace_source_index_request(WorkspaceSourceIndexParams {
            project_id: "ws-src-2".to_owned(),
        })
        .await?;
    let message = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(index_id)),
    )
    .await??;
    let response: WorkspaceSourceIndexResponse = to_response(message)?;

    let util = response
        .index
        .refs
        .iter()
        .find(|r| r.file_path == "src/components/format.ts")
        .expect("format.ts ref");
    assert_eq!(util.symbol, "format");
    assert!(util.range.is_none(), "module.exports stays file-level");
    assert_eq!(util.content_hash, util.file_hash);

    let vue = response
        .index
        .refs
        .iter()
        .find(|r| r.file_path == "src/views/HomeView.vue")
        .expect("HomeView ref");
    assert_eq!(vue.symbol, "HomeView");
    assert!(vue.range.is_none(), "Vue SFC stays file-level");
    Ok(())
}

#[tokio::test]
async fn resolve_by_symbol_name_and_route_path() -> Result<()> {
    let ody_home = TempDir::new()?;
    let next = next_fixture()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    let bind_id = mcp
        .send_workspace_project_bind_request(bind_params(
            "ws-src-3",
            vec![next.path().to_path_buf()],
        ))
        .await?;
    read_project(&mut mcp, bind_id).await?;

    let resolve_symbol = mcp
        .send_workspace_source_resolve_request(WorkspaceSourceResolveParams {
            project_id: "ws-src-3".to_owned(),
            kind: WorkspaceSourceQueryKind::Symbol,
            value: "BlogPage".to_owned(),
            limit: None,
        })
        .await?;
    let message = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(resolve_symbol)),
    )
    .await??;
    let response: WorkspaceSourceResolveResponse = to_response(message)?;
    assert_eq!(response.matches.len(), 1);
    assert_eq!(response.matches[0].source_ref.symbol, "BlogPage");
    assert_eq!(
        response.matches[0].artifact.route_path.as_deref(),
        Some("/blog")
    );

    let resolve_route = mcp
        .send_workspace_source_resolve_request(WorkspaceSourceResolveParams {
            project_id: "ws-src-3".to_owned(),
            kind: WorkspaceSourceQueryKind::RoutePath,
            value: "/blog".to_owned(),
            limit: None,
        })
        .await?;
    let message = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(resolve_route)),
    )
    .await??;
    let response: WorkspaceSourceResolveResponse = to_response(message)?;
    assert_eq!(response.matches.len(), 1);
    assert_eq!(response.matches[0].artifact.name, "page");

    let resolve_unknown = mcp
        .send_workspace_source_resolve_request(WorkspaceSourceResolveParams {
            project_id: "ws-src-3".to_owned(),
            kind: WorkspaceSourceQueryKind::Name,
            value: "DoesNotExist".to_owned(),
            limit: None,
        })
        .await?;
    let message = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(resolve_unknown)),
    )
    .await??;
    let response: WorkspaceSourceResolveResponse = to_response(message)?;
    assert!(response.matches.is_empty());
    Ok(())
}

#[tokio::test]
async fn index_and_resolve_are_read_only_and_unknown_project_is_diagnosable() -> Result<()> {
    let ody_home = TempDir::new()?;
    let fixture = react_fixture()?;
    let before = snapshot_tree(fixture.path())?;

    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    let bind_id = mcp
        .send_workspace_project_bind_request(bind_params(
            "ws-src-4",
            vec![fixture.path().to_path_buf()],
        ))
        .await?;
    read_project(&mut mcp, bind_id).await?;

    let index_id = mcp
        .send_workspace_source_index_request(WorkspaceSourceIndexParams {
            project_id: "ws-src-4".to_owned(),
        })
        .await?;
    let message = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(index_id)),
    )
    .await??;
    let _: WorkspaceSourceIndexResponse = to_response(message)?;

    let resolve_id = mcp
        .send_workspace_source_resolve_request(WorkspaceSourceResolveParams {
            project_id: "ws-src-4".to_owned(),
            kind: WorkspaceSourceQueryKind::Symbol,
            value: "Button".to_owned(),
            limit: None,
        })
        .await?;
    let message = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(resolve_id)),
    )
    .await??;
    let _: WorkspaceSourceResolveResponse = to_response(message)?;

    assert_eq!(
        before,
        snapshot_tree(fixture.path())?,
        "index/resolve must not modify any file"
    );

    // Unknown project id → JSON-RPC error response, diagnosable message.
    let bad_id = mcp
        .send_workspace_source_index_request(WorkspaceSourceIndexParams {
            project_id: "no-such-project".to_owned(),
        })
        .await?;
    let error = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(bad_id)),
    )
    .await??;
    assert!(
        error.error.message.contains("unknown workspace project id"),
        "{}",
        error.error.message
    );
    Ok(())
}

async fn read_changeset(mcp: &mut TestAppServer, request_id: i64) -> Result<WorkspaceChangeSet> {
    let message = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    #[derive(serde::Deserialize)]
    struct Envelope {
        changeset: WorkspaceChangeSet,
    }
    Ok(to_response::<Envelope>(message)?.changeset)
}

async fn read_changeset_error(mcp: &mut TestAppServer, request_id: i64) -> Result<String> {
    let error = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;
    Ok(error.error.message)
}

/// Full-file sha256 for a workspace file, taken from the source index.
async fn file_hash_via_index(
    mcp: &mut TestAppServer,
    project_id: &str,
    file_path: &str,
) -> Result<String> {
    let index_id = mcp
        .send_workspace_source_index_request(WorkspaceSourceIndexParams {
            project_id: project_id.to_owned(),
        })
        .await?;
    let message = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(index_id)),
    )
    .await??;
    let response: WorkspaceSourceIndexResponse = to_response(message)?;
    Ok(response
        .index
        .refs
        .iter()
        .find(|r| r.file_path == file_path)
        .unwrap_or_else(|| panic!("ref for {file_path}"))
        .file_hash
        .clone())
}

fn git_available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

#[tokio::test]
async fn changeset_create_apply_restore_roundtrip() -> Result<()> {
    let ody_home = TempDir::new()?;
    let fixture = react_fixture()?;
    let original = fs::read_to_string(fixture.path().join("src/pages/HomePage.tsx"))?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    let bind_id = mcp
        .send_workspace_project_bind_request(bind_params(
            "ws-cs-1",
            vec![fixture.path().to_path_buf()],
        ))
        .await?;
    read_project(&mut mcp, bind_id).await?;

    let base_hash = file_hash_via_index(&mut mcp, "ws-cs-1", "src/pages/HomePage.tsx").await?;
    let create_id = mcp
        .send_workspace_source_changeset_create_request(WorkspaceChangeSetCreateParams {
            project_id: "ws-cs-1".to_owned(),
            title: "update homepage".to_owned(),
            changes: vec![WorkspaceFileChange {
                root_index: 0,
                path: "src/pages/HomePage.tsx".to_owned(),
                kind: WorkspaceFileChangeKind::Update,
                base_hash: Some(base_hash),
                content: Some(
                    "export default function HomePage() {\n  return <main>updated</main>;\n}\n"
                        .to_owned(),
                ),
            }],
            idempotency_key: "roundtrip-1".to_owned(),
        })
        .await?;
    let changeset = read_changeset(&mut mcp, create_id).await?;
    assert_eq!(changeset.status, WorkspaceChangeSetStatus::Pending);
    assert!(changeset
        .unified_diff
        .contains("-  return <main>home</main>;"));
    assert!(changeset
        .unified_diff
        .contains("+  return <main>updated</main>;"));

    // Idempotent create retry returns the same changeset.
    let retry_id = mcp
        .send_workspace_source_changeset_create_request(WorkspaceChangeSetCreateParams {
            project_id: "ws-cs-1".to_owned(),
            title: "update homepage".to_owned(),
            changes: vec![],
            idempotency_key: "roundtrip-1".to_owned(),
        })
        .await?;
    let retried = read_changeset(&mut mcp, retry_id).await?;
    assert_eq!(
        retried.id, changeset.id,
        "idempotent retry returns existing record"
    );

    let apply_id = mcp
        .send_workspace_source_changeset_apply_request(WorkspaceChangeSetApplyParams {
            changeset_id: changeset.id.clone(),
        })
        .await?;
    let applied = read_changeset(&mut mcp, apply_id).await?;
    assert_eq!(applied.status, WorkspaceChangeSetStatus::Applied);
    assert!(matches!(
        applied.checkpoint,
        WorkspaceChangeSetCheckpoint::Git { .. }
    ));
    assert_eq!(
        fs::read_to_string(fixture.path().join("src/pages/HomePage.tsx"))?,
        "export default function HomePage() {\n  return <main>updated</main>;\n}\n"
    );

    let restore_id = mcp
        .send_workspace_source_changeset_restore_request(WorkspaceChangeSetRestoreParams {
            changeset_id: changeset.id.clone(),
        })
        .await?;
    let restored = read_changeset(&mut mcp, restore_id).await?;
    assert_eq!(restored.status, WorkspaceChangeSetStatus::Restored);
    assert_eq!(
        fs::read_to_string(fixture.path().join("src/pages/HomePage.tsx"))?,
        original
    );
    Ok(())
}

#[tokio::test]
async fn apply_rejects_stale_base_hash_without_writing() -> Result<()> {
    let ody_home = TempDir::new()?;
    let fixture = react_fixture()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    let bind_id = mcp
        .send_workspace_project_bind_request(bind_params(
            "ws-cs-2",
            vec![fixture.path().to_path_buf()],
        ))
        .await?;
    read_project(&mut mcp, bind_id).await?;

    let base_hash = file_hash_via_index(&mut mcp, "ws-cs-2", "src/pages/HomePage.tsx").await?;
    let create_id = mcp
        .send_workspace_source_changeset_create_request(WorkspaceChangeSetCreateParams {
            project_id: "ws-cs-2".to_owned(),
            title: "stale apply".to_owned(),
            changes: vec![WorkspaceFileChange {
                root_index: 0,
                path: "src/pages/HomePage.tsx".to_owned(),
                kind: WorkspaceFileChangeKind::Update,
                base_hash: Some(base_hash),
                content: Some("export default function HomePage() {\n  return <main>new</main>;\n}\n".to_owned()),
            }],
            idempotency_key: "stale-1".to_owned(),
        })
        .await?;
    let changeset = read_changeset(&mut mcp, create_id).await?;

    // External edit between create and apply.
    let external = "export default function HomePage() {\n  return <main>external</main>;\n}\n";
    fs::write(fixture.path().join("src/pages/HomePage.tsx"), external)?;

    let apply_id = mcp
        .send_workspace_source_changeset_apply_request(WorkspaceChangeSetApplyParams {
            changeset_id: changeset.id.clone(),
        })
        .await?;
    let message = read_changeset_error(&mut mcp, apply_id).await?;
    assert!(message.contains("changed on disk"), "{message}");
    assert_eq!(
        fs::read_to_string(fixture.path().join("src/pages/HomePage.tsx"))?,
        external,
        "apply must not overwrite the external edit"
    );
    Ok(())
}

#[tokio::test]
async fn create_verifies_base_hash_and_rejects_mismatch() -> Result<()> {
    let ody_home = TempDir::new()?;
    let fixture = react_fixture()?;
    let before = snapshot_tree(fixture.path())?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    let bind_id = mcp
        .send_workspace_project_bind_request(bind_params(
            "ws-cs-3",
            vec![fixture.path().to_path_buf()],
        ))
        .await?;
    read_project(&mut mcp, bind_id).await?;

    let create_id = mcp
        .send_workspace_source_changeset_create_request(WorkspaceChangeSetCreateParams {
            project_id: "ws-cs-3".to_owned(),
            title: "bad hash".to_owned(),
            changes: vec![WorkspaceFileChange {
                root_index: 0,
                path: "src/pages/HomePage.tsx".to_owned(),
                kind: WorkspaceFileChangeKind::Update,
                base_hash: Some("aa".repeat(32)),
                content: Some("export default function HomePage() {}".to_owned()),
            }],
            idempotency_key: "badhash-1".to_owned(),
        })
        .await?;
    let message = read_changeset_error(&mut mcp, create_id).await?;
    assert!(message.contains("baseHash mismatch"), "{message}");
    assert_eq!(before, snapshot_tree(fixture.path())?, "create must not write");
    Ok(())
}

#[tokio::test]
async fn create_rejects_escape_paths_and_symlink_escape() -> Result<()> {
    let ody_home = TempDir::new()?;
    let fixture = react_fixture()?;
    let outside = TempDir::new()?;
    let outside_before = snapshot_tree(outside.path())?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    let bind_id = mcp
        .send_workspace_project_bind_request(bind_params(
            "ws-cs-4",
            vec![fixture.path().to_path_buf()],
        ))
        .await?;
    read_project(&mut mcp, bind_id).await?;

    let escape_id = mcp
        .send_workspace_source_changeset_create_request(WorkspaceChangeSetCreateParams {
            project_id: "ws-cs-4".to_owned(),
            title: "escape".to_owned(),
            changes: vec![WorkspaceFileChange {
                root_index: 0,
                path: "../outside.txt".to_owned(),
                kind: WorkspaceFileChangeKind::Add,
                base_hash: None,
                content: Some("evil".to_owned()),
            }],
            idempotency_key: "escape-1".to_owned(),
        })
        .await?;
    let message = read_changeset_error(&mut mcp, escape_id).await?;
    assert!(message.contains("escape"), "{message}");

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(outside.path(), fixture.path().join("link"))?;
        let symlink_id = mcp
            .send_workspace_source_changeset_create_request(WorkspaceChangeSetCreateParams {
                project_id: "ws-cs-4".to_owned(),
                title: "symlink escape".to_owned(),
                changes: vec![WorkspaceFileChange {
                    root_index: 0,
                    path: "link/evil.txt".to_owned(),
                    kind: WorkspaceFileChangeKind::Add,
                    base_hash: None,
                    content: Some("evil".to_owned()),
                }],
                idempotency_key: "escape-2".to_owned(),
            })
            .await?;
        let message = read_changeset_error(&mut mcp, symlink_id).await?;
        assert!(message.contains("outside"), "{message}");
    }

    assert_eq!(
        outside_before,
        snapshot_tree(outside.path())?,
        "nothing may be written outside the root"
    );
    Ok(())
}

#[tokio::test]
async fn reject_blocks_apply_and_double_apply_fails() -> Result<()> {
    let ody_home = TempDir::new()?;
    let fixture = react_fixture()?;
    let original = fs::read_to_string(fixture.path().join("src/pages/HomePage.tsx"))?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    let bind_id = mcp
        .send_workspace_project_bind_request(bind_params(
            "ws-cs-5",
            vec![fixture.path().to_path_buf()],
        ))
        .await?;
    read_project(&mut mcp, bind_id).await?;

    let base_hash = file_hash_via_index(&mut mcp, "ws-cs-5", "src/pages/HomePage.tsx").await?;
    let make_change = |key: &str| WorkspaceChangeSetCreateParams {
        project_id: "ws-cs-5".to_owned(),
        title: "reject me".to_owned(),
        changes: vec![WorkspaceFileChange {
            root_index: 0,
            path: "src/pages/HomePage.tsx".to_owned(),
            kind: WorkspaceFileChangeKind::Update,
            base_hash: Some(base_hash.clone()),
            content: Some("export default function HomePage() {\n  return <main>rejected</main>;\n}\n".to_owned()),
        }],
        idempotency_key: key.to_owned(),
    };

    let create_id = mcp
        .send_workspace_source_changeset_create_request(make_change("reject-a"))
        .await?;
    let changeset = read_changeset(&mut mcp, create_id).await?;
    let reject_id = mcp
        .send_workspace_source_changeset_reject_request(WorkspaceChangeSetRejectParams {
            changeset_id: changeset.id.clone(),
        })
        .await?;
    let rejected = read_changeset(&mut mcp, reject_id).await?;
    assert_eq!(rejected.status, WorkspaceChangeSetStatus::Rejected);

    let apply_id = mcp
        .send_workspace_source_changeset_apply_request(WorkspaceChangeSetApplyParams {
            changeset_id: changeset.id.clone(),
        })
        .await?;
    let message = read_changeset_error(&mut mcp, apply_id).await?;
    assert!(message.contains("Rejected"), "{message}");
    assert_eq!(
        fs::read_to_string(fixture.path().join("src/pages/HomePage.tsx"))?,
        original,
        "reject and failed apply must not write"
    );

    // Double apply: second apply on an Applied changeset fails.
    let create_id = mcp
        .send_workspace_source_changeset_create_request(make_change("reject-b"))
        .await?;
    let changeset = read_changeset(&mut mcp, create_id).await?;
    let apply_id = mcp
        .send_workspace_source_changeset_apply_request(WorkspaceChangeSetApplyParams {
            changeset_id: changeset.id.clone(),
        })
        .await?;
    let applied = read_changeset(&mut mcp, apply_id).await?;
    assert_eq!(applied.status, WorkspaceChangeSetStatus::Applied);
    let apply_again = mcp
        .send_workspace_source_changeset_apply_request(WorkspaceChangeSetApplyParams {
            changeset_id: changeset.id.clone(),
        })
        .await?;
    let message = read_changeset_error(&mut mcp, apply_again).await?;
    assert!(message.contains("Applied"), "{message}");
    Ok(())
}

#[tokio::test]
async fn restore_rejects_external_modification_after_apply() -> Result<()> {
    let ody_home = TempDir::new()?;
    let fixture = react_fixture()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    let bind_id = mcp
        .send_workspace_project_bind_request(bind_params(
            "ws-cs-6",
            vec![fixture.path().to_path_buf()],
        ))
        .await?;
    read_project(&mut mcp, bind_id).await?;

    let base_hash = file_hash_via_index(&mut mcp, "ws-cs-6", "src/pages/HomePage.tsx").await?;
    let create_id = mcp
        .send_workspace_source_changeset_create_request(WorkspaceChangeSetCreateParams {
            project_id: "ws-cs-6".to_owned(),
            title: "restore guard".to_owned(),
            changes: vec![WorkspaceFileChange {
                root_index: 0,
                path: "src/pages/HomePage.tsx".to_owned(),
                kind: WorkspaceFileChangeKind::Update,
                base_hash: Some(base_hash),
                content: Some("export default function HomePage() {\n  return <main>applied</main>;\n}\n".to_owned()),
            }],
            idempotency_key: "restore-guard-1".to_owned(),
        })
        .await?;
    let changeset = read_changeset(&mut mcp, create_id).await?;
    let apply_id = mcp
        .send_workspace_source_changeset_apply_request(WorkspaceChangeSetApplyParams {
            changeset_id: changeset.id.clone(),
        })
        .await?;
    let applied = read_changeset(&mut mcp, apply_id).await?;
    assert_eq!(applied.status, WorkspaceChangeSetStatus::Applied);

    // User edits the file after apply.
    let user_edit = "export default function HomePage() {\n  return <main>user edit</main>;\n}\n";
    fs::write(fixture.path().join("src/pages/HomePage.tsx"), user_edit)?;

    let restore_id = mcp
        .send_workspace_source_changeset_restore_request(WorkspaceChangeSetRestoreParams {
            changeset_id: changeset.id.clone(),
        })
        .await?;
    let message = read_changeset_error(&mut mcp, restore_id).await?;
    assert!(message.contains("changed after apply"), "{message}");
    assert_eq!(
        fs::read_to_string(fixture.path().join("src/pages/HomePage.tsx"))?,
        user_edit,
        "restore must not overwrite the user edit"
    );
    Ok(())
}

#[tokio::test]
async fn apply_adds_page_and_diff_reports_unified_and_git_diff() -> Result<()> {
    let ody_home = TempDir::new()?;
    let fixture = react_fixture()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    let bind_id = mcp
        .send_workspace_project_bind_request(bind_params(
            "ws-cs-7",
            vec![fixture.path().to_path_buf()],
        ))
        .await?;
    read_project(&mut mcp, bind_id).await?;

    let create_id = mcp
        .send_workspace_source_changeset_create_request(WorkspaceChangeSetCreateParams {
            project_id: "ws-cs-7".to_owned(),
            title: "add contact page".to_owned(),
            changes: vec![WorkspaceFileChange {
                root_index: 0,
                path: "src/pages/Contact.tsx".to_owned(),
                kind: WorkspaceFileChangeKind::Add,
                base_hash: None,
                content: Some(
                    "export default function Contact() {\n  return <main>contact</main>;\n}\n"
                        .to_owned(),
                ),
            }],
            idempotency_key: "add-page-1".to_owned(),
        })
        .await?;
    let changeset = read_changeset(&mut mcp, create_id).await?;
    assert!(changeset.unified_diff.contains("new file mode"));

    let apply_id = mcp
        .send_workspace_source_changeset_apply_request(WorkspaceChangeSetApplyParams {
            changeset_id: changeset.id.clone(),
        })
        .await?;
    let applied = read_changeset(&mut mcp, apply_id).await?;
    assert_eq!(applied.status, WorkspaceChangeSetStatus::Applied);
    assert_eq!(
        fs::read_to_string(fixture.path().join("src/pages/Contact.tsx"))?,
        "export default function Contact() {\n  return <main>contact</main>;\n}\n"
    );

    // The new page is resolvable through the source index.
    let resolve_id = mcp
        .send_workspace_source_resolve_request(WorkspaceSourceResolveParams {
            project_id: "ws-cs-7".to_owned(),
            kind: WorkspaceSourceQueryKind::Name,
            value: "Contact".to_owned(),
            limit: None,
        })
        .await?;
    let message = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(resolve_id)),
    )
    .await??;
    let response: WorkspaceSourceResolveResponse = to_response(message)?;
    assert_eq!(response.matches.len(), 1);
    assert_eq!(response.matches[0].source_ref.symbol, "Contact");

    // Diff response carries the per-changeset unified diff and a git channel.
    let diff_id = mcp
        .send_workspace_source_diff_request(WorkspaceSourceDiffParams {
            project_id: "ws-cs-7".to_owned(),
        })
        .await?;
    let message = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(diff_id)),
    )
    .await??;
    let response: WorkspaceSourceDiffResponse = to_response(message)?;
    let entry = response
        .changesets
        .iter()
        .find(|entry| entry.id == changeset.id)
        .expect("changeset diff entry");
    assert_eq!(entry.status, WorkspaceChangeSetStatus::Applied);
    assert!(entry.unified_diff.contains("new file mode"));
    match response.git_diff {
        Some(git_diff) => {
            assert_eq!(git_diff.available, git_diff.unified_diff.is_some());
            assert_eq!(git_diff.available, git_diff.error.is_none());
        }
        None => panic!("baseline checkpoint makes the root a git repo; git_diff expected"),
    }
    Ok(())
}

#[tokio::test]
async fn apply_records_head_checkpoint_for_git_repo_without_new_commits() -> Result<()> {
    if !git_available() {
        eprintln!("git binary unavailable; skipping");
        return Ok(());
    }
    let ody_home = TempDir::new()?;
    let fixture = react_fixture()?;
    std::process::Command::new("git")
        .arg("init")
        .arg(fixture.path())
        .output()?;
    std::process::Command::new("git")
        .args(["-C"])
        .arg(fixture.path())
        .args(["add", "."])
        .output()?;
    std::process::Command::new("git")
        .args(["-C"])
        .arg(fixture.path())
        .args([
            "-c",
            "user.name=ody-test",
            "-c",
            "user.email=ody-test@example.com",
            "commit",
            "-m",
            "init",
        ])
        .output()?;
    let head_before = String::from_utf8(
        std::process::Command::new("git")
            .args(["-C"])
            .arg(fixture.path())
            .args(["rev-parse", "HEAD"])
            .output()?
            .stdout,
    )?;

    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    let bind_id = mcp
        .send_workspace_project_bind_request(bind_params(
            "ws-cs-8",
            vec![fixture.path().to_path_buf()],
        ))
        .await?;
    read_project(&mut mcp, bind_id).await?;

    let base_hash = file_hash_via_index(&mut mcp, "ws-cs-8", "src/pages/HomePage.tsx").await?;
    let create_id = mcp
        .send_workspace_source_changeset_create_request(WorkspaceChangeSetCreateParams {
            project_id: "ws-cs-8".to_owned(),
            title: "git checkpoint".to_owned(),
            changes: vec![WorkspaceFileChange {
                root_index: 0,
                path: "src/pages/HomePage.tsx".to_owned(),
                kind: WorkspaceFileChangeKind::Update,
                base_hash: Some(base_hash),
                content: Some("export default function HomePage() {\n  return <main>git</main>;\n}\n".to_owned()),
            }],
            idempotency_key: "git-checkpoint-1".to_owned(),
        })
        .await?;
    let changeset = read_changeset(&mut mcp, create_id).await?;
    let apply_id = mcp
        .send_workspace_source_changeset_apply_request(WorkspaceChangeSetApplyParams {
            changeset_id: changeset.id.clone(),
        })
        .await?;
    let applied = read_changeset(&mut mcp, apply_id).await?;
    match &applied.checkpoint {
        WorkspaceChangeSetCheckpoint::Git { head_commit_hash } => {
            assert_eq!(
                head_commit_hash.as_deref(),
                Some(head_before.trim()),
                "checkpoint records HEAD; no new commit may be created"
            );
        }
        other => panic!("expected git checkpoint, got {other:?}"),
    }

    // The change stays in the working tree; nothing is committed.
    let status = String::from_utf8(
        std::process::Command::new("git")
            .args(["-C"])
            .arg(fixture.path())
            .args(["status", "--porcelain"])
            .output()?
            .stdout,
    )?;
    assert!(
        status.contains("src/pages/HomePage.tsx"),
        "working tree change expected, got: {status}"
    );
    Ok(())
}

#[tokio::test]
async fn changeset_never_writes_outside_root() -> Result<()> {
    let ody_home = TempDir::new()?;
    let fixture = react_fixture()?;
    let sibling = TempDir::new()?;
    let sibling_before = snapshot_tree(sibling.path())?;
    let root_before = snapshot_tree(fixture.path())?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    let bind_id = mcp
        .send_workspace_project_bind_request(bind_params(
            "ws-cs-9",
            vec![fixture.path().to_path_buf()],
        ))
        .await?;
    read_project(&mut mcp, bind_id).await?;

    let base_hash = file_hash_via_index(&mut mcp, "ws-cs-9", "src/pages/HomePage.tsx").await?;
    let create_id = mcp
        .send_workspace_source_changeset_create_request(WorkspaceChangeSetCreateParams {
            project_id: "ws-cs-9".to_owned(),
            title: "inside only".to_owned(),
            changes: vec![WorkspaceFileChange {
                root_index: 0,
                path: "src/pages/HomePage.tsx".to_owned(),
                kind: WorkspaceFileChangeKind::Update,
                base_hash: Some(base_hash),
                content: Some("export default function HomePage() {\n  return <main>in</main>;\n}\n".to_owned()),
            }],
            idempotency_key: "inside-only-1".to_owned(),
        })
        .await?;
    let changeset = read_changeset(&mut mcp, create_id).await?;
    let apply_id = mcp
        .send_workspace_source_changeset_apply_request(WorkspaceChangeSetApplyParams {
            changeset_id: changeset.id.clone(),
        })
        .await?;
    let _ = read_changeset(&mut mcp, apply_id).await?;
    assert_eq!(sibling_before, snapshot_tree(sibling.path())?, "sibling untouched after apply");

    let restore_id = mcp
        .send_workspace_source_changeset_restore_request(WorkspaceChangeSetRestoreParams {
            changeset_id: changeset.id.clone(),
        })
        .await?;
    let _ = read_changeset(&mut mcp, restore_id).await?;
    assert_eq!(root_before, snapshot_tree(fixture.path())?, "root back to base after restore");
    assert_eq!(sibling_before, snapshot_tree(sibling.path())?, "sibling untouched after restore");
    Ok(())
}

#[tokio::test]
async fn changeset_list_scopes_to_project_and_orders_newest_first() -> Result<()> {
    let ody_home = TempDir::new()?;
    let fixture = react_fixture()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    let bind_id = mcp
        .send_workspace_project_bind_request(bind_params(
            "ws-cs-10",
            vec![fixture.path().to_path_buf()],
        ))
        .await?;
    read_project(&mut mcp, bind_id).await?;

    let base_hash = file_hash_via_index(&mut mcp, "ws-cs-10", "src/pages/HomePage.tsx").await?;
    let mut ids = Vec::new();
    for (i, key) in ["list-a", "list-b"].iter().enumerate() {
        let create_id = mcp
            .send_workspace_source_changeset_create_request(WorkspaceChangeSetCreateParams {
                project_id: "ws-cs-10".to_owned(),
                title: format!("cs {i}"),
                changes: vec![WorkspaceFileChange {
                    root_index: 0,
                    path: "src/pages/HomePage.tsx".to_owned(),
                    kind: WorkspaceFileChangeKind::Update,
                    base_hash: Some(base_hash.clone()),
                    content: Some(format!("export default function HomePage() {{\n  return <main>{i}</main>;\n}}\n")),
                }],
                idempotency_key: (*key).to_owned(),
            })
            .await?;
        ids.push(read_changeset(&mut mcp, create_id).await?.id);
    }

    let list_id = mcp
        .send_workspace_source_changeset_list_request(WorkspaceChangeSetListParams {
            project_id: "ws-cs-10".to_owned(),
        })
        .await?;
    let message = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(list_id)),
    )
    .await??;
    #[derive(serde::Deserialize)]
    struct ListEnvelope {
        changesets: Vec<WorkspaceChangeSet>,
    }
    let listed = to_response::<ListEnvelope>(message)?.changesets;
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].id, ids[1], "newest first");
    assert_eq!(listed[1].id, ids[0]);

    // A different project sees none of them.
    let other_dir = TempDir::new()?;
    let bind_id = mcp
        .send_workspace_project_bind_request(bind_params(
            "ws-cs-10b",
            vec![other_dir.path().to_path_buf()],
        ))
        .await?;
    read_project(&mut mcp, bind_id).await?;
    let list_id = mcp
        .send_workspace_source_changeset_list_request(WorkspaceChangeSetListParams {
            project_id: "ws-cs-10b".to_owned(),
        })
        .await?;
    let message = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(list_id)),
    )
    .await??;
    let listed = to_response::<ListEnvelope>(message)?.changesets;
    assert!(listed.is_empty());
    Ok(())
}
