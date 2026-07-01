use std::sync::Arc;

use ody_config::McpServerTransportConfig;
use ody_core::McpManager;
use ody_core::config::Config;
use ody_core::config::ConfigBuilder;
use ody_core_plugins::PluginsManager;
use ody_extension_api::ExtensionRegistryBuilder;
use ody_extension_api::McpServerContribution;
use ody_extension_api::McpServerContributionContext;
use ody_extension_api::McpServerContributor;
use ody_login::OdyAuth;
use ody_mcp::ODY_APPS_MCP_SERVER_NAME;
use pretty_assertions::assert_eq;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[tokio::test]
async fn contributes_hosted_plugin_runtime_without_an_executor() -> TestResult {
    let ody_home = tempfile::tempdir()?;
    let config = ConfigBuilder::default()
        .ody_home(ody_home.path().to_path_buf())
        .fallback_cwd(Some(ody_home.path().to_path_buf()))
        .cli_overrides(vec![
            ("features.apps".to_string(), true.into()),
            ("chatgpt_base_url".to_string(), "https://chatgpt.com".into()),
        ])
        .build()
        .await?;
    let auth = OdyAuth::create_dummy_chatgpt_auth_for_testing();
    let manager = installed_manager(&config);

    let servers = manager.effective_servers(&config, Some(&auth)).await;
    let server = servers
        .get(ODY_APPS_MCP_SERVER_NAME)
        .and_then(|server| server.configured_config())
        .ok_or("hosted plugin runtime should be contributed as a configured server")?;
    let McpServerTransportConfig::StreamableHttp { url, .. } = &server.transport else {
        panic!("hosted plugin runtime should use streamable HTTP");
    };
    assert_eq!(url, "https://chatgpt.com/backend-api/ps/mcp");

    Ok(())
}

#[tokio::test]
async fn runtime_overlay_preserves_disabled_server() -> TestResult {
    let ody_home = tempfile::tempdir()?;
    let config = ConfigBuilder::default()
        .ody_home(ody_home.path().to_path_buf())
        .fallback_cwd(Some(ody_home.path().to_path_buf()))
        .cli_overrides(vec![
            ("features.apps".to_string(), true.into()),
            (
                "mcp_servers.ody_apps.url".to_string(),
                "https://example.com/mcp".into(),
            ),
            ("mcp_servers.ody_apps.enabled".to_string(), false.into()),
        ])
        .build()
        .await?;
    let auth = OdyAuth::create_dummy_chatgpt_auth_for_testing();
    let manager = installed_manager(&config);

    let servers = manager.effective_servers(&config, Some(&auth)).await;
    let server = servers
        .get(ODY_APPS_MCP_SERVER_NAME)
        .ok_or("hosted plugin runtime should remain configured")?;

    assert!(!server.enabled());
    Ok(())
}

#[tokio::test]
async fn legacy_fallback_overwrites_reserved_config_without_an_extension() -> TestResult {
    let ody_home = tempfile::tempdir()?;
    let config = ConfigBuilder::default()
        .ody_home(ody_home.path().to_path_buf())
        .fallback_cwd(Some(ody_home.path().to_path_buf()))
        .cli_overrides(vec![
            ("features.apps".to_string(), true.into()),
            (
                "mcp_servers.ody_apps.url".to_string(),
                "https://example.com/mcp".into(),
            ),
        ])
        .build()
        .await?;
    let auth = OdyAuth::create_dummy_chatgpt_auth_for_testing();
    let manager = McpManager::new(Arc::new(PluginsManager::new(
        config.ody_home.to_path_buf(),
    )));

    let servers = manager.effective_servers(&config, Some(&auth)).await;
    let server = servers
        .get(ODY_APPS_MCP_SERVER_NAME)
        .and_then(|server| server.configured_config())
        .ok_or("legacy Apps MCP should be present")?;
    let McpServerTransportConfig::StreamableHttp { url, .. } = &server.transport else {
        panic!("legacy Apps MCP should use streamable HTTP");
    };
    assert_eq!(url, "https://chatgpt.com/backend-api/wham/apps");

    Ok(())
}

#[tokio::test]
async fn later_extension_can_remove_same_name_registration() -> TestResult {
    let ody_home = tempfile::tempdir()?;
    let config = ConfigBuilder::default()
        .ody_home(ody_home.path().to_path_buf())
        .fallback_cwd(Some(ody_home.path().to_path_buf()))
        .cli_overrides(vec![("features.apps".to_string(), true.into())])
        .build()
        .await?;
    let auth = OdyAuth::create_dummy_chatgpt_auth_for_testing();
    let mut builder = ExtensionRegistryBuilder::new();
    ody_mcp_extension::install(&mut builder);
    builder.mcp_server_contributor(Arc::new(RemoveOdyApps));
    let manager = McpManager::new_with_extensions(
        Arc::new(PluginsManager::new(config.ody_home.to_path_buf())),
        Arc::new(builder.build()),
    );

    let servers = manager.effective_servers(&config, Some(&auth)).await;

    assert!(!servers.contains_key(ODY_APPS_MCP_SERVER_NAME));
    Ok(())
}

#[tokio::test]
async fn hosted_apps_mcp_requires_chatgpt_auth() -> TestResult {
    let ody_home = tempfile::tempdir()?;
    let config = ConfigBuilder::default()
        .ody_home(ody_home.path().to_path_buf())
        .fallback_cwd(Some(ody_home.path().to_path_buf()))
        .cli_overrides(vec![("features.apps".to_string(), true.into())])
        .build()
        .await?;
    let auth = OdyAuth::from_api_key("test");
    let manager = installed_manager(&config);

    let servers = manager.effective_servers(&config, Some(&auth)).await;
    assert!(!servers.contains_key(ODY_APPS_MCP_SERVER_NAME));

    Ok(())
}

#[tokio::test]
async fn disabled_apps_remove_reserved_server_config_for_all_hosts() -> TestResult {
    let ody_home = tempfile::tempdir()?;
    let config = ConfigBuilder::default()
        .ody_home(ody_home.path().to_path_buf())
        .fallback_cwd(Some(ody_home.path().to_path_buf()))
        .cli_overrides(vec![
            ("features.apps".to_string(), false.into()),
            (
                "mcp_servers.ody_apps.url".to_string(),
                "https://example.com/mcp".into(),
            ),
        ])
        .build()
        .await?;
    let managers = [
        installed_manager(&config),
        McpManager::new(Arc::new(PluginsManager::new(
            config.ody_home.to_path_buf(),
        ))),
    ];
    for manager in managers {
        let servers = manager.runtime_servers(&config).await;
        assert!(!servers.contains_key(ODY_APPS_MCP_SERVER_NAME));
    }
    Ok(())
}

fn installed_manager(config: &Config) -> McpManager {
    let mut builder = ExtensionRegistryBuilder::new();
    ody_mcp_extension::install(&mut builder);
    McpManager::new_with_extensions(
        Arc::new(PluginsManager::new(config.ody_home.to_path_buf())),
        Arc::new(builder.build()),
    )
}

struct RemoveOdyApps;

impl McpServerContributor<Config> for RemoveOdyApps {
    fn id(&self) -> &'static str {
        "remove_ody_apps"
    }

    fn contribute<'a>(
        &'a self,
        _context: McpServerContributionContext<'a, Config>,
    ) -> ody_extension_api::ExtensionFuture<'a, Vec<McpServerContribution>> {
        Box::pin(async move {
            vec![McpServerContribution::Remove {
                name: ODY_APPS_MCP_SERVER_NAME.to_string(),
            }]
        })
    }
}
