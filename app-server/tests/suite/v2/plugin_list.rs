use std::time::Duration;

use anyhow::Result;
use anyhow::bail;
use app_test_support::ApiKeyAuthFixture;
use app_test_support::TestAppServer;
use app_test_support::to_response;
use app_test_support::write_api_key_auth;
use ody_app_server_protocol::JSONRPCResponse;
use ody_app_server_protocol::PluginAuthPolicy;
use ody_app_server_protocol::PluginInstallPolicy;
use ody_app_server_protocol::PluginInstalledParams;
use ody_app_server_protocol::PluginInstalledResponse;
use ody_app_server_protocol::PluginListMarketplaceKind;
use ody_app_server_protocol::PluginListParams;
use ody_app_server_protocol::PluginListResponse;
use ody_app_server_protocol::PluginMarketplaceEntry;
use ody_app_server_protocol::PluginSource;
use ody_app_server_protocol::PluginSummary;
use ody_app_server_protocol::RequestId;
use ody_config::types::AuthCredentialsStoreMode;
use ody_core::config::set_project_trust_level;
use ody_protocol::config_types::TrustLevel;
use ody_utils_absolute_path::AbsolutePathBuf;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::time::timeout;
use wiremock::MockServer;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const TEST_CURATED_PLUGIN_SHA: &str = "0123456789abcdef0123456789abcdef01234567";
const ALTERNATE_MARKETPLACE_RELATIVE_PATH: &str = ".claude-plugin/marketplace.json";
const ALTERNATE_PLUGIN_MANIFEST_RELATIVE_PATH: &str = ".claude-plugin/plugin.json";

fn write_plugins_enabled_config(ody_home: &std::path::Path) -> std::io::Result<()> {
    std::fs::write(
        ody_home.join("config.toml"),
        r#"[features]
plugins = true
"#,
    )
}

fn write_plugins_enabled_config_with_base_url(
    ody_home: &std::path::Path,
    base_url: &str,
) -> std::io::Result<()> {
    std::fs::write(
        ody_home.join("config.toml"),
        format!(
            r#"legacy_base_url = "{base_url}"

[features]
plugins = true
"#,
        ),
    )
}

#[tokio::test]
async fn plugin_list_skips_invalid_marketplace_file_and_reports_error() -> Result<()> {
    let ody_home = TempDir::new()?;
    let repo_root = TempDir::new()?;
    std::fs::create_dir_all(repo_root.path().join(".git"))?;
    std::fs::create_dir_all(repo_root.path().join(".agents/plugins"))?;
    write_plugins_enabled_config(ody_home.path())?;
    let marketplace_path =
        AbsolutePathBuf::try_from(repo_root.path().join(".agents/plugins/marketplace.json"))?;
    std::fs::write(marketplace_path.as_path(), "{not json")?;

    let home = ody_home.path().to_string_lossy().into_owned();
    let mut mcp = TestAppServer::new_with_env(
        ody_home.path(),
        &[
            ("HOME", Some(home.as_str())),
            ("USERPROFILE", Some(home.as_str())),
        ],
    )
    .await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_list_request(PluginListParams {
            cwds: Some(vec![AbsolutePathBuf::try_from(repo_root.path())?]),
            marketplace_kinds: None,
        })
        .await?;

    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let response: PluginListResponse = to_response(response)?;

    assert!(
        response
            .marketplaces
            .iter()
            .all(|marketplace| { marketplace.path.as_ref() != Some(&marketplace_path) }),
        "invalid marketplace should be skipped"
    );
    assert_eq!(response.marketplace_load_errors.len(), 1);
    assert_eq!(
        response.marketplace_load_errors[0].marketplace_path,
        marketplace_path
    );
    assert!(
        response.marketplace_load_errors[0]
            .message
            .contains("invalid marketplace file"),
        "unexpected error: {:?}",
        response.marketplace_load_errors
    );
    Ok(())
}

#[tokio::test]
async fn plugin_installed_includes_installed_plugins_and_explicit_install_suggestions() -> Result<()>
{
    let ody_home = TempDir::new()?;
    write_odysseythink_curated_marketplace(
        ody_home.path(),
        &["linear", "computer-use", "not-mentioned"],
    )?;
    write_installed_plugin(&ody_home, "odysseythink-curated", "linear")?;
    std::fs::write(
        ody_home.path().join("config.toml"),
        r#"[features]
plugins = true

[plugins."linear@odysseythink-curated"]
enabled = true
"#,
    )?;

    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_installed_request(PluginInstalledParams {
            cwds: None,
            install_suggestion_plugin_names: Some(vec!["computer-use".to_string()]),
        })
        .await?;

    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let response: PluginInstalledResponse = to_response(response)?;

    assert_eq!(response.marketplaces.len(), 1);
    assert_eq!(response.marketplaces[0].name, "odysseythink-curated");
    assert_eq!(
        response.marketplaces[0]
            .plugins
            .iter()
            .map(|plugin| (plugin.id.clone(), plugin.installed, plugin.enabled))
            .collect::<Vec<_>>(),
        vec![
            ("linear@odysseythink-curated".to_string(), true, true),
            (
                "computer-use@odysseythink-curated".to_string(),
                false,
                false
            ),
        ]
    );
    assert_eq!(response.marketplace_load_errors, Vec::new());
    Ok(())
}

#[tokio::test]
async fn plugin_installed_ignores_local_cache_without_catalog() -> Result<()> {
    let ody_home = TempDir::new()?;
    write_installed_plugin(&ody_home, "odysseythink-curated", "linear")?;
    std::fs::write(
        ody_home.path().join("config.toml"),
        r#"[features]
plugins = true

[plugins."linear@odysseythink-curated"]
enabled = true
"#,
    )?;

    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_installed_request(PluginInstalledParams {
            cwds: None,
            install_suggestion_plugin_names: None,
        })
        .await?;

    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let response: PluginInstalledResponse = to_response(response)?;

    assert_eq!(response.marketplaces, Vec::new());
    assert_eq!(response.marketplace_load_errors, Vec::new());
    Ok(())
}

#[tokio::test]
async fn plugin_list_rejects_relative_cwds() -> Result<()> {
    let ody_home = TempDir::new()?;
    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_raw_request(
            "plugin/list",
            Some(serde_json::json!({
                "cwds": ["relative-root"],
            })),
        )
        .await?;

    let err = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_error_message(RequestId::Integer(request_id)),
    )
    .await??;

    assert_eq!(err.error.code, -32600);
    assert!(err.error.message.contains("Invalid request"));
    Ok(())
}

#[tokio::test]
async fn plugin_list_keeps_valid_marketplaces_when_another_marketplace_fails_to_load() -> Result<()>
{
    let ody_home = TempDir::new()?;
    let valid_repo_root = TempDir::new()?;
    let invalid_repo_root = TempDir::new()?;
    std::fs::create_dir_all(valid_repo_root.path().join(".git"))?;
    std::fs::create_dir_all(valid_repo_root.path().join(".agents/plugins"))?;
    std::fs::create_dir_all(
        valid_repo_root
            .path()
            .join("plugins/valid-plugin/.ody-plugin"),
    )?;
    std::fs::create_dir_all(invalid_repo_root.path().join(".git"))?;
    std::fs::create_dir_all(invalid_repo_root.path().join(".agents/plugins"))?;
    write_plugins_enabled_config(ody_home.path())?;

    let valid_marketplace_path = AbsolutePathBuf::try_from(
        valid_repo_root
            .path()
            .join(".agents/plugins/marketplace.json"),
    )?;
    let invalid_marketplace_path = AbsolutePathBuf::try_from(
        invalid_repo_root
            .path()
            .join(".agents/plugins/marketplace.json"),
    )?;
    let valid_plugin_path =
        AbsolutePathBuf::try_from(valid_repo_root.path().join("plugins/valid-plugin"))?;

    std::fs::write(
        valid_marketplace_path.as_path(),
        r#"{
  "name": "valid-marketplace",
  "plugins": [
    {
      "name": "valid-plugin",
      "source": {
        "source": "local",
        "path": "./plugins/valid-plugin"
      }
    }
  ]
}"#,
    )?;
    std::fs::write(
        valid_repo_root
            .path()
            .join("plugins/valid-plugin/.ody-plugin/plugin.json"),
        r#"{"name":"valid-plugin","keywords":["api-key","developer tools"]}"#,
    )?;
    std::fs::write(invalid_marketplace_path.as_path(), "{not json")?;

    let home = ody_home.path().to_string_lossy().into_owned();
    let mut mcp = TestAppServer::new_with_env(
        ody_home.path(),
        &[
            ("HOME", Some(home.as_str())),
            ("USERPROFILE", Some(home.as_str())),
        ],
    )
    .await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_list_request(PluginListParams {
            cwds: Some(vec![
                AbsolutePathBuf::try_from(valid_repo_root.path())?,
                AbsolutePathBuf::try_from(invalid_repo_root.path())?,
            ]),
            marketplace_kinds: None,
        })
        .await?;

    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let response: PluginListResponse = to_response(response)?;

    assert_eq!(
        response.marketplaces,
        vec![PluginMarketplaceEntry {
            name: "valid-marketplace".to_string(),
            path: Some(valid_marketplace_path),
            interface: None,
            plugins: vec![PluginSummary {
                id: "valid-plugin@valid-marketplace".to_string(),
                remote_plugin_id: None,
                local_version: None,
                name: "valid-plugin".to_string(),
                share_context: None,
                source: PluginSource::Local {
                    path: valid_plugin_path,
                },
                installed: false,
                enabled: false,
                install_policy: PluginInstallPolicy::Available,
                auth_policy: PluginAuthPolicy::OnInstall,
                availability: ody_app_server_protocol::PluginAvailability::Available,
                interface: None,
                keywords: vec!["api-key".to_string(), "developer tools".to_string()],
            }],
        }]
    );
    assert_eq!(response.marketplace_load_errors.len(), 1);
    assert_eq!(
        response.marketplace_load_errors[0].marketplace_path,
        invalid_marketplace_path
    );
    assert!(
        response.marketplace_load_errors[0]
            .message
            .contains("invalid marketplace file"),
        "unexpected error: {:?}",
        response.marketplace_load_errors
    );
    assert!(response.featured_plugin_ids.is_empty());
    Ok(())
}

#[tokio::test]
async fn plugin_list_returns_local_marketplaces() -> Result<()> {
    let ody_home = TempDir::new()?;
    let repo_root = TempDir::new()?;
    std::fs::create_dir_all(repo_root.path().join(".git"))?;
    std::fs::create_dir_all(repo_root.path().join(".agents/plugins"))?;
    write_plugins_enabled_config(ody_home.path())?;

    let marketplace_path =
        AbsolutePathBuf::try_from(repo_root.path().join(".agents/plugins/marketplace.json"))?;
    std::fs::write(
        marketplace_path.as_path(),
        r#"{
  "name": "ody-curated",
  "plugins": [
    {
      "name": "demo-plugin",
      "source": {
        "source": "local",
        "path": "./demo-plugin"
      }
    }
  ]
}"#,
    )?;

    let home = ody_home.path().to_string_lossy().into_owned();
    let mut mcp = TestAppServer::new_with_env(
        ody_home.path(),
        &[
            ("HOME", Some(home.as_str())),
            ("USERPROFILE", Some(home.as_str())),
        ],
    )
    .await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_list_request(PluginListParams {
            cwds: Some(vec![AbsolutePathBuf::try_from(repo_root.path())?]),
            marketplace_kinds: None,
        })
        .await?;

    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let response: PluginListResponse = to_response(response)?;

    assert_eq!(response.marketplaces.len(), 1);
    assert_eq!(response.marketplaces[0].name, "ody-curated");
    assert_eq!(response.marketplaces[0].path, Some(marketplace_path));
    assert!(response.marketplace_load_errors.is_empty());
    assert!(response.featured_plugin_ids.is_empty());
    Ok(())
}

#[tokio::test]
async fn plugin_list_returns_consistent_local_marketplaces_on_repeated_requests() -> Result<()> {
    let ody_home = TempDir::new()?;
    let repo_root = TempDir::new()?;
    std::fs::create_dir_all(repo_root.path().join(".git"))?;
    std::fs::create_dir_all(repo_root.path().join(".agents/plugins"))?;
    std::fs::create_dir_all(repo_root.path().join("demo-plugin/.ody-plugin"))?;
    write_plugins_enabled_config(ody_home.path())?;

    std::fs::write(
        repo_root.path().join(".agents/plugins/marketplace.json"),
        r#"{
  "name": "local-marketplace",
  "plugins": [
    {
      "name": "demo-plugin",
      "source": {
        "source": "local",
        "path": "./demo-plugin"
      }
    }
  ]
}"#,
    )?;
    std::fs::write(
        repo_root.path().join("demo-plugin/.ody-plugin/plugin.json"),
        r#"{"name":"demo-plugin"}"#,
    )?;

    let home = ody_home.path().to_string_lossy().into_owned();
    let mut mcp = TestAppServer::new_with_env(
        ody_home.path(),
        &[
            ("HOME", Some(home.as_str())),
            ("USERPROFILE", Some(home.as_str())),
        ],
    )
    .await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    for _ in 0..2 {
        let request_id = mcp
            .send_plugin_list_request(PluginListParams {
                cwds: Some(vec![AbsolutePathBuf::try_from(repo_root.path())?]),
                marketplace_kinds: None,
            })
            .await?;

        let response: JSONRPCResponse = timeout(
            DEFAULT_TIMEOUT,
            mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
        )
        .await??;
        let response: PluginListResponse = to_response(response)?;
        assert_eq!(response.marketplaces.len(), 1);
        assert_eq!(response.marketplaces[0].name, "local-marketplace");
    }

    Ok(())
}

#[tokio::test]
async fn plugin_list_uses_alternate_discoverable_manifest_and_keeps_undiscoverable_plugins()
-> Result<()> {
    let ody_home = TempDir::new()?;
    let repo_root = TempDir::new()?;
    let valid_plugin_root = repo_root.path().join("plugins/valid-plugin");
    std::fs::create_dir_all(repo_root.path().join(".git"))?;
    std::fs::create_dir_all(
        repo_root
            .path()
            .join(ALTERNATE_MARKETPLACE_RELATIVE_PATH)
            .parent()
            .unwrap(),
    )?;
    std::fs::create_dir_all(
        valid_plugin_root
            .join(ALTERNATE_PLUGIN_MANIFEST_RELATIVE_PATH)
            .parent()
            .unwrap(),
    )?;
    write_plugins_enabled_config(ody_home.path())?;

    let marketplace_path =
        AbsolutePathBuf::try_from(repo_root.path().join(ALTERNATE_MARKETPLACE_RELATIVE_PATH))?;
    let valid_plugin_path = AbsolutePathBuf::try_from(valid_plugin_root.clone())?;

    std::fs::write(
        marketplace_path.as_path(),
        r#"{
  "name": "alternate-marketplace",
  "plugins": [
    {
      "name": "valid-plugin",
      "source": "./plugins/valid-plugin"
    },
    {
      "name": "missing-plugin",
      "source": "./plugins/missing-plugin"
    }
  ]
}"#,
    )?;
    std::fs::write(
        valid_plugin_root.join(ALTERNATE_PLUGIN_MANIFEST_RELATIVE_PATH),
        r#"{
  "name": "valid-plugin",
  "interface": {
    "displayName": "Valid Plugin"
  }
}"#,
    )?;

    let home = ody_home.path().to_string_lossy().into_owned();
    let mut mcp = TestAppServer::new_with_env(
        ody_home.path(),
        &[
            ("HOME", Some(home.as_str())),
            ("USERPROFILE", Some(home.as_str())),
        ],
    )
    .await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_list_request(PluginListParams {
            cwds: Some(vec![AbsolutePathBuf::try_from(repo_root.path())?]),
            marketplace_kinds: None,
        })
        .await?;

    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let response: PluginListResponse = to_response(response)?;

    assert_eq!(
        response.marketplaces,
        vec![PluginMarketplaceEntry {
            name: "alternate-marketplace".to_string(),
            path: Some(marketplace_path),
            interface: None,
            plugins: vec![
                PluginSummary {
                    id: "valid-plugin@alternate-marketplace".to_string(),
                    remote_plugin_id: None,
                    local_version: None,
                    name: "valid-plugin".to_string(),
                    share_context: None,
                    source: PluginSource::Local {
                        path: valid_plugin_path,
                    },
                    installed: false,
                    enabled: false,
                    install_policy: PluginInstallPolicy::Available,
                    auth_policy: PluginAuthPolicy::OnInstall,
                    availability: ody_app_server_protocol::PluginAvailability::Available,
                    interface: Some(ody_app_server_protocol::PluginInterface {
                        display_name: Some("Valid Plugin".to_string()),
                        short_description: None,
                        long_description: None,
                        developer_name: None,
                        category: None,
                        capabilities: Vec::new(),
                        website_url: None,
                        privacy_policy_url: None,
                        terms_of_service_url: None,
                        default_prompt: None,
                        brand_color: None,
                        composer_icon: None,
                        composer_icon_url: None,
                        logo: None,
                        logo_dark: None,
                        logo_url: None,
                        logo_url_dark: None,
                        screenshots: Vec::new(),
                        screenshot_urls: Vec::new(),
                    }),
                    keywords: Vec::new(),
                },
                PluginSummary {
                    id: "missing-plugin@alternate-marketplace".to_string(),
                    remote_plugin_id: None,
                    local_version: None,
                    name: "missing-plugin".to_string(),
                    share_context: None,
                    source: PluginSource::Local {
                        path: AbsolutePathBuf::try_from(
                            repo_root.path().join("plugins/missing-plugin"),
                        )?,
                    },
                    installed: false,
                    enabled: false,
                    install_policy: PluginInstallPolicy::Available,
                    auth_policy: PluginAuthPolicy::OnInstall,
                    availability: ody_app_server_protocol::PluginAvailability::Available,
                    interface: None,
                    keywords: Vec::new(),
                },
            ],
        }]
    );
    assert!(response.marketplace_load_errors.is_empty());
    Ok(())
}

#[tokio::test]
async fn plugin_list_accepts_omitted_cwds() -> Result<()> {
    let ody_home = TempDir::new()?;
    std::fs::create_dir_all(ody_home.path().join(".agents/plugins"))?;
    write_plugins_enabled_config(ody_home.path())?;
    std::fs::write(
        ody_home.path().join(".agents/plugins/marketplace.json"),
        r#"{
  "name": "ody-curated",
  "plugins": [
    {
      "name": "home-plugin",
      "source": {
        "source": "local",
        "path": "./home-plugin"
      }
    }
  ]
}"#,
    )?;
    let home = ody_home.path().to_string_lossy().into_owned();
    let mut mcp = TestAppServer::new_with_env(
        ody_home.path(),
        &[
            ("HOME", Some(home.as_str())),
            ("USERPROFILE", Some(home.as_str())),
        ],
    )
    .await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_list_request(PluginListParams {
            cwds: None,
            marketplace_kinds: None,
        })
        .await?;

    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let _: PluginListResponse = to_response(response)?;
    Ok(())
}

#[tokio::test]
async fn plugin_list_includes_install_and_enabled_state_from_config() -> Result<()> {
    let ody_home = TempDir::new()?;
    let repo_root = TempDir::new()?;
    std::fs::create_dir_all(repo_root.path().join(".git"))?;
    std::fs::create_dir_all(repo_root.path().join(".agents/plugins"))?;
    write_installed_plugin(&ody_home, "ody-curated", "enabled-plugin")?;
    write_installed_plugin(&ody_home, "ody-curated", "disabled-plugin")?;
    std::fs::write(
        repo_root.path().join(".agents/plugins/marketplace.json"),
        r#"{
  "name": "ody-curated",
  "interface": {
    "displayName": "Legacy Official"
  },
  "plugins": [
    {
      "name": "enabled-plugin",
      "source": {
        "source": "local",
        "path": "./enabled-plugin"
      }
    },
    {
      "name": "disabled-plugin",
      "source": {
        "source": "local",
        "path": "./disabled-plugin"
      }
    },
    {
      "name": "uninstalled-plugin",
      "source": {
        "source": "local",
        "path": "./uninstalled-plugin"
      }
    }
  ]
}"#,
    )?;
    std::fs::write(
        ody_home.path().join("config.toml"),
        r#"[features]
plugins = true

[plugins."enabled-plugin@ody-curated"]
enabled = true

[plugins."disabled-plugin@ody-curated"]
enabled = false
"#,
    )?;

    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_list_request(PluginListParams {
            cwds: Some(vec![AbsolutePathBuf::try_from(repo_root.path())?]),
            marketplace_kinds: None,
        })
        .await?;

    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let response: PluginListResponse = to_response(response)?;

    let marketplace = response
        .marketplaces
        .into_iter()
        .find(|marketplace| {
            marketplace.path.as_ref()
                == Some(
                    &AbsolutePathBuf::try_from(
                        repo_root.path().join(".agents/plugins/marketplace.json"),
                    )
                    .expect("absolute marketplace path"),
                )
        })
        .expect("expected repo marketplace entry");

    assert_eq!(marketplace.name, "ody-curated");
    assert_eq!(
        marketplace
            .interface
            .as_ref()
            .and_then(|interface| interface.display_name.as_deref()),
        Some("Legacy Official")
    );
    assert_eq!(marketplace.plugins.len(), 3);
    assert_eq!(marketplace.plugins[0].id, "enabled-plugin@ody-curated");
    assert_eq!(marketplace.plugins[0].name, "enabled-plugin");
    assert_eq!(marketplace.plugins[0].installed, true);
    assert_eq!(marketplace.plugins[0].enabled, true);
    assert_eq!(
        marketplace.plugins[0].install_policy,
        PluginInstallPolicy::Available
    );
    assert_eq!(
        marketplace.plugins[0].auth_policy,
        PluginAuthPolicy::OnInstall
    );
    assert_eq!(marketplace.plugins[1].id, "disabled-plugin@ody-curated");
    assert_eq!(marketplace.plugins[1].name, "disabled-plugin");
    assert_eq!(marketplace.plugins[1].installed, true);
    assert_eq!(marketplace.plugins[1].enabled, false);
    assert_eq!(
        marketplace.plugins[1].install_policy,
        PluginInstallPolicy::Available
    );
    assert_eq!(
        marketplace.plugins[1].auth_policy,
        PluginAuthPolicy::OnInstall
    );
    assert_eq!(marketplace.plugins[2].id, "uninstalled-plugin@ody-curated");
    assert_eq!(marketplace.plugins[2].name, "uninstalled-plugin");
    assert_eq!(marketplace.plugins[2].installed, false);
    assert_eq!(marketplace.plugins[2].enabled, false);
    assert_eq!(
        marketplace.plugins[2].install_policy,
        PluginInstallPolicy::Available
    );
    assert_eq!(
        marketplace.plugins[2].auth_policy,
        PluginAuthPolicy::OnInstall
    );
    Ok(())
}

#[tokio::test]
async fn plugin_list_uses_home_config_for_enabled_state() -> Result<()> {
    let ody_home = TempDir::new()?;
    std::fs::create_dir_all(ody_home.path().join(".agents/plugins"))?;
    write_installed_plugin(&ody_home, "ody-curated", "shared-plugin")?;
    std::fs::write(
        ody_home.path().join(".agents/plugins/marketplace.json"),
        r#"{
  "name": "ody-curated",
  "plugins": [
    {
      "name": "shared-plugin",
      "source": {
        "source": "local",
        "path": "./shared-plugin"
      }
    }
  ]
}"#,
    )?;
    std::fs::write(
        ody_home.path().join("config.toml"),
        r#"[features]
plugins = true

[plugins."shared-plugin@ody-curated"]
enabled = true
"#,
    )?;

    let workspace_enabled = TempDir::new()?;
    std::fs::create_dir_all(workspace_enabled.path().join(".git"))?;
    std::fs::create_dir_all(workspace_enabled.path().join(".agents/plugins"))?;
    std::fs::write(
        workspace_enabled
            .path()
            .join(".agents/plugins/marketplace.json"),
        r#"{
  "name": "ody-curated",
  "plugins": [
    {
      "name": "shared-plugin",
      "source": {
        "source": "local",
        "path": "./shared-plugin"
      }
    }
  ]
}"#,
    )?;
    std::fs::create_dir_all(workspace_enabled.path().join(".ody"))?;
    std::fs::write(
        workspace_enabled.path().join(".ody/config.toml"),
        r#"[plugins."shared-plugin@ody-curated"]
enabled = false
"#,
    )?;
    set_project_trust_level(
        ody_home.path(),
        workspace_enabled.path(),
        TrustLevel::Trusted,
    )?;

    let workspace_default = TempDir::new()?;
    let home = ody_home.path().to_string_lossy().into_owned();
    let mut mcp = TestAppServer::new_with_env(
        ody_home.path(),
        &[
            ("HOME", Some(home.as_str())),
            ("USERPROFILE", Some(home.as_str())),
        ],
    )
    .await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_list_request(PluginListParams {
            cwds: Some(vec![
                AbsolutePathBuf::try_from(workspace_enabled.path())?,
                AbsolutePathBuf::try_from(workspace_default.path())?,
            ]),
            marketplace_kinds: None,
        })
        .await?;

    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let response: PluginListResponse = to_response(response)?;

    let shared_plugin = response
        .marketplaces
        .iter()
        .flat_map(|marketplace| marketplace.plugins.iter())
        .find(|plugin| plugin.name == "shared-plugin")
        .expect("expected shared-plugin entry");
    assert_eq!(shared_plugin.id, "shared-plugin@ody-curated");
    assert_eq!(shared_plugin.installed, true);
    assert_eq!(shared_plugin.enabled, true);
    Ok(())
}

#[tokio::test]
async fn plugin_list_returns_plugin_interface_with_absolute_asset_paths() -> Result<()> {
    let ody_home = TempDir::new()?;
    let repo_root = TempDir::new()?;
    let plugin_root = repo_root.path().join("plugins/demo-plugin");
    std::fs::create_dir_all(repo_root.path().join(".git"))?;
    std::fs::create_dir_all(repo_root.path().join(".agents/plugins"))?;
    std::fs::create_dir_all(plugin_root.join(".ody-plugin"))?;
    write_plugins_enabled_config(ody_home.path())?;
    std::fs::write(
        repo_root.path().join(".agents/plugins/marketplace.json"),
        r#"{
  "name": "ody-curated",
  "plugins": [
    {
      "name": "demo-plugin",
      "source": {
        "source": "local",
        "path": "./plugins/demo-plugin"
      },
      "policy": {
        "installation": "AVAILABLE",
        "authentication": "ON_INSTALL"
      },
      "category": "Design"
    }
  ]
}"#,
    )?;
    std::fs::write(
        plugin_root.join(".ody-plugin/plugin.json"),
        r##"{
  "name": "demo-plugin",
  "interface": {
    "displayName": "Plugin Display Name",
    "shortDescription": "Short description for subtitle",
    "longDescription": "Long description for details page",
    "developerName": "OpenAI",
    "category": "Productivity",
    "capabilities": ["Interactive", "Write"],
    "websiteURL": "https://odysseythink.com/",
    "privacyPolicyURL": "https://odysseythink.com/policies/row-privacy-policy/",
    "termsOfServiceURL": "https://odysseythink.com/policies/row-terms-of-use/",
    "defaultPrompt": [
      "Starter prompt for trying a plugin",
      "Find my next action"
    ],
    "brandColor": "#3B82F6",
    "composerIcon": "./assets/icon.png",
    "logo": "./assets/logo.png",
    "screenshots": ["./assets/screenshot1.png", "./assets/screenshot2.png"]
  }
}"##,
    )?;

    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_list_request(PluginListParams {
            cwds: Some(vec![AbsolutePathBuf::try_from(repo_root.path())?]),
            marketplace_kinds: None,
        })
        .await?;

    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let response: PluginListResponse = to_response(response)?;

    let plugin = response
        .marketplaces
        .iter()
        .flat_map(|marketplace| marketplace.plugins.iter())
        .find(|plugin| plugin.name == "demo-plugin")
        .expect("expected demo-plugin entry");

    assert_eq!(plugin.id, "demo-plugin@ody-curated");
    assert_eq!(plugin.installed, false);
    assert_eq!(plugin.enabled, false);
    assert_eq!(plugin.install_policy, PluginInstallPolicy::Available);
    assert_eq!(plugin.auth_policy, PluginAuthPolicy::OnInstall);
    let interface = plugin
        .interface
        .as_ref()
        .expect("expected plugin interface");
    assert_eq!(
        interface.display_name.as_deref(),
        Some("Plugin Display Name")
    );
    assert_eq!(interface.category.as_deref(), Some("Design"));
    assert_eq!(
        interface.website_url.as_deref(),
        Some("https://odysseythink.com/")
    );
    assert_eq!(
        interface.privacy_policy_url.as_deref(),
        Some("https://odysseythink.com/policies/row-privacy-policy/")
    );
    assert_eq!(
        interface.terms_of_service_url.as_deref(),
        Some("https://odysseythink.com/policies/row-terms-of-use/")
    );
    assert_eq!(
        interface.default_prompt,
        Some(vec![
            "Starter prompt for trying a plugin".to_string(),
            "Find my next action".to_string()
        ])
    );
    assert_eq!(
        interface.composer_icon,
        Some(AbsolutePathBuf::try_from(
            plugin_root.join("assets/icon.png")
        )?)
    );
    assert_eq!(
        interface.logo,
        Some(AbsolutePathBuf::try_from(
            plugin_root.join("assets/logo.png")
        )?)
    );
    assert_eq!(
        interface.screenshots,
        vec![
            AbsolutePathBuf::try_from(plugin_root.join("assets/screenshot1.png"))?,
            AbsolutePathBuf::try_from(plugin_root.join("assets/screenshot2.png"))?,
        ]
    );
    Ok(())
}

#[tokio::test]
async fn plugin_list_accepts_legacy_string_default_prompt() -> Result<()> {
    let ody_home = TempDir::new()?;
    let repo_root = TempDir::new()?;
    let plugin_root = repo_root.path().join("plugins/demo-plugin");
    std::fs::create_dir_all(repo_root.path().join(".git"))?;
    std::fs::create_dir_all(repo_root.path().join(".agents/plugins"))?;
    std::fs::create_dir_all(plugin_root.join(".ody-plugin"))?;
    write_plugins_enabled_config(ody_home.path())?;
    std::fs::write(
        repo_root.path().join(".agents/plugins/marketplace.json"),
        r#"{
  "name": "ody-curated",
  "plugins": [
    {
      "name": "demo-plugin",
      "source": {
        "source": "local",
        "path": "./plugins/demo-plugin"
      }
    }
  ]
}"#,
    )?;
    std::fs::write(
        plugin_root.join(".ody-plugin/plugin.json"),
        r##"{
  "name": "demo-plugin",
  "interface": {
    "defaultPrompt": "Starter prompt for trying a plugin"
  }
}"##,
    )?;

    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_list_request(PluginListParams {
            cwds: Some(vec![AbsolutePathBuf::try_from(repo_root.path())?]),
            marketplace_kinds: None,
        })
        .await?;

    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let response: PluginListResponse = to_response(response)?;

    let plugin = response
        .marketplaces
        .iter()
        .flat_map(|marketplace| marketplace.plugins.iter())
        .find(|plugin| plugin.name == "demo-plugin")
        .expect("expected demo-plugin entry");
    assert_eq!(
        plugin
            .interface
            .as_ref()
            .and_then(|interface| interface.default_prompt.clone()),
        Some(vec!["Starter prompt for trying a plugin".to_string()])
    );
    Ok(())
}

#[tokio::test]
async fn plugin_list_returns_installed_git_source_interface_from_cache() -> Result<()> {
    let ody_home = TempDir::new()?;
    let repo_root = TempDir::new()?;
    let missing_remote_repo = repo_root.path().join("missing-remote-plugin-repo");
    let missing_remote_repo_url = url::Url::from_directory_path(&missing_remote_repo)
        .unwrap()
        .to_string();
    std::fs::create_dir_all(repo_root.path().join(".git"))?;
    std::fs::create_dir_all(repo_root.path().join(".agents/plugins"))?;
    std::fs::write(
        repo_root.path().join(".agents/plugins/marketplace.json"),
        format!(
            r#"{{
  "name": "debug",
  "plugins": [
    {{
      "name": "toolkit",
      "source": {{
        "source": "git-subdir",
        "url": "{missing_remote_repo_url}",
        "path": "plugins/toolkit"
      }},
      "category": "Developer Tools"
    }}
  ]
}}"#
        ),
    )?;
    let cached_plugin_root = ody_home.path().join("plugins/cache/debug/toolkit/local");
    std::fs::create_dir_all(cached_plugin_root.join(".ody-plugin"))?;
    std::fs::write(
        cached_plugin_root.join(".ody-plugin/plugin.json"),
        r##"{
  "name": "toolkit",
  "interface": {
    "displayName": "Toolkit",
    "shortDescription": "Search cached data",
    "category": "Cached Category",
    "brandColor": "#3B82F6",
    "composerIcon": "./assets/icon.png",
    "logo": "./assets/logo.png"
  }
}"##,
    )?;
    std::fs::write(
        ody_home.path().join("config.toml"),
        r#"[features]
plugins = true

[plugins."toolkit@debug"]
enabled = true
"#,
    )?;

    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_list_request(PluginListParams {
            cwds: Some(vec![AbsolutePathBuf::try_from(repo_root.path())?]),
            marketplace_kinds: None,
        })
        .await?;

    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let response: PluginListResponse = to_response(response)?;

    let plugin = response
        .marketplaces
        .iter()
        .flat_map(|marketplace| marketplace.plugins.iter())
        .find(|plugin| plugin.name == "toolkit")
        .expect("expected toolkit entry");

    assert_eq!(plugin.id, "toolkit@debug");
    assert_eq!(plugin.installed, true);
    assert_eq!(plugin.enabled, true);
    assert_eq!(
        plugin.source,
        PluginSource::Git {
            url: missing_remote_repo_url,
            path: Some("plugins/toolkit".to_string()),
            ref_name: None,
            sha: None,
        }
    );
    let interface = plugin
        .interface
        .as_ref()
        .expect("expected cached plugin interface");
    assert_eq!(interface.display_name.as_deref(), Some("Toolkit"));
    assert_eq!(
        interface.short_description.as_deref(),
        Some("Search cached data")
    );
    assert_eq!(interface.category.as_deref(), Some("Developer Tools"));
    assert_eq!(interface.brand_color.as_deref(), Some("#3B82F6"));
    let canonical_cached_plugin_root = std::fs::canonicalize(&cached_plugin_root)?;
    assert_eq!(
        interface.composer_icon,
        Some(AbsolutePathBuf::try_from(
            canonical_cached_plugin_root.join("assets/icon.png")
        )?)
    );
    assert_eq!(
        interface.logo,
        Some(AbsolutePathBuf::try_from(
            canonical_cached_plugin_root.join("assets/logo.png")
        )?)
    );
    Ok(())
}

#[tokio::test]
async fn plugin_list_vertical_kind_returns_empty_without_remote_plugin_enabled() -> Result<()> {
    let ody_home = TempDir::new()?;
    write_plugins_enabled_config(ody_home.path())?;

    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_list_request(PluginListParams {
            cwds: None,
            marketplace_kinds: Some(vec![PluginListMarketplaceKind::Vertical]),
        })
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let response: PluginListResponse = to_response(response)?;

    assert_eq!(
        response,
        PluginListResponse {
            marketplaces: Vec::new(),
            marketplace_load_errors: Vec::new(),
            featured_plugin_ids: Vec::new(),
        }
    );
    Ok(())
}

#[tokio::test]
async fn plugin_list_does_not_query_odysseythink_curated_remote_collection_by_default() -> Result<()>
{
    let ody_home = TempDir::new()?;
    let server = MockServer::start().await;
    write_plugins_enabled_config_with_base_url(
        ody_home.path(),
        &format!("{}/backend-api/", server.uri()),
    )?;
    write_api_key_auth(
        ody_home.path(),
        ApiKeyAuthFixture::new("api-key").account_id("account-123"),
        AuthCredentialsStoreMode::File,
    )?;

    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_list_request(PluginListParams {
            cwds: None,
            marketplace_kinds: None,
        })
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let response: PluginListResponse = to_response(response)?;

    assert!(
        response
            .marketplaces
            .iter()
            .all(|marketplace| marketplace.name != "odysseythink-curated-remote")
    );
    assert!(
        server
            .received_requests()
            .await
            .expect("wiremock should record requests")
            .iter()
            .all(|request| !request
                .url
                .query_pairs()
                .any(|(name, value)| name == "collection" && value == "vertical"))
    );
    Ok(())
}

#[tokio::test]
async fn plugin_list_vertical_kind_noops_when_remote_plugin_enabled() -> Result<()> {
    let ody_home = TempDir::new()?;
    let server = MockServer::start().await;
    write_remote_plugin_catalog_config(ody_home.path(), &format!("{}/backend-api/", server.uri()))?;
    write_api_key_auth(
        ody_home.path(),
        ApiKeyAuthFixture::new("api-key").account_id("account-123"),
        AuthCredentialsStoreMode::File,
    )?;

    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_list_request(PluginListParams {
            cwds: None,
            marketplace_kinds: Some(vec![PluginListMarketplaceKind::Vertical]),
        })
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let response: PluginListResponse = to_response(response)?;

    assert!(
        response
            .marketplaces
            .iter()
            .all(|marketplace| marketplace.name != "odysseythink-curated-remote")
    );
    assert!(
        server
            .received_requests()
            .await
            .expect("wiremock should record requests")
            .iter()
            .all(|request| !request
                .url
                .query_pairs()
                .any(|(name, value)| name == "collection" && value == "vertical"))
    );
    Ok(())
}

#[tokio::test]
async fn plugin_list_does_not_append_global_remote_when_marketplace_kinds_are_explicit()
-> Result<()> {
    let ody_home = TempDir::new()?;
    let server = MockServer::start().await;
    write_remote_plugin_catalog_config(ody_home.path(), &format!("{}/backend-api/", server.uri()))?;
    write_api_key_auth(
        ody_home.path(),
        ApiKeyAuthFixture::new("api-key").account_id("account-123"),
        AuthCredentialsStoreMode::File,
    )?;

    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_list_request(PluginListParams {
            cwds: None,
            marketplace_kinds: Some(vec![PluginListMarketplaceKind::Local]),
        })
        .await?;

    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let response: PluginListResponse = to_response(response)?;

    assert!(
        response
            .marketplaces
            .iter()
            .all(|marketplace| marketplace.name != "odysseythink-curated-remote")
    );
    wait_for_remote_plugin_request_count(&server, "/ps/plugins/list", /*expected_count*/ 0).await?;
    Ok(())
}

#[tokio::test]
async fn plugin_list_omits_shared_with_me_kind_when_plugin_sharing_disabled() -> Result<()> {
    let ody_home = TempDir::new()?;
    let server = MockServer::start().await;
    std::fs::write(
        ody_home.path().join("config.toml"),
        format!(
            r#"legacy_base_url = "{}/backend-api/"

[features]
plugins = true
plugin_sharing = false
"#,
            server.uri()
        ),
    )?;
    write_api_key_auth(
        ody_home.path(),
        ApiKeyAuthFixture::new("api-key").account_id("account-123"),
        AuthCredentialsStoreMode::File,
    )?;

    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_list_request(PluginListParams {
            cwds: None,
            marketplace_kinds: Some(vec![PluginListMarketplaceKind::SharedWithMe]),
        })
        .await?;

    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let response: PluginListResponse = to_response(response)?;

    assert_eq!(
        response,
        PluginListResponse {
            marketplaces: Vec::new(),
            marketplace_load_errors: Vec::new(),
            featured_plugin_ids: Vec::new(),
        }
    );
    wait_for_remote_plugin_request_count(
        &server,
        "/ps/plugins/workspace/shared",
        /*expected_count*/ 0,
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn plugin_list_omits_created_by_me_when_remote_plugins_disabled() -> Result<()> {
    let ody_home = TempDir::new()?;
    let server = MockServer::start().await;
    std::fs::write(
        ody_home.path().join("config.toml"),
        format!(
            r#"legacy_base_url = "{}/backend-api/"

[features]
plugins = true
remote_plugin = false
plugin_sharing = true
"#,
            server.uri()
        ),
    )?;
    write_api_key_auth(
        ody_home.path(),
        ApiKeyAuthFixture::new("api-key").account_id("account-123"),
        AuthCredentialsStoreMode::File,
    )?;

    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_list_request(PluginListParams {
            cwds: None,
            marketplace_kinds: Some(vec![PluginListMarketplaceKind::CreatedByMeRemote]),
        })
        .await?;

    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let response: PluginListResponse = to_response(response)?;

    assert_eq!(
        response,
        PluginListResponse {
            marketplaces: Vec::new(),
            marketplace_load_errors: Vec::new(),
            featured_plugin_ids: Vec::new(),
        }
    );
    wait_for_remote_plugin_request_count(&server, "/ps/plugins/list", /*expected_count*/ 0).await?;
    Ok(())
}

#[tokio::test]
async fn plugin_list_does_not_fetch_remote_marketplaces_when_plugins_disabled() -> Result<()> {
    let ody_home = TempDir::new()?;
    let server = MockServer::start().await;
    std::fs::write(
        ody_home.path().join("config.toml"),
        format!(
            r#"
legacy_base_url = "{}/backend-api/"

[features]
plugins = false
remote_plugin = true
"#,
            server.uri()
        ),
    )?;
    write_api_key_auth(
        ody_home.path(),
        ApiKeyAuthFixture::new("api-key").account_id("account-123"),
        AuthCredentialsStoreMode::File,
    )?;

    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    let request_id = mcp
        .send_plugin_list_request(PluginListParams {
            cwds: None,
            marketplace_kinds: None,
        })
        .await?;

    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let response: PluginListResponse = to_response(response)?;

    assert!(response.marketplaces.is_empty());
    wait_for_remote_plugin_request_count(&server, "/ps/plugins/list", /*expected_count*/ 0).await?;
    Ok(())
}

async fn wait_for_remote_plugin_request_count(
    server: &MockServer,
    path_suffix: &str,
    expected_count: usize,
) -> Result<()> {
    timeout(DEFAULT_TIMEOUT, async {
        loop {
            let Some(requests) = server.received_requests().await else {
                bail!("wiremock did not record requests");
            };
            let request_count = requests
                .iter()
                .filter(|request| {
                    request.method == "GET" && request.url.path().ends_with(path_suffix)
                })
                .count();
            if request_count == expected_count {
                return Ok::<(), anyhow::Error>(());
            }
            if request_count > expected_count {
                bail!(
                    "expected exactly {expected_count} {path_suffix} requests, got {request_count}"
                );
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await??;
    Ok(())
}

fn write_installed_plugin(
    ody_home: &TempDir,
    marketplace_name: &str,
    plugin_name: &str,
) -> Result<()> {
    write_installed_plugin_with_version(ody_home, marketplace_name, plugin_name, "local")
}

fn write_installed_plugin_with_version(
    ody_home: &TempDir,
    marketplace_name: &str,
    plugin_name: &str,
    plugin_version: &str,
) -> Result<()> {
    let plugin_root = ody_home
        .path()
        .join("plugins/cache")
        .join(marketplace_name)
        .join(plugin_name)
        .join(plugin_version)
        .join(".ody-plugin");
    std::fs::create_dir_all(&plugin_root)?;
    std::fs::write(
        plugin_root.join("plugin.json"),
        format!(r#"{{"name":"{plugin_name}"}}"#),
    )?;
    Ok(())
}

fn write_remote_plugin_catalog_config(
    ody_home: &std::path::Path,
    base_url: &str,
) -> std::io::Result<()> {
    std::fs::write(
        ody_home.join("config.toml"),
        format!(
            r#"
legacy_base_url = "{base_url}"

[features]
plugins = true
remote_plugin = true
"#
        ),
    )
}

fn write_odysseythink_curated_marketplace(
    ody_home: &std::path::Path,
    plugin_names: &[&str],
) -> std::io::Result<()> {
    write_curated_marketplace(
        ody_home,
        "marketplace.json",
        "odysseythink-curated",
        /*display_name*/ None,
        plugin_names,
    )
}

fn write_odysseythink_api_curated_marketplace(
    ody_home: &std::path::Path,
    plugin_names: &[&str],
) -> std::io::Result<()> {
    write_curated_marketplace(
        ody_home,
        "api_marketplace.json",
        "odysseythink-api-curated",
        Some("OpenAI Curated"),
        plugin_names,
    )
}

fn write_curated_marketplace(
    ody_home: &std::path::Path,
    manifest_name: &str,
    marketplace_name: &str,
    display_name: Option<&str>,
    plugin_names: &[&str],
) -> std::io::Result<()> {
    let curated_root = ody_home.join(".tmp/plugins");
    std::fs::create_dir_all(curated_root.join(".git"))?;
    std::fs::create_dir_all(curated_root.join(".agents/plugins"))?;
    let plugins = plugin_names
        .iter()
        .map(|plugin_name| {
            format!(
                r#"{{
      "name": "{plugin_name}",
      "source": {{
        "source": "local",
        "path": "./plugins/{plugin_name}"
      }}
    }}"#
            )
        })
        .collect::<Vec<_>>()
        .join(",\n");
    let interface = display_name
        .map(|display_name| {
            format!(
                r#"
  "interface": {{
    "displayName": "{display_name}"
  }},"#
            )
        })
        .unwrap_or_default();
    std::fs::write(
        curated_root.join(".agents/plugins").join(manifest_name),
        format!(
            r#"{{
  "name": "{marketplace_name}",{interface}
  "plugins": [
{plugins}
  ]
}}"#
        ),
    )?;

    for plugin_name in plugin_names {
        let plugin_root = curated_root.join(format!("plugins/{plugin_name}/.ody-plugin"));
        std::fs::create_dir_all(&plugin_root)?;
        std::fs::write(
            plugin_root.join("plugin.json"),
            format!(r#"{{"name":"{plugin_name}"}}"#),
        )?;
    }
    std::fs::create_dir_all(ody_home.join(".tmp"))?;
    std::fs::write(
        ody_home.join(".tmp/plugins.sha"),
        format!("{TEST_CURATED_PLUGIN_SHA}\n"),
    )?;
    Ok(())
}
