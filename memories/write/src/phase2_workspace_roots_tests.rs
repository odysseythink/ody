use super::agent;
use crate::memory_root;
use core_test_support::responses::start_mock_server;
use core_test_support::test_ody::test_ody;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use tempfile::TempDir;

#[tokio::test]
async fn consolidation_rebinds_workspace_roots_to_memory_root() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let home = Arc::new(TempDir::new()?);
    let test = test_ody().with_home(home).build(&server).await?;
    let provider = ody_model_provider::create_model_provider(test.config.model_provider.clone());

    let parent_permission_profile = test.config.permissions.permission_profile().clone();
    let agent_config =
        agent::get_config(&test.config, parent_permission_profile, provider.as_ref())
            .expect("agent config should be created");
    let root = memory_root(&test.config.ody_home);

    assert_eq!(agent_config.cwd, root);
    assert_eq!(agent_config.workspace_roots, vec![root]);
    assert_eq!(
        agent_config.permissions.network_sandbox_policy(),
        ody_protocol::permissions::NetworkSandboxPolicy::Restricted
    );

    test.ody.shutdown_and_wait().await?;
    Ok(())
}
