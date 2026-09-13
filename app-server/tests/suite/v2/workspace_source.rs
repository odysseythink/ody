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
use ody_app_server_protocol::WorkspaceSourceValidateParams;
use ody_app_server_protocol::WorkspaceSourceValidateResponse;
use ody_app_server_protocol::WorkspaceValidationCheck;
use ody_app_server_protocol::WorkspaceValidationKind;
use ody_app_server_protocol::WorkspaceValidationOverall;
use ody_app_server_protocol::WorkspaceValidationStatus;
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
            "devDependencies": { "vite": "^6.0.0" },
            "scripts": {
                "build": "node -e \"console.log('build ok')\"",
                "test": "node -e \"console.log('test ok')\""
            }
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

fn node_available() -> bool {
    std::process::Command::new("node")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// Fixture with node-based scripts that need no `npm install`.
fn node_scripts_fixture() -> Result<TempDir> {
    let dir = TempDir::new()?;
    fs::write(
        dir.path().join("package.json"),
        serde_json::json!({
            "name": "validate-fixture",
            "scripts": {
                "build": "node -e \"console.log('build ok')\"",
                "test": "node -e \"console.log('tests ok'); process.exit(0)\"",
                "typecheck": "node -e \"process.exit(3)\"",
                "lint": "node --check src/index.js",
                "slow": "node -e \"setTimeout(() => {}, 60000)\"",
                "missing": "node -e \"process.exit(0)\""
            }
        })
        .to_string(),
    )?;
    let src = dir.path().join("src");
    fs::create_dir_all(&src)?;
    fs::write(src.join("index.js"), "module.exports = 1;\n")?;
    Ok(dir)
}

async fn read_validate(
    mcp: &mut TestAppServer,
    request_id: i64,
) -> Result<WorkspaceSourceValidateResponse> {
    let message = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    Ok(to_response::<WorkspaceSourceValidateResponse>(message)?)
}

async fn bind_validate_fixture(
    mcp: &mut TestAppServer,
    fixture: &TempDir,
    project_id: &str,
) -> Result<()> {
    let bind_id = mcp
        .send_workspace_project_bind_request(bind_params(
            project_id,
            vec![fixture.path().to_path_buf()],
        ))
        .await?;
    read_project(mcp, bind_id).await?;
    Ok(())
}

#[tokio::test]
async fn validate_runs_project_scripts_and_reports_structured_results() -> Result<()> {
    if !node_available() {
        eprintln!("skipping: node unavailable");
        return Ok(());
    }
    let ody_home = TempDir::new()?;
    let fixture = node_scripts_fixture()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    bind_validate_fixture(&mut mcp, &fixture, "ws-val-1").await?;

    let validate_id = mcp
        .send_workspace_source_validate_request(WorkspaceSourceValidateParams {
            project_id: "ws-val-1".to_owned(),
            changeset_id: None,
            checks: vec![
                WorkspaceValidationCheck {
                    kind: WorkspaceValidationKind::Build,
                    script: "build".to_owned(),
                },
                WorkspaceValidationCheck {
                    kind: WorkspaceValidationKind::Test,
                    script: "test".to_owned(),
                },
            ],
            timeout_ms: None,
        })
        .await?;
    let response = read_validate(&mut mcp, validate_id).await?;
    assert_eq!(response.report.runs.len(), 2);
    assert_eq!(response.report.overall, WorkspaceValidationOverall::Succeeded);
    assert_eq!(response.report.runs[0].status, WorkspaceValidationStatus::Succeeded);
    assert_eq!(response.report.runs[0].exit_code, Some(0));
    assert!(response.report.runs[1].stdout_tail.contains("tests ok"));
    assert!(response.report.runs[0].duration_ms >= 0);
    Ok(())
}

#[tokio::test]
async fn validate_reports_failed_exit_code_and_stderr_tail() -> Result<()> {
    if !node_available() {
        eprintln!("skipping: node unavailable");
        return Ok(());
    }
    let ody_home = TempDir::new()?;
    let fixture = node_scripts_fixture()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    bind_validate_fixture(&mut mcp, &fixture, "ws-val-2").await?;

    // typecheck exits 3; lint (node --check) succeeds: overall Failed but
    // per-run statuses differ — proving runs are independent.
    let validate_id = mcp
        .send_workspace_source_validate_request(WorkspaceSourceValidateParams {
            project_id: "ws-val-2".to_owned(),
            changeset_id: None,
            checks: vec![
                WorkspaceValidationCheck {
                    kind: WorkspaceValidationKind::Typecheck,
                    script: "typecheck".to_owned(),
                },
                WorkspaceValidationCheck {
                    kind: WorkspaceValidationKind::Format,
                    script: "lint".to_owned(),
                },
            ],
            timeout_ms: None,
        })
        .await?;
    let response = read_validate(&mut mcp, validate_id).await?;
    assert_eq!(response.report.overall, WorkspaceValidationOverall::Failed);
    assert_eq!(response.report.runs[0].status, WorkspaceValidationStatus::Failed);
    assert_eq!(response.report.runs[0].exit_code, Some(3));
    assert_eq!(response.report.runs[1].status, WorkspaceValidationStatus::Succeeded);
    Ok(())
}

#[tokio::test]
async fn validate_times_out_long_running_script() -> Result<()> {
    if !node_available() {
        eprintln!("skipping: node unavailable");
        return Ok(());
    }
    let ody_home = TempDir::new()?;
    let fixture = node_scripts_fixture()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    bind_validate_fixture(&mut mcp, &fixture, "ws-val-3").await?;

    let validate_id = mcp
        .send_workspace_source_validate_request(WorkspaceSourceValidateParams {
            project_id: "ws-val-3".to_owned(),
            changeset_id: None,
            checks: vec![WorkspaceValidationCheck {
                kind: WorkspaceValidationKind::Build,
                script: "slow".to_owned(),
            }],
            timeout_ms: Some(1_000), // clamped minimum
        })
        .await?;
    let response = read_validate(&mut mcp, validate_id).await?;
    assert_eq!(response.report.runs[0].status, WorkspaceValidationStatus::TimedOut);
    // The timed-out process must not outlive the request (strategy 8.2).
    // kill_on_drop kills it; nothing to poll for a node child, so assert
    // the structured status only.
    Ok(())
}

#[tokio::test]
async fn validate_spawn_error_when_no_package_json() -> Result<()> {
    let ody_home = TempDir::new()?;
    let empty = TempDir::new()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    bind_validate_fixture(&mut mcp, &empty, "ws-val-4").await?;

    let validate_id = mcp
        .send_workspace_source_validate_request(WorkspaceSourceValidateParams {
            project_id: "ws-val-4".to_owned(),
            changeset_id: None,
            checks: vec![WorkspaceValidationCheck {
                kind: WorkspaceValidationKind::Build,
                script: "build".to_owned(),
            }],
            timeout_ms: None,
        })
        .await?;
    let response = read_validate(&mut mcp, validate_id).await?;
    assert_eq!(response.report.runs[0].status, WorkspaceValidationStatus::SpawnError);
    assert!(response.report.runs[0].stderr_tail.contains("no package.json"));
    // 不依赖 node——本用例无 node_available 门控。
    Ok(())
}

#[tokio::test]
async fn validate_unknown_script_fails_with_diagnosable_stderr() -> Result<()> {
    if !node_available() {
        eprintln!("skipping: node unavailable");
        return Ok(());
    }
    let ody_home = TempDir::new()?;
    let fixture = node_scripts_fixture()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    bind_validate_fixture(&mut mcp, &fixture, "ws-val-5").await?;

    let validate_id = mcp
        .send_workspace_source_validate_request(WorkspaceSourceValidateParams {
            project_id: "ws-val-5".to_owned(),
            changeset_id: None,
            checks: vec![WorkspaceValidationCheck {
                kind: WorkspaceValidationKind::Build,
                script: "nosuchscript".to_owned(),
            }],
            timeout_ms: None,
        })
        .await?;
    let response = read_validate(&mut mcp, validate_id).await?;
    assert_eq!(response.report.runs[0].status, WorkspaceValidationStatus::Failed);
    assert_ne!(response.report.runs[0].exit_code, Some(0));
    assert!(!response.report.runs[0].stderr_tail.is_empty());
    Ok(())
}

#[tokio::test]
async fn validate_rejects_unknown_project_and_mismatched_changeset() -> Result<()> {
    let ody_home = TempDir::new()?;
    let fixture = react_fixture()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    bind_validate_fixture(&mut mcp, &fixture, "ws-val-6").await?;

    // unknown project
    let bad = mcp
        .send_workspace_source_validate_request(WorkspaceSourceValidateParams {
            project_id: "nope".to_owned(),
            changeset_id: None,
            checks: vec![WorkspaceValidationCheck {
                kind: WorkspaceValidationKind::Build,
                script: "build".to_owned(),
            }],
            timeout_ms: None,
        })
        .await?;
    let message = read_changeset_error(&mut mcp, bad).await?;
    assert!(
        message.contains("unknown workspace project id"),
        "{message}"
    );

    // changeset from another project in the same store
    let other_fixture = react_fixture()?;
    let bind_id = mcp
        .send_workspace_project_bind_request(bind_params(
            "ws-other",
            vec![other_fixture.path().to_path_buf()],
        ))
        .await?;
    read_project(&mut mcp, bind_id).await?;
    let base_hash = file_hash_via_index(&mut mcp, "ws-other", "src/pages/HomePage.tsx").await?;
    let create_id = mcp
        .send_workspace_source_changeset_create_request(WorkspaceChangeSetCreateParams {
            project_id: "ws-other".to_owned(),
            title: "cs".to_owned(),
            changes: vec![WorkspaceFileChange {
                root_index: 0,
                path: "src/pages/HomePage.tsx".to_owned(),
                kind: WorkspaceFileChangeKind::Update,
                base_hash: Some(base_hash),
                content: Some("export default function HomePage() { return null; }\n".to_owned()),
            }],
            idempotency_key: "cross-1".to_owned(),
        })
        .await?;
    let changeset = read_changeset(&mut mcp, create_id).await?;

    let mismatched = mcp
        .send_workspace_source_validate_request(WorkspaceSourceValidateParams {
            project_id: "ws-val-6".to_owned(),
            changeset_id: Some(changeset.id),
            checks: vec![WorkspaceValidationCheck {
                kind: WorkspaceValidationKind::Build,
                script: "build".to_owned(),
            }],
            timeout_ms: None,
        })
        .await?;
    let message = read_changeset_error(&mut mcp, mismatched).await?;
    assert!(message.contains("does not belong"), "{message}");
    Ok(())
}

/// Vue SFC fixture: script-setup view + defineComponent component.
fn vue_fixture() -> Result<TempDir> {
    let dir = TempDir::new()?;
    fs::write(
        dir.path().join("package.json"),
        serde_json::json!({
            "name": "vue-fixture",
            "dependencies": { "vue": "^3.5.0" },
            "scripts": { "build": "node -e \"console.log('vue build ok')\"" }
        })
        .to_string(),
    )?;
    let views = dir.path().join("src/views");
    fs::create_dir_all(&views)?;
    fs::write(
        views.join("HomeView.vue"),
        "<template><div>home</div></template>\n<script setup>\nconst title = 'home';\n</script>\n",
    )?;
    let components = dir.path().join("src/components");
    fs::create_dir_all(&components)?;
    fs::write(
        components.join("HelloWorld.vue"),
        "<template><p>hello</p></template>\n<script>\nexport default defineComponent({ name: 'HelloWorld' });\n</script>\n",
    )?;
    Ok(dir)
}

/// Fullstack two-root fixture: sibling frontend/ and backend/ roots
/// (E0 bind rejects nesting, so siblings are required).
fn fullstack_fixture() -> Result<(TempDir, PathBuf, PathBuf)> {
    let dir = TempDir::new()?;
    let frontend = dir.path().join("frontend");
    let backend = dir.path().join("backend");
    fs::create_dir_all(frontend.join("src/pages"))?;
    fs::create_dir_all(backend.join("src/routes"))?;
    fs::write(
        frontend.join("package.json"),
        serde_json::json!({
            "name": "frontend",
            "dependencies": { "react": "^19.0.0" },
            "scripts": { "build": "node -e \"console.log('frontend build ok')\"" }
        })
        .to_string(),
    )?;
    fs::write(
        frontend.join("src/pages/HomePage.tsx"),
        "export default function HomePage() {\n  return <main>frontend home</main>;\n}\n",
    )?;
    fs::write(
        backend.join("package.json"),
        serde_json::json!({
            "name": "backend",
            "dependencies": { "express": "^4.21.0" },
            "scripts": { "test": "node -e \"console.log('backend tests ok')\"" }
        })
        .to_string(),
    )?;
    fs::write(
        // 文件放在 src/routes/ 下使索引引擎（pages/views/routes/components
        // 约定目录）为其产出 SourceRef——file_hash 作为 changeset base_hash。
        backend.join("src/routes/handler.js"),
        "module.exports = function handler(req) {\n  return { status: 200 };\n};\n",
    )?;
    Ok((dir, frontend, backend))
}

/// Asserts the on-disk file hash equals a previously captured index
/// file_hash (sha256 of whole file — base/applied state bridge).
async fn assert_file_hash_via_index(
    mcp: &mut TestAppServer,
    project_id: &str,
    file_path: &str,
    expected_file_hash: &str,
) -> Result<()> {
    let actual = file_hash_via_index(mcp, project_id, file_path).await?;
    assert_eq!(actual, expected_file_hash, "{file_path} content changed unexpectedly");
    Ok(())
}

#[tokio::test]
async fn e1_archetype_react_vite_modify_page_refactor_component_add_page() -> Result<()> {
    let ody_home = TempDir::new()?;
    let fixture = react_fixture()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    let bind_id = mcp
        .send_workspace_project_bind_request(bind_params("e1-react", vec![fixture.path().to_path_buf()]))
        .await?;
    read_project(&mut mcp, bind_id).await?;

    // Baseline hashes from the index (file_hash = whole-file sha256).
    let home_base = file_hash_via_index(&mut mcp, "e1-react", "src/pages/HomePage.tsx").await?;
    let button_base = file_hash_via_index(&mut mcp, "e1-react", "src/components/Button.tsx").await?;

    // 操作 1: 修改页面.
    let new_home = "export default function HomePage() {\n  return <main>home v2</main>;\n}\n";
    let create1 = mcp
        .send_workspace_source_changeset_create_request(WorkspaceChangeSetCreateParams {
            project_id: "e1-react".to_owned(),
            title: "modify page".to_owned(),
            changes: vec![WorkspaceFileChange {
                root_index: 0,
                path: "src/pages/HomePage.tsx".to_owned(),
                kind: WorkspaceFileChangeKind::Update,
                base_hash: Some(home_base.clone()),
                content: Some(new_home.to_owned()),
            }],
            idempotency_key: "e1-react-op1".to_owned(),
        })
        .await?;
    let cs1 = read_changeset(&mut mcp, create1).await?;
    let apply1 = mcp
        .send_workspace_source_changeset_apply_request(WorkspaceChangeSetApplyParams {
            changeset_id: cs1.id.clone(),
        })
        .await?;
    assert_eq!(read_changeset(&mut mcp, apply1).await?.status, WorkspaceChangeSetStatus::Applied);
    assert_eq!(fs::read_to_string(fixture.path().join("src/pages/HomePage.tsx"))?, new_home);

    // 操作 2: 重构组件（加 prop 并改内部实现）.
    let new_button = "export function Button({ label }: { label: string }) {\n  return <button>{label}</button>;\n}\nexport default Button;\n";
    let button_base_now = file_hash_via_index(&mut mcp, "e1-react", "src/components/Button.tsx").await?;
    assert_eq!(button_base_now, button_base, "op1 must not touch Button.tsx");
    let create2 = mcp
        .send_workspace_source_changeset_create_request(WorkspaceChangeSetCreateParams {
            project_id: "e1-react".to_owned(),
            title: "refactor component".to_owned(),
            changes: vec![WorkspaceFileChange {
                root_index: 0,
                path: "src/components/Button.tsx".to_owned(),
                kind: WorkspaceFileChangeKind::Update,
                base_hash: Some(button_base_now),
                content: Some(new_button.to_owned()),
            }],
            idempotency_key: "e1-react-op2".to_owned(),
        })
        .await?;
    let cs2 = read_changeset(&mut mcp, create2).await?;
    let apply2 = mcp
        .send_workspace_source_changeset_apply_request(WorkspaceChangeSetApplyParams {
            changeset_id: cs2.id.clone(),
        })
        .await?;
    assert_eq!(read_changeset(&mut mcp, apply2).await?.status, WorkspaceChangeSetStatus::Applied);
    // 重构后 symbol 仍可解析（导出结构未被破坏）.
    let resolve = mcp
        .send_workspace_source_resolve_request(WorkspaceSourceResolveParams {
            project_id: "e1-react".to_owned(),
            kind: WorkspaceSourceQueryKind::Symbol,
            value: "Button".to_owned(),
            limit: None,
        })
        .await?;
    let message = timeout(DEFAULT_TIMEOUT, mcp.read_stream_until_response_message(RequestId::Integer(resolve))).await??;
    let resolved: WorkspaceSourceResolveResponse = to_response(message)?;
    assert_eq!(resolved.matches.len(), 1, "Button still resolvable after refactor");

    // 操作 3: 增加页面（Vite 约定目录新增页面文件）.
    let create3 = mcp
        .send_workspace_source_changeset_create_request(WorkspaceChangeSetCreateParams {
            project_id: "e1-react".to_owned(),
            title: "add page".to_owned(),
            changes: vec![WorkspaceFileChange {
                root_index: 0,
                path: "src/pages/ContactPage.tsx".to_owned(),
                kind: WorkspaceFileChangeKind::Add,
                base_hash: None,
                content: Some("export default function ContactPage() {\n  return <main>contact</main>;\n}\n".to_owned()),
            }],
            idempotency_key: "e1-react-op3".to_owned(),
        })
        .await?;
    let cs3 = read_changeset(&mut mcp, create3).await?;
    let apply3 = mcp
        .send_workspace_source_changeset_apply_request(WorkspaceChangeSetApplyParams {
            changeset_id: cs3.id.clone(),
        })
        .await?;
    assert_eq!(read_changeset(&mut mcp, apply3).await?.status, WorkspaceChangeSetStatus::Applied);
    let index_id = mcp
        .send_workspace_source_index_request(WorkspaceSourceIndexParams { project_id: "e1-react".to_owned() })
        .await?;
    let message = timeout(DEFAULT_TIMEOUT, mcp.read_stream_until_response_message(RequestId::Integer(index_id))).await??;
    let indexed: WorkspaceSourceIndexResponse = to_response(message)?;
    let contact = indexed.index.artifacts.iter().find(|a| a.name == "ContactPage").expect("ContactPage indexed after add");
    assert_eq!(contact.kind, ody_app_server_protocol::WorkspaceSourceKind::Page);

    // 原工程验证（node 门控：脚本为 node 内置能力，无需 npm install）.
    if node_available() {
        let validate_id = mcp
            .send_workspace_source_validate_request(WorkspaceSourceValidateParams {
                project_id: "e1-react".to_owned(),
                changeset_id: Some(cs1.id.clone()),
                checks: vec![
                    WorkspaceValidationCheck { kind: WorkspaceValidationKind::Build, script: "build".to_owned() },
                    WorkspaceValidationCheck { kind: WorkspaceValidationKind::Test, script: "test".to_owned() },
                ],
                timeout_ms: None,
            })
            .await?;
        let report = read_validate(&mut mcp, validate_id).await?;
        assert_eq!(report.report.overall, WorkspaceValidationOverall::Succeeded, "all three ops pass project validation");
    }

    // diff: 三个 changeset 的 unified diff 均可审查.
    let diff_id = mcp
        .send_workspace_source_diff_request(WorkspaceSourceDiffParams { project_id: "e1-react".to_owned() })
        .await?;
    let message = timeout(DEFAULT_TIMEOUT, mcp.read_stream_until_response_message(RequestId::Integer(diff_id))).await??;
    let diff: WorkspaceSourceDiffResponse = to_response(message)?;
    assert_eq!(diff.changesets.len(), 3);
    assert!(diff.changesets.iter().all(|entry| !entry.unified_diff.is_empty()));
    assert!(diff.changesets.iter().any(|entry| entry.unified_diff.contains("new file mode")));

    // 用户恢复：全部 restore 后文件回到 base hash.
    for cs in [cs1, cs2, cs3] {
        let restore_id = mcp
            .send_workspace_source_changeset_restore_request(WorkspaceChangeSetRestoreParams { changeset_id: cs.id })
            .await?;
        assert_eq!(read_changeset(&mut mcp, restore_id).await?.status, WorkspaceChangeSetStatus::Restored);
    }
    assert_file_hash_via_index(&mut mcp, "e1-react", "src/pages/HomePage.tsx", &home_base).await?;
    assert_file_hash_via_index(&mut mcp, "e1-react", "src/components/Button.tsx", &button_base).await?;
    assert!(!fixture.path().join("src/pages/ContactPage.tsx").exists(), "restored add removes the new file");
    Ok(())
}

#[tokio::test]
async fn e1_archetype_next_app_router_add_route() -> Result<()> {
    let ody_home = TempDir::new()?;
    let fixture = next_fixture()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;

    let bind_id = mcp
        .send_workspace_project_bind_request(bind_params("e1-next", vec![fixture.path().to_path_buf()]))
        .await?;
    read_project(&mut mcp, bind_id).await?;

    // 基线：/blog 路由已存在.
    let resolve_blog = mcp
        .send_workspace_source_resolve_request(WorkspaceSourceResolveParams {
            project_id: "e1-next".to_owned(),
            kind: WorkspaceSourceQueryKind::RoutePath,
            value: "/blog".to_owned(),
            limit: None,
        })
        .await?;
    let message = timeout(DEFAULT_TIMEOUT, mcp.read_stream_until_response_message(RequestId::Integer(resolve_blog))).await??;
    let resolved: WorkspaceSourceResolveResponse = to_response(message)?;
    assert_eq!(resolved.matches.len(), 1, "baseline /blog route resolves");

    // 增加路由：app router 自注册——新增 app/about/page.tsx 即新路由.
    let create = mcp
        .send_workspace_source_changeset_create_request(WorkspaceChangeSetCreateParams {
            project_id: "e1-next".to_owned(),
            title: "add /about route".to_owned(),
            changes: vec![WorkspaceFileChange {
                root_index: 0,
                path: "app/about/page.tsx".to_owned(),
                kind: WorkspaceFileChangeKind::Add,
                base_hash: None,
                content: Some("export default function AboutPage() {\n  return <main>about</main>;\n}\n".to_owned()),
            }],
            idempotency_key: "e1-next-route".to_owned(),
        })
        .await?;
    let cs = read_changeset(&mut mcp, create).await?;
    let apply = mcp
        .send_workspace_source_changeset_apply_request(WorkspaceChangeSetApplyParams { changeset_id: cs.id.clone() })
        .await?;
    assert_eq!(read_changeset(&mut mcp, apply).await?.status, WorkspaceChangeSetStatus::Applied);
    assert!(fixture.path().join("app/about/page.tsx").is_file());

    // 重新索引后 /about 作为 Route 可解析（E0 消歧规则：小写 stem page.tsx → Route）.
    let resolve_about = mcp
        .send_workspace_source_resolve_request(WorkspaceSourceResolveParams {
            project_id: "e1-next".to_owned(),
            kind: WorkspaceSourceQueryKind::RoutePath,
            value: "/about".to_owned(),
            limit: None,
        })
        .await?;
    let message = timeout(DEFAULT_TIMEOUT, mcp.read_stream_until_response_message(RequestId::Integer(resolve_about))).await??;
    let resolved: WorkspaceSourceResolveResponse = to_response(message)?;
    assert_eq!(resolved.matches.len(), 1, "new /about route resolves after apply");
    assert_eq!(resolved.matches[0].artifact.kind, ody_app_server_protocol::WorkspaceSourceKind::Route);
    Ok(())
}

#[tokio::test]
async fn e1_archetype_vue_refactor_component_and_restore() -> Result<()> {
    let ody_home = TempDir::new()?;
    let fixture = vue_fixture()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    let bind_id = mcp
        .send_workspace_project_bind_request(bind_params("e1-vue", vec![fixture.path().to_path_buf()]))
        .await?;
    read_project(&mut mcp, bind_id).await?;

    // Vue SFC：file-level ref（symbol = 文件名 stem，range = None）.
    let hello_base = file_hash_via_index(&mut mcp, "e1-vue", "src/components/HelloWorld.vue").await?;
    let resolve = mcp
        .send_workspace_source_resolve_request(WorkspaceSourceResolveParams {
            project_id: "e1-vue".to_owned(),
            kind: WorkspaceSourceQueryKind::Name,
            value: "HelloWorld".to_owned(),
            limit: None,
        })
        .await?;
    let message = timeout(DEFAULT_TIMEOUT, mcp.read_stream_until_response_message(RequestId::Integer(resolve))).await??;
    let resolved: WorkspaceSourceResolveResponse = to_response(message)?;
    assert_eq!(resolved.matches.len(), 1);
    assert!(resolved.matches[0].source_ref.range.is_none(), "SFC stays file-level");

    // 重构：template 加 class、script 加 props.
    let new_hello = "<template><p class=\"greeting\">hello</p></template>\n<script>\nexport default defineComponent({ name: 'HelloWorld', props: { label: String } });\n</script>\n";
    let create = mcp
        .send_workspace_source_changeset_create_request(WorkspaceChangeSetCreateParams {
            project_id: "e1-vue".to_owned(),
            title: "refactor HelloWorld".to_owned(),
            changes: vec![WorkspaceFileChange {
                root_index: 0,
                path: "src/components/HelloWorld.vue".to_owned(),
                kind: WorkspaceFileChangeKind::Update,
                base_hash: Some(hello_base.clone()),
                content: Some(new_hello.to_owned()),
            }],
            idempotency_key: "e1-vue-refactor".to_owned(),
        })
        .await?;
    let cs = read_changeset(&mut mcp, create).await?;
    let apply = mcp
        .send_workspace_source_changeset_apply_request(WorkspaceChangeSetApplyParams { changeset_id: cs.id.clone() })
        .await?;
    assert_eq!(read_changeset(&mut mcp, apply).await?.status, WorkspaceChangeSetStatus::Applied);
    assert_eq!(fs::read_to_string(fixture.path().join("src/components/HelloWorld.vue"))?, new_hello);

    // 原工程验证（node 门控）.
    if node_available() {
        let validate_id = mcp
            .send_workspace_source_validate_request(WorkspaceSourceValidateParams {
                project_id: "e1-vue".to_owned(),
                changeset_id: Some(cs.id.clone()),
                checks: vec![WorkspaceValidationCheck {
                    kind: WorkspaceValidationKind::Build,
                    script: "build".to_owned(),
                }],
                timeout_ms: None,
            })
            .await?;
        let report = read_validate(&mut mcp, validate_id).await?;
        assert_eq!(report.report.overall, WorkspaceValidationOverall::Succeeded);
    }

    // 恢复.
    let restore = mcp
        .send_workspace_source_changeset_restore_request(WorkspaceChangeSetRestoreParams { changeset_id: cs.id })
        .await?;
    assert_eq!(read_changeset(&mut mcp, restore).await?.status, WorkspaceChangeSetStatus::Restored);
    assert_file_hash_via_index(&mut mcp, "e1-vue", "src/components/HelloWorld.vue", &hello_base).await?;
    Ok(())
}

#[tokio::test]
async fn e1_archetype_fullstack_two_roots_modify_page_and_backend() -> Result<()> {
    let ody_home = TempDir::new()?;
    let (_dir, frontend, backend) = fullstack_fixture()?;
    let outside = snapshot_tree(_dir.path())?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    // 双根绑定：frontend = roots[0]，backend = roots[1]（均为 canonicalized sibling）.
    let bind_id = mcp
        .send_workspace_project_bind_request(bind_params(
            "e1-fullstack",
            vec![frontend.to_path_buf(), backend.to_path_buf()],
        ))
        .await?;
    let project = read_project(&mut mcp, bind_id).await?;
    assert_eq!(project.roots.len(), 2);

    let fe_base = file_hash_via_index(&mut mcp, "e1-fullstack", "src/pages/HomePage.tsx").await?;
    let be_base = {
        // index 的 file_path 是 root 相对路径；两个根都可能有 src/...，
        // 用 artifact 的 root_path 区分 backend 根的 ref.
        let index_id = mcp
            .send_workspace_source_index_request(WorkspaceSourceIndexParams { project_id: "e1-fullstack".to_owned() })
            .await?;
        let message = timeout(DEFAULT_TIMEOUT, mcp.read_stream_until_response_message(RequestId::Integer(index_id))).await??;
        let indexed: WorkspaceSourceIndexResponse = to_response(message)?;
        let backend_root = project.roots[1].path.clone();
        indexed
            .index
            .refs
            .iter()
            .find(|r| r.root_path == backend_root && r.file_path == "src/routes/handler.js")
            .expect("backend ref")
            .file_hash
            .clone()
    };

    // 一个 changeset 跨两个根：改前端页面 + 改后端 handler.
    let create = mcp
        .send_workspace_source_changeset_create_request(WorkspaceChangeSetCreateParams {
            project_id: "e1-fullstack".to_owned(),
            title: "frontend page + backend handler".to_owned(),
            changes: vec![
                WorkspaceFileChange {
                    root_index: 0,
                    path: "src/pages/HomePage.tsx".to_owned(),
                    kind: WorkspaceFileChangeKind::Update,
                    base_hash: Some(fe_base.clone()),
                    content: Some("export default function HomePage() {\n  return <main>frontend home v2</main>;\n}\n".to_owned()),
                },
                WorkspaceFileChange {
                    root_index: 1,
                    path: "src/routes/handler.js".to_owned(),
                    kind: WorkspaceFileChangeKind::Update,
                    base_hash: Some(be_base.clone()),
                    content: Some("module.exports = function handler(req) {\n  return { status: 201 };\n};\n".to_owned()),
                },
            ],
            idempotency_key: "e1-fullstack-cross-root".to_owned(),
        })
        .await?;
    let cs = read_changeset(&mut mcp, create).await?;
    assert_eq!(cs.changes.len(), 2);
    let apply = mcp
        .send_workspace_source_changeset_apply_request(WorkspaceChangeSetApplyParams { changeset_id: cs.id.clone() })
        .await?;
    let applied = read_changeset(&mut mcp, apply).await?;
    assert_eq!(applied.status, WorkspaceChangeSetStatus::Applied);
    assert!(matches!(applied.checkpoint, WorkspaceChangeSetCheckpoint::Git { .. }));
    assert!(fs::read_to_string(frontend.join("src/pages/HomePage.tsx"))?.contains("v2"));
    assert!(fs::read_to_string(backend.join("src/routes/handler.js"))?.contains("201"));

    // 后端验证（node 门控）.
    if node_available() {
        // E1 验证在 primary root 执行：frontend 的 build。backend test 属 E3 跨根聚合.
        let validate_id = mcp
            .send_workspace_source_validate_request(WorkspaceSourceValidateParams {
                project_id: "e1-fullstack".to_owned(),
                changeset_id: Some(cs.id.clone()),
                checks: vec![WorkspaceValidationCheck {
                    kind: WorkspaceValidationKind::Build,
                    script: "build".to_owned(),
                }],
                timeout_ms: None,
            })
            .await?;
        let report = read_validate(&mut mcp, validate_id).await?;
        assert_eq!(report.report.overall, WorkspaceValidationOverall::Succeeded);
    }

    // 恢复双根.
    let restore = mcp
        .send_workspace_source_changeset_restore_request(WorkspaceChangeSetRestoreParams { changeset_id: cs.id })
        .await?;
    assert_eq!(read_changeset(&mut mcp, restore).await?.status, WorkspaceChangeSetStatus::Restored);
    assert_file_hash_via_index(&mut mcp, "e1-fullstack", "src/pages/HomePage.tsx", &fe_base).await?;
    // backend hash 校验：再取一次 index 按 root_path 过滤比对.
    let index_id = mcp
        .send_workspace_source_index_request(WorkspaceSourceIndexParams { project_id: "e1-fullstack".to_owned() })
        .await?;
    let message = timeout(DEFAULT_TIMEOUT, mcp.read_stream_until_response_message(RequestId::Integer(index_id))).await??;
    let indexed: WorkspaceSourceIndexResponse = to_response(message)?;
    let backend_root = project.roots[1].path.clone();
    let be_now = indexed
        .index
        .refs
        .iter()
        .find(|r| r.root_path == backend_root && r.file_path == "src/routes/handler.js")
        .expect("backend ref")
        .file_hash
        .clone();
    assert_eq!(be_now, be_base);

    // 全生命周期结束：工程树与开始前完全一致（含 baseline .git 之外无残留）.
    let mut after = snapshot_tree(_dir.path())?;
    // 非 git 根的 checkpoint baseline 是允许的持久副作用（ADR 决策 4）；
    // 从快照中剔除 .git 后对比.
    after.retain(|path, _| !path.components().any(|c| c.as_os_str() == ".git"));
    let mut before_no_git = outside;
    before_no_git.retain(|path, _| !path.components().any(|c| c.as_os_str() == ".git"));
    assert_eq!(before_no_git, after, "restore leaves the workspace exactly as before (modulo checkpoint .git)");
    Ok(())
}

#[tokio::test]
async fn e1_user_can_reject_changeset_and_nothing_is_written() -> Result<()> {
    let ody_home = TempDir::new()?;
    let fixture = react_fixture()?;
    let before = snapshot_tree(fixture.path())?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    let bind_id = mcp
        .send_workspace_project_bind_request(bind_params("e1-reject", vec![fixture.path().to_path_buf()]))
        .await?;
    read_project(&mut mcp, bind_id).await?;

    let base_hash = file_hash_via_index(&mut mcp, "e1-reject", "src/pages/HomePage.tsx").await?;
    let create = mcp
        .send_workspace_source_changeset_create_request(WorkspaceChangeSetCreateParams {
            project_id: "e1-reject".to_owned(),
            title: "will be rejected".to_owned(),
            changes: vec![WorkspaceFileChange {
                root_index: 0,
                path: "src/pages/HomePage.tsx".to_owned(),
                kind: WorkspaceFileChangeKind::Update,
                base_hash: Some(base_hash),
                content: Some("export default function HomePage() { return null; }\n".to_owned()),
            }],
            idempotency_key: "e1-reject-1".to_owned(),
        })
        .await?;
    let cs = read_changeset(&mut mcp, create).await?;
    let reject = mcp
        .send_workspace_source_changeset_reject_request(WorkspaceChangeSetRejectParams { changeset_id: cs.id.clone() })
        .await?;
    assert_eq!(read_changeset(&mut mcp, reject).await?.status, WorkspaceChangeSetStatus::Rejected);

    let apply_attempt = mcp
        .send_workspace_source_changeset_apply_request(WorkspaceChangeSetApplyParams { changeset_id: cs.id })
        .await?;
    let message = read_changeset_error(&mut mcp, apply_attempt).await?;
    assert!(message.contains("Rejected"), "{message}");
    assert_eq!(before, snapshot_tree(fixture.path())?, "reject + failed apply wrote nothing");
    Ok(())
}

#[tokio::test]
async fn e1_external_edit_blocks_apply_and_leaves_sibling_files_untouched() -> Result<()> {
    let ody_home = TempDir::new()?;
    let fixture = react_fixture()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    let bind_id = mcp
        .send_workspace_project_bind_request(bind_params("e1-conflict", vec![fixture.path().to_path_buf()]))
        .await?;
    read_project(&mut mcp, bind_id).await?;

    let home_base = file_hash_via_index(&mut mcp, "e1-conflict", "src/pages/HomePage.tsx").await?;
    let button_base = file_hash_via_index(&mut mcp, "e1-conflict", "src/components/Button.tsx").await?;
    let create = mcp
        .send_workspace_source_changeset_create_request(WorkspaceChangeSetCreateParams {
            project_id: "e1-conflict".to_owned(),
            title: "two files".to_owned(),
            changes: vec![
                WorkspaceFileChange {
                    root_index: 0,
                    path: "src/pages/HomePage.tsx".to_owned(),
                    kind: WorkspaceFileChangeKind::Update,
                    base_hash: Some(home_base),
                    content: Some("export default function HomePage() { return <main>x</main>; }\n".to_owned()),
                },
                WorkspaceFileChange {
                    root_index: 0,
                    path: "src/components/Button.tsx".to_owned(),
                    kind: WorkspaceFileChangeKind::Update,
                    base_hash: Some(button_base.clone()),
                    content: Some("export function Button() { return <button>x</button>; }\nexport default Button;\n".to_owned()),
                },
            ],
            idempotency_key: "e1-conflict-1".to_owned(),
        })
        .await?;
    let cs = read_changeset(&mut mcp, create).await?;

    // 外部编辑（模拟 IDE 用户）只动 HomePage.tsx.
    let external = "export default function HomePage() {\n  return <main>edited externally</main>;\n}\n";
    fs::write(fixture.path().join("src/pages/HomePage.tsx"), external)?;

    let apply_attempt = mcp
        .send_workspace_source_changeset_apply_request(WorkspaceChangeSetApplyParams { changeset_id: cs.id.clone() })
        .await?;
    let message = read_changeset_error(&mut mcp, apply_attempt).await?;
    assert!(message.contains("changed on disk"), "{message}");
    assert!(message.contains("re-index and recreate"), "diagnosable recovery hint");

    // 冲突文件保持外部编辑内容；同 changeset 的另一个文件未被写入.
    assert_eq!(fs::read_to_string(fixture.path().join("src/pages/HomePage.tsx"))?, external);
    assert_file_hash_via_index(&mut mcp, "e1-conflict", "src/components/Button.tsx", &button_base).await?;
    Ok(())
}

#[tokio::test]
async fn e1_full_lifecycle_never_writes_outside_bound_roots() -> Result<()> {
    let ody_home = TempDir::new()?;
    let fixture = react_fixture()?;
    // sibling 独立 TempDir，与 root 并列探测逃逸写入.
    let sibling = TempDir::new()?;
    fs::write(sibling.path().join("keep.txt"), "untouched")?;
    let fixture_before = snapshot_tree(fixture.path())?;
    let sibling_before = snapshot_tree(sibling.path())?;

    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    let bind_id = mcp
        .send_workspace_project_bind_request(bind_params("e1-boundary", vec![fixture.path().to_path_buf()]))
        .await?;
    read_project(&mut mcp, bind_id).await?;

    let base_hash = file_hash_via_index(&mut mcp, "e1-boundary", "src/pages/HomePage.tsx").await?;
    let create = mcp
        .send_workspace_source_changeset_create_request(WorkspaceChangeSetCreateParams {
            project_id: "e1-boundary".to_owned(),
            title: "boundary probe".to_owned(),
            changes: vec![WorkspaceFileChange {
                root_index: 0,
                path: "src/pages/HomePage.tsx".to_owned(),
                kind: WorkspaceFileChangeKind::Update,
                base_hash: Some(base_hash.clone()),
                content: Some("export default function HomePage() { return <main>b</main>; }\n".to_owned()),
            }],
            idempotency_key: "e1-boundary-1".to_owned(),
        })
        .await?;
    let cs = read_changeset(&mut mcp, create).await?;
    let apply = mcp
        .send_workspace_source_changeset_apply_request(WorkspaceChangeSetApplyParams { changeset_id: cs.id.clone() })
        .await?;
    assert_eq!(read_changeset(&mut mcp, apply).await?.status, WorkspaceChangeSetStatus::Applied);
    let restore = mcp
        .send_workspace_source_changeset_restore_request(WorkspaceChangeSetRestoreParams { changeset_id: cs.id })
        .await?;
    assert_eq!(read_changeset(&mut mcp, restore).await?.status, WorkspaceChangeSetStatus::Restored);

    assert_file_hash_via_index(&mut mcp, "e1-boundary", "src/pages/HomePage.tsx", &base_hash).await?;
    let mut fixture_after = snapshot_tree(fixture.path())?;
    fixture_after.retain(|path, _| !path.components().any(|c| c.as_os_str() == ".git"));
    let mut fixture_before_no_git = fixture_before;
    fixture_before_no_git.retain(|path, _| !path.components().any(|c| c.as_os_str() == ".git"));
    assert_eq!(fixture_before_no_git, fixture_after);
    assert_eq!(sibling_before, snapshot_tree(sibling.path())?, "nothing written outside the bound root");
    Ok(())
}

#[tokio::test]
async fn e1_index_and_resolve_read_only_across_all_archetypes() -> Result<()> {
    for (label, fixture) in [
        ("react", react_fixture()?),
        ("next", next_fixture()?),
        ("vue", vue_fixture()?),
    ] {
        let before = snapshot_tree(fixture.path())?;
        let ody_home = TempDir::new()?;
        let mut mcp = TestAppServer::new(ody_home.path()).await?;
        init_experimental(&mut mcp).await?;
        let bind_id = mcp
            .send_workspace_project_bind_request(bind_params(
                &format!("e1-ro-{label}"),
                vec![fixture.path().to_path_buf()],
            ))
            .await?;
        read_project(&mut mcp, bind_id).await?;
        let index_id = mcp
            .send_workspace_source_index_request(WorkspaceSourceIndexParams {
                project_id: format!("e1-ro-{label}"),
            })
            .await?;
        let message = timeout(DEFAULT_TIMEOUT, mcp.read_stream_until_response_message(RequestId::Integer(index_id))).await??;
        let _: WorkspaceSourceIndexResponse = to_response(message)?;
        let diff_id = mcp
            .send_workspace_source_diff_request(WorkspaceSourceDiffParams {
                project_id: format!("e1-ro-{label}"),
            })
            .await?;
        let message = timeout(DEFAULT_TIMEOUT, mcp.read_stream_until_response_message(RequestId::Integer(diff_id))).await??;
        let _: WorkspaceSourceDiffResponse = to_response(message)?;
        assert_eq!(before, snapshot_tree(fixture.path())?, "{label}: index/diff are read-only");
    }
    Ok(())
}
