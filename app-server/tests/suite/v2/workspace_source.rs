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
use ody_app_server_protocol::WorkspaceProjectBindParams;
use ody_app_server_protocol::WorkspaceProjectRef;
use ody_app_server_protocol::WorkspaceSourceIndexParams;
use ody_app_server_protocol::WorkspaceSourceIndexResponse;
use ody_app_server_protocol::WorkspaceSourceQueryKind;
use ody_app_server_protocol::WorkspaceSourceResolveParams;
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
                stack.push(path);
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
