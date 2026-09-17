use super::agent;
use core_test_support::responses::start_mock_server;
use core_test_support::test_ody::test_ody;
use ody_model_provider::create_model_provider;
use ody_protocol::models::ManagedFileSystemPermissions;
use ody_protocol::models::PermissionProfile;
use ody_protocol::permissions::NetworkSandboxPolicy;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use tempfile::TempDir;

#[tokio::test]
async fn consolidation_uses_canonical_parent_enforcement() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let home = Arc::new(TempDir::new()?);
    let test = test_ody().with_home(home).build(&server).await?;
    let provider = create_model_provider(test.config.model_provider.clone());

    let root = crate::memory_root(&test.config.ody_home);
    let managed_worker_policy = ody_protocol::protocol::SandboxPolicy::WorkspaceWrite {
        writable_roots: vec![root.clone()],
        network_access: false,
        exclude_tmpdir_env_var: true,
        exclude_slash_tmp: true,
    };

    for (parent_permission_profile, expected_permission_profile) in [
        (PermissionProfile::Disabled, PermissionProfile::Disabled),
        (
            PermissionProfile::External {
                network: NetworkSandboxPolicy::Restricted,
            },
            PermissionProfile::External {
                network: NetworkSandboxPolicy::Restricted,
            },
        ),
        (
            PermissionProfile::Managed {
                file_system: ManagedFileSystemPermissions::Unrestricted,
                network: NetworkSandboxPolicy::Enabled,
            },
            PermissionProfile::from_legacy_sandbox_policy_for_cwd(
                &managed_worker_policy,
                root.as_path(),
            ),
        ),
    ] {
        let agent_config =
            agent::get_config(&test.config, parent_permission_profile, provider.as_ref())
                .expect("agent config should be created");

        assert_eq!(
            agent_config.permissions.permission_profile(),
            &expected_permission_profile
        );
    }

    test.ody.shutdown_and_wait().await?;
    Ok(())
}
