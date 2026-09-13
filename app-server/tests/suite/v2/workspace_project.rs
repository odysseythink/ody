use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use app_test_support::DEFAULT_CLIENT_NAME;
use app_test_support::TestAppServer;
use app_test_support::to_response;
use ody_app_server_protocol::ClientInfo;
use ody_app_server_protocol::InitializeCapabilities;
use ody_app_server_protocol::JSONRPCMessage;
use ody_app_server_protocol::RequestId;
use ody_app_server_protocol::WorkspaceProjectBindParams;
use ody_app_server_protocol::WorkspaceProjectCloseParams;
use ody_app_server_protocol::WorkspaceProjectGetParams;
use ody_app_server_protocol::WorkspaceProjectListParams;
use ody_app_server_protocol::WorkspaceProjectListResponse;
use ody_app_server_protocol::WorkspaceProjectRef;
use ody_app_server_protocol::WorkspaceDiscovery;
use ody_app_server_protocol::WorkspaceProjectScanParams;
use ody_app_server_protocol::WorkspaceProjectScanResponse;
use ody_app_server_protocol::WorkspaceSourceKind;
use ody_utils_absolute_path::test_support::PathBufExt;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

fn default_client_info() -> ClientInfo {
    ClientInfo {
        name: DEFAULT_CLIENT_NAME.to_string(),
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
    let response = message;
    let envelope: ProjectEnvelope = to_response(response)?;
    Ok(envelope.project)
}

#[tokio::test]
async fn workspace_project_bind_get_list_close_lifecycle() -> Result<()> {
    let ody_home = TempDir::new()?;
    let project_dir = TempDir::new()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;

    let bind_id = mcp
        .send_workspace_project_bind_request(bind_params(
            "ws-1",
            vec![project_dir.path().to_path_buf()],
        ))
        .await?;
    let project = read_project(&mut mcp, bind_id).await?;
    assert_eq!(project.id, "ws-1");
    assert_eq!(project.schema_version, 1);
    assert_eq!(project.roots.len(), 1);
    assert_eq!(
        project.roots[0].path,
        project_dir.path().canonicalize()?.to_string_lossy()
    );
    assert_eq!(
        project.roots[0].role,
        ody_app_server_protocol::WorkspaceRootRole::Primary
    );
    assert_eq!(project.roots[0].auth_source, "user_selected");

    let get_id = mcp
        .send_workspace_project_get_request(WorkspaceProjectGetParams {
            project_id: "ws-1".to_owned(),
        })
        .await?;
    assert_eq!(read_project(&mut mcp, get_id).await?.id, "ws-1");

    let list_id = mcp
        .send_workspace_project_list_request(WorkspaceProjectListParams {})
        .await?;
    let message = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(list_id)),
    )
    .await??;
    let response = message;
    let listed: WorkspaceProjectListResponse = to_response(response)?;
    assert_eq!(listed.projects.len(), 1);
    assert_eq!(listed.projects[0].id, "ws-1");

    let close_id = mcp
        .send_workspace_project_close_request(WorkspaceProjectCloseParams {
            project_id: "ws-1".to_owned(),
        })
        .await?;
    assert_eq!(read_project(&mut mcp, close_id).await?.id, "ws-1");

    let get_after_close = mcp
        .send_workspace_project_get_request(WorkspaceProjectGetParams {
            project_id: "ws-1".to_owned(),
        })
        .await?;
    let error = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(get_after_close)),
    )
    .await??;
    assert_eq!(error.error.code, -32602);
    assert!(
        error
            .error
            .message
            .contains("unknown workspace project id: ws-1"),
        "unexpected error message: {}",
        error.error.message
    );
    Ok(())
}

#[tokio::test]
async fn workspace_project_bind_is_idempotent_per_key() -> Result<()> {
    let ody_home = TempDir::new()?;
    let project_dir = TempDir::new()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    let params = bind_params("ws-1", vec![project_dir.path().to_path_buf()]);

    let first = mcp.send_workspace_project_bind_request(params.clone()).await?;
    let project = read_project(&mut mcp, first).await?;
    let second = mcp.send_workspace_project_bind_request(params).await?;
    let retried = read_project(&mut mcp, second).await?;
    assert_eq!(project.id, retried.id);
    assert_eq!(project.created_at_ms, retried.created_at_ms);
    Ok(())
}

#[tokio::test]
async fn workspace_project_bind_rejects_key_reuse_with_different_project() -> Result<()> {
    let ody_home = TempDir::new()?;
    let project_dir = TempDir::new()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    let mut first_params = bind_params("ws-1", vec![project_dir.path().to_path_buf()]);
    first_params.idempotency_key = "shared-key".to_owned();
    let first = mcp.send_workspace_project_bind_request(first_params).await?;
    read_project(&mut mcp, first).await?;

    let mut conflict_params = bind_params("ws-2", vec![project_dir.path().to_path_buf()]);
    conflict_params.idempotency_key = "shared-key".to_owned();
    let conflict = mcp.send_workspace_project_bind_request(conflict_params).await?;
    let error = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(conflict)),
    )
    .await??;
    assert_eq!(error.error.code, -32602);
    assert!(
        error.error.message.contains("already used"),
        "{}",
        error.error.message
    );
    Ok(())
}

#[tokio::test]
async fn workspace_project_bind_rejects_missing_and_overlapping_roots() -> Result<()> {
    let ody_home = TempDir::new()?;
    let parent = TempDir::new()?;
    let child = parent.path().join("child");
    std::fs::create_dir_all(&child)?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;

    let missing = mcp
        .send_workspace_project_bind_request(bind_params(
            "ws-missing",
            vec![parent.path().join("does-not-exist")],
        ))
        .await?;
    let error = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(missing)),
    )
    .await??;
    assert_eq!(error.error.code, -32602);
    assert!(
        error.error.message.contains("does-not-exist"),
        "{}",
        error.error.message
    );

    let overlap = mcp
        .send_workspace_project_bind_request(bind_params(
            "ws-overlap",
            vec![parent.path().to_path_buf(), child.clone()],
        ))
        .await?;
    let error = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(overlap)),
    )
    .await??;
    assert_eq!(error.error.code, -32602);
    assert!(
        error.error.message.contains("overlap"),
        "{}",
        error.error.message
    );
    Ok(())
}

#[tokio::test]
async fn workspace_project_bind_rejects_ody_home_root() -> Result<()> {
    let ody_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    let request_id = mcp
        .send_workspace_project_bind_request(bind_params(
            "ws-ody-home",
            vec![ody_home.path().to_path_buf()],
        ))
        .await?;
    let error = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;
    assert_eq!(error.error.code, -32602);
    assert!(
        error
            .error
            .message
            .contains("application data directory"),
        "{}",
        error.error.message
    );
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn workspace_project_bind_rejects_filesystem_root() -> Result<()> {
    let ody_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    let request_id = mcp
        .send_workspace_project_bind_request(bind_params("ws-root", vec![PathBuf::from("/")]))
        .await?;
    let error = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;
    assert_eq!(error.error.code, -32602);
    assert!(
        error.error.message.contains("filesystem root"),
        "{}",
        error.error.message
    );
    Ok(())
}

#[tokio::test]
async fn workspace_project_requires_experimental_capability() -> Result<()> {
    let ody_home = TempDir::new()?;
    let project_dir = TempDir::new()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    let init = mcp
        .initialize_with_capabilities(
            default_client_info(),
            Some(InitializeCapabilities {
                experimental_api: false,
                request_attestation: false,
                opt_out_notification_methods: None,
                mcp_server_form_elicitation: false,
            }),
        )
        .await?;
    let JSONRPCMessage::Response(_) = init else {
        anyhow::bail!("expected initialize response, got {init:?}");
    };
    let request_id = mcp
        .send_workspace_project_bind_request(bind_params(
            "ws-1",
            vec![project_dir.path().to_path_buf()],
        ))
        .await?;
    let error = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;
    assert_eq!(error.error.code, -32600);
    assert_eq!(
        error.error.message,
        "workspace/project/v1 requires experimentalApi capability"
    );
    Ok(())
}

#[tokio::test]
async fn bind_restart_recovers_project_bindings() -> Result<()> {
    let ody_home = TempDir::new()?;
    let project_dir = TempDir::new()?;
    {
        let mut mcp = TestAppServer::new(ody_home.path()).await?;
        init_experimental(&mut mcp).await?;
        let request_id = mcp
            .send_workspace_project_bind_request(bind_params(
                "ws-1",
                vec![project_dir.path().to_path_buf()],
            ))
            .await?;
        read_project(&mut mcp, request_id).await?;
    }

    let mut restarted = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut restarted).await?;
    let list_id = restarted
        .send_workspace_project_list_request(WorkspaceProjectListParams {})
        .await?;
    let message = timeout(
        DEFAULT_TIMEOUT,
        restarted.read_stream_until_response_message(RequestId::Integer(list_id)),
    )
    .await??;
    let response = message;
    let listed: WorkspaceProjectListResponse = to_response(response)?;
    assert_eq!(listed.projects.len(), 1);
    assert_eq!(listed.projects[0].id, "ws-1");
    Ok(())
}

fn write_fixture(root: &std::path::Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().expect("parent dir")).expect("create parent");
    std::fs::write(path, contents).expect("write fixture file");
}

async fn scan_project(
    mcp: &mut TestAppServer,
    project_id: &str,
) -> Result<WorkspaceDiscovery> {
    let request_id = mcp
        .send_workspace_project_scan_request(WorkspaceProjectScanParams {
            project_id: project_id.to_owned(),
        })
        .await?;
    let message = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let response = message;
    let payload: WorkspaceProjectScanResponse = to_response(response)?;
    Ok(payload.discovery)
}

#[tokio::test]
async fn workspace_project_scan_discovers_bound_project() -> Result<()> {
    let ody_home = TempDir::new()?;
    let project_dir = TempDir::new()?;
    write_fixture(
        project_dir.path(),
        "package.json",
        r#"{
            "name": "storefront",
            "scripts": { "dev": "vite", "build": "vite build" },
            "dependencies": { "react": "^18.2.0" },
            "devDependencies": { "vite": "^5.0.0" }
        }"#,
    );
    write_fixture(project_dir.path(), "pnpm-lock.yaml", "lockfileVersion: 9\n");
    write_fixture(project_dir.path(), "src/pages/Home.tsx", "export {}");
    write_fixture(project_dir.path(), "src/components/Button.tsx", "export {}");

    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    let bind_id = mcp
        .send_workspace_project_bind_request(bind_params(
            "ws-1",
            vec![project_dir.path().to_path_buf()],
        ))
        .await?;
    read_project(&mut mcp, bind_id).await?;

    let discovery = scan_project(&mut mcp, "ws-1").await?;
    assert_eq!(discovery.project_id, "ws-1");
    assert!(!discovery.truncated);
    assert_eq!(discovery.roots.len(), 1);
    let root = &discovery.roots[0];
    assert_eq!(root.package_name.as_deref(), Some("storefront"));
    assert_eq!(root.package_manager.as_deref(), Some("pnpm"));
    let tech: Vec<&str> = root.tech_stack.iter().map(|t| t.id.as_str()).collect();
    assert!(tech.contains(&"react"));
    assert!(tech.contains(&"vite"));
    let pages: Vec<&str> = root
        .sources
        .iter()
        .filter(|s| s.kind == WorkspaceSourceKind::Page)
        .map(|s| s.path.as_str())
        .collect();
    assert_eq!(pages, vec!["src/pages/Home.tsx"]);
    Ok(())
}

#[tokio::test]
async fn workspace_project_scan_scans_all_roots() -> Result<()> {
    let ody_home = TempDir::new()?;
    let frontend = TempDir::new()?;
    let backend = TempDir::new()?;
    write_fixture(
        frontend.path(),
        "package.json",
        r#"{ "name": "web", "scripts": { "dev": "vite" },
             "dependencies": { "vue": "^3.4.0" } }"#,
    );
    write_fixture(
        frontend.path(),
        "src/views/Home.vue",
        "<template></template>",
    );
    write_fixture(
        backend.path(),
        "package.json",
        r#"{ "name": "api", "scripts": { "start": "node server.js" },
             "dependencies": { "express": "^4.19.0" } }"#,
    );
    write_fixture(backend.path(), "server.js", "console.log('ok')");

    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    let bind_id = mcp
        .send_workspace_project_bind_request(bind_params(
            "ws-fullstack",
            vec![frontend.path().to_path_buf(), backend.path().to_path_buf()],
        ))
        .await?;
    let project = read_project(&mut mcp, bind_id).await?;
    assert_eq!(
        project.roots[1].role,
        ody_app_server_protocol::WorkspaceRootRole::Secondary
    );

    let discovery = scan_project(&mut mcp, "ws-fullstack").await?;
    assert_eq!(discovery.roots.len(), 2);
    let frontend_root = &discovery.roots[0];
    let backend_root = &discovery.roots[1];
    assert_eq!(frontend_root.package_name.as_deref(), Some("web"));
    assert!(frontend_root
        .sources
        .iter()
        .any(|s| s.path == "src/views/Home.vue"));
    assert_eq!(backend_root.package_name.as_deref(), Some("api"));
    let backend_tech: Vec<&str> =
        backend_root.tech_stack.iter().map(|t| t.id.as_str()).collect();
    assert!(backend_tech.contains(&"express"));
    Ok(())
}

#[tokio::test]
async fn workspace_project_scan_reports_removed_root_without_failing() -> Result<()> {
    let ody_home = TempDir::new()?;
    let project_dir = TempDir::new()?;
    write_fixture(project_dir.path(), "package.json", r#"{ "name": "gone" }"#);
    let root_path = project_dir.path().to_path_buf();

    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    let bind_id = mcp
        .send_workspace_project_bind_request(bind_params("ws-1", vec![root_path.clone()]))
        .await?;
    read_project(&mut mcp, bind_id).await?;

    // The project moved or was deleted after binding.
    std::fs::remove_dir_all(&root_path)?;

    let discovery = scan_project(&mut mcp, "ws-1").await?;
    assert_eq!(discovery.roots.len(), 1);
    assert_eq!(discovery.roots[0].errors.len(), 1);
    assert!(
        discovery.roots[0]
            .errors[0]
            .contains(root_path.to_str().expect("utf8 path")),
        "{}",
        discovery.roots[0].errors[0]
    );
    Ok(())
}

#[tokio::test]
async fn workspace_project_scan_rejects_unknown_project() -> Result<()> {
    let ody_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    init_experimental(&mut mcp).await?;
    let request_id = mcp
        .send_workspace_project_scan_request(WorkspaceProjectScanParams {
            project_id: "nope".to_owned(),
        })
        .await?;
    let error = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;
    assert_eq!(error.error.code, -32602);
    assert!(
        error
            .error
            .message
            .contains("unknown workspace project id: nope"),
        "{}",
        error.error.message
    );
    Ok(())
}
