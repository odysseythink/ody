use super::ToolSuggestDiscoverablePlugin;
use super::ToolSuggestPluginDiscoveryInput;
use crate::OPENAI_BUNDLED_MARKETPLACE_NAME;
use crate::PluginInstallRequest;
use crate::PluginsConfigInput;
use crate::PluginsManager;
use crate::loader::curated_plugins_repo_path;
use crate::test_support::TEST_CURATED_PLUGIN_SHA;
use crate::test_support::load_plugins_config;
use crate::test_support::write_curated_plugin;
use crate::test_support::write_curated_plugin_sha_with;
use crate::test_support::write_file;
use crate::test_support::write_odysseythink_api_curated_marketplace;
use crate::test_support::write_odysseythink_curated_marketplace;
use ody_app_server_protocol::AuthMode;
use ody_config::CONFIG_TOML_FILE;
use ody_utils_absolute_path::AbsolutePathBuf;
use pretty_assertions::assert_eq;
use std::collections::HashSet;
use std::path::Path;
use tempfile::tempdir;
use tracing::Level;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_test::internal::MockWriter;

#[tokio::test]
async fn returns_api_curated_fallback_plugins_for_direct_provider_auth() {
    let ody_home = tempdir().expect("tempdir should succeed");
    let curated_root = curated_plugins_repo_path(ody_home.path());
    write_odysseythink_api_curated_marketplace(
        &curated_root,
        &["sample", "slack", "odysseythink-developers"],
    );

    let plugins = load_plugins_config(ody_home.path(), ody_home.path()).await;
    let plugins_manager = PluginsManager::new(ody_home.path().to_path_buf());
    plugins_manager.set_auth_mode(Some(AuthMode::ApiKey));
    let discoverable_plugins =
        list_discoverable_plugins(&plugins_manager, discovery_input(plugins, &[], &[], &[])).await;

    assert_eq!(
        discoverable_plugins
            .into_iter()
            .map(|plugin| plugin.id)
            .collect::<Vec<_>>(),
        vec![
            "odysseythink-developers@odysseythink-api-curated".to_string(),
            "slack@odysseythink-api-curated".to_string(),
        ]
    );
}

#[tokio::test]
async fn returns_microsoft_fallback_plugins() {
    let ody_home = tempdir().expect("tempdir should succeed");
    let curated_root = curated_plugins_repo_path(ody_home.path());
    write_odysseythink_curated_marketplace(
        &curated_root,
        &["teams", "sharepoint", "outlook-email", "outlook-calendar"],
    );
    install_marketplace_plugin(ody_home.path(), curated_root.as_path(), "teams").await;

    let plugins = load_plugins_config(ody_home.path(), ody_home.path()).await;
    let plugins_manager = PluginsManager::new(ody_home.path().to_path_buf());
    let discoverable_plugins =
        list_discoverable_plugins(&plugins_manager, discovery_input(plugins, &[], &[], &[])).await;

    assert_eq!(
        discoverable_plugins
            .into_iter()
            .map(|plugin| plugin.id)
            .collect::<Vec<_>>(),
        vec![
            "outlook-calendar@odysseythink-curated".to_string(),
            "outlook-email@odysseythink-curated".to_string(),
            "sharepoint@odysseythink-curated".to_string(),
        ]
    );
}

#[tokio::test]
async fn reprojects_cached_skill_availability_for_current_config() {
    let ody_home = tempdir().expect("tempdir should succeed");
    let curated_root = curated_plugins_repo_path(ody_home.path());
    write_odysseythink_curated_marketplace(&curated_root, &["slack"]);

    let plugins = load_plugins_config(ody_home.path(), ody_home.path()).await;
    let plugins_manager = PluginsManager::new(ody_home.path().to_path_buf());
    let expected = ToolSuggestDiscoverablePlugin {
        id: "slack@odysseythink-curated".to_string(),
        remote_plugin_id: None,
        name: "slack".to_string(),
        description: Some(
            "Plugin that includes skills, MCP servers, and app connectors".to_string(),
        ),
        has_skills: true,
        mcp_server_names: vec!["sample-docs".to_string()],
        app_connector_ids: vec!["connector_calendar".to_string()],
    };
    let initial =
        list_discoverable_plugins(&plugins_manager, discovery_input(plugins, &[], &[], &[])).await;
    assert_eq!(initial, vec![expected.clone()]);

    write_file(
        &ody_home.path().join(CONFIG_TOML_FILE),
        r#"[[skills.config]]
name = "slack:sample"
enabled = false
"#,
    );
    let plugins = load_plugins_config(ody_home.path(), ody_home.path()).await;
    let after_skill_disabled =
        list_discoverable_plugins(&plugins_manager, discovery_input(plugins, &[], &[], &[])).await;
    assert_eq!(
        after_skill_disabled,
        vec![ToolSuggestDiscoverablePlugin {
            has_skills: false,
            ..expected
        }]
    );
}

#[tokio::test]
async fn does_not_advertise_skills_when_skill_loading_fails() {
    let ody_home = tempdir().expect("tempdir should succeed");
    let curated_root = curated_plugins_repo_path(ody_home.path());
    write_odysseythink_curated_marketplace(&curated_root, &["slack"]);
    write_file(
        &curated_root.join("plugins/slack/skills/SKILL.md"),
        "---\nname: bad",
    );

    let plugins = load_plugins_config(ody_home.path(), ody_home.path()).await;
    let plugins_manager = PluginsManager::new(ody_home.path().to_path_buf());
    let discoverable_plugins =
        list_discoverable_plugins(&plugins_manager, discovery_input(plugins, &[], &[], &[])).await;

    assert_eq!(
        discoverable_plugins,
        vec![ToolSuggestDiscoverablePlugin {
            id: "slack@odysseythink-curated".to_string(),
            remote_plugin_id: None,
            name: "slack".to_string(),
            description: Some(
                "Plugin that includes skills, MCP servers, and app connectors".to_string(),
            ),
            has_skills: false,
            mcp_server_names: vec!["sample-docs".to_string()],
            app_connector_ids: vec!["connector_calendar".to_string()],
        }]
    );
}

#[tokio::test]
async fn clear_cache_invalidates_cached_tool_suggest_metadata() {
    let ody_home = tempdir().expect("tempdir should succeed");
    let curated_root = curated_plugins_repo_path(ody_home.path());
    write_odysseythink_curated_marketplace(&curated_root, &["slack"]);
    let plugin_manifest = curated_root.join("plugins/slack/.ody-plugin/plugin.json");
    write_file(
        &plugin_manifest,
        r#"{
  "name": "slack",
  "description": "Before reload"
}"#,
    );

    let plugins = load_plugins_config(ody_home.path(), ody_home.path()).await;
    let plugins_manager = PluginsManager::new(ody_home.path().to_path_buf());
    let input = discovery_input(plugins, &[], &[], &[]);
    let expected_cached = vec![ToolSuggestDiscoverablePlugin {
        id: "slack@odysseythink-curated".to_string(),
        remote_plugin_id: None,
        name: "slack".to_string(),
        description: Some("Before reload".to_string()),
        has_skills: true,
        mcp_server_names: vec!["sample-docs".to_string()],
        app_connector_ids: vec!["connector_calendar".to_string()],
    }];
    let initial = list_discoverable_plugins(&plugins_manager, input.clone()).await;
    assert_eq!(initial, expected_cached);

    write_file(
        &plugin_manifest,
        r#"{
  "name": "slack",
  "description": "After reload"
}"#,
    );
    let before_reload = list_discoverable_plugins(&plugins_manager, input.clone()).await;
    assert_eq!(before_reload, expected_cached);

    plugins_manager.clear_cache();
    let after_reload = list_discoverable_plugins(&plugins_manager, input).await;
    assert_eq!(
        after_reload,
        vec![ToolSuggestDiscoverablePlugin {
            description: Some("After reload".to_string()),
            ..expected_cached[0].clone()
        }]
    );
}

#[tokio::test]
async fn ignores_missing_marketplace_plugin() {
    let ody_home = tempdir().expect("tempdir should succeed");
    let curated_root = curated_plugins_repo_path(ody_home.path());
    write_odysseythink_curated_marketplace(&curated_root, &["installed", "slack"]);
    let marketplace_name = OPENAI_BUNDLED_MARKETPLACE_NAME;
    let marketplace_root = ody_home
        .path()
        .join(format!(".tmp/marketplaces/{marketplace_name}"));
    write_file(
        &marketplace_root.join(".agents/plugins/marketplace.json"),
        &format!(
            r#"{{
  "name": "{marketplace_name}",
  "plugins": [
    {{"name": "sample", "source": {{"source": "local", "path": "./plugins/sample"}}}}
  ]
}}
"#
        ),
    );
    write_file(
        &ody_home.path().join(CONFIG_TOML_FILE),
        &format!(
            r#"[features]
plugins = true

[marketplaces.{marketplace_name}]
source_type = "git"
source = "/tmp/{marketplace_name}"
"#
        ),
    );
    install_marketplace_plugin(ody_home.path(), curated_root.as_path(), "installed").await;

    let plugins = load_plugins_config(ody_home.path(), ody_home.path()).await;
    let plugins_manager = PluginsManager::new(ody_home.path().to_path_buf());
    let discoverable_plugins =
        list_discoverable_plugins(&plugins_manager, discovery_input(plugins, &[], &[], &[])).await;

    assert_eq!(discoverable_plugins.len(), 1);
    assert_eq!(discoverable_plugins[0].id, "slack@odysseythink-curated");
}

#[tokio::test]
async fn normalizes_description() {
    let ody_home = tempdir().expect("tempdir should succeed");
    let curated_root = curated_plugins_repo_path(ody_home.path());
    write_odysseythink_curated_marketplace(&curated_root, &["installed", "slack"]);
    write_file(
        &curated_root.join("plugins/slack/.ody-plugin/plugin.json"),
        r#"{
  "name": "slack",
  "description": "  Plugin\n   with   extra   spacing  "
}"#,
    );
    install_marketplace_plugin(ody_home.path(), curated_root.as_path(), "installed").await;

    let plugins = load_plugins_config(ody_home.path(), ody_home.path()).await;
    let plugins_manager = PluginsManager::new(ody_home.path().to_path_buf());
    let discoverable_plugins =
        list_discoverable_plugins(&plugins_manager, discovery_input(plugins, &[], &[], &[])).await;

    assert_eq!(
        discoverable_plugins,
        vec![ToolSuggestDiscoverablePlugin {
            id: "slack@odysseythink-curated".to_string(),
            remote_plugin_id: None,
            name: "slack".to_string(),
            description: Some("Plugin with extra spacing".to_string()),
            has_skills: true,
            mcp_server_names: vec!["sample-docs".to_string()],
            app_connector_ids: vec!["connector_calendar".to_string()],
        }]
    );
}

#[tokio::test]
async fn omits_installed_curated_plugins() {
    let ody_home = tempdir().expect("tempdir should succeed");
    let curated_root = curated_plugins_repo_path(ody_home.path());
    write_odysseythink_curated_marketplace(&curated_root, &["slack"]);
    install_marketplace_plugin(ody_home.path(), curated_root.as_path(), "slack").await;

    let plugins = load_plugins_config(ody_home.path(), ody_home.path()).await;
    let plugins_manager = PluginsManager::new(ody_home.path().to_path_buf());
    let discoverable_plugins =
        list_discoverable_plugins(&plugins_manager, discovery_input(plugins, &[], &[], &[])).await;

    assert_eq!(discoverable_plugins, Vec::new());
}

#[tokio::test]
async fn omits_not_available_curated_plugins() {
    let ody_home = tempdir().expect("tempdir should succeed");
    let curated_root = curated_plugins_repo_path(ody_home.path());
    write_file(
        &curated_root.join(".agents/plugins/marketplace.json"),
        r#"{
  "name": "odysseythink-curated",
  "plugins": [
    {
      "name": "installed",
      "source": {
        "source": "local",
        "path": "./plugins/installed"
      }
    },
    {
      "name": "slack",
      "source": {
        "source": "local",
        "path": "./plugins/slack"
      }
    },
    {
      "name": "gmail",
      "source": {
        "source": "local",
        "path": "./plugins/gmail"
      },
      "policy": {
        "installation": "NOT_AVAILABLE"
      }
    }
  ]
}
"#,
    );
    write_curated_plugin(&curated_root, "installed");
    write_curated_plugin(&curated_root, "slack");
    write_curated_plugin(&curated_root, "gmail");
    install_marketplace_plugin(ody_home.path(), curated_root.as_path(), "installed").await;

    let plugins = load_plugins_config(ody_home.path(), ody_home.path()).await;
    let plugins_manager = PluginsManager::new(ody_home.path().to_path_buf());
    let discoverable_plugins =
        list_discoverable_plugins(&plugins_manager, discovery_input(plugins, &[], &[], &[])).await;

    assert_eq!(
        discoverable_plugins
            .into_iter()
            .map(|plugin| plugin.id)
            .collect::<Vec<_>>(),
        vec!["slack@odysseythink-curated".to_string()]
    );
}

#[tokio::test]
async fn does_not_reload_marketplace_per_plugin() {
    let ody_home = tempdir().expect("tempdir should succeed");
    let curated_root = curated_plugins_repo_path(ody_home.path());
    write_odysseythink_curated_marketplace(
        &curated_root,
        &["slack", "gmail", "odysseythink-developers"],
    );
    install_marketplace_plugin(ody_home.path(), curated_root.as_path(), "slack").await;

    let too_long_prompt = "x".repeat(129);
    for plugin_name in ["gmail", "odysseythink-developers"] {
        write_file(
            &curated_root.join(format!("plugins/{plugin_name}/.ody-plugin/plugin.json")),
            &format!(
                r#"{{
  "name": "{plugin_name}",
  "description": "Plugin that includes skills, MCP servers, and app connectors",
  "interface": {{
    "defaultPrompt": "{too_long_prompt}"
  }}
}}"#
            ),
        );
    }

    let plugins = load_plugins_config(ody_home.path(), ody_home.path()).await;
    let plugins_manager = PluginsManager::new(ody_home.path().to_path_buf());
    let buffer: &'static std::sync::Mutex<Vec<u8>> =
        Box::leak(Box::new(std::sync::Mutex::new(Vec::new())));
    let subscriber = tracing_subscriber::fmt()
        .with_level(true)
        .with_ansi(false)
        .with_max_level(Level::WARN)
        .with_span_events(FmtSpan::NONE)
        .with_writer(MockWriter::new(buffer))
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let discoverable_plugins =
        list_discoverable_plugins(&plugins_manager, discovery_input(plugins, &[], &[], &[])).await;

    assert_eq!(
        discoverable_plugins
            .iter()
            .map(|plugin| plugin.id.as_str())
            .collect::<Vec<_>>(),
        vec![
            "gmail@odysseythink-curated",
            "odysseythink-developers@odysseythink-curated"
        ]
    );

    let logs = String::from_utf8(buffer.lock().expect("buffer lock").clone())
        .expect("utf8 logs")
        .replace('\\', "/");
    assert_eq!(logs.matches("ignoring interface.defaultPrompt").count(), 8);
    assert_eq!(logs.matches("gmail/.ody-plugin/plugin.json").count(), 4);
    assert_eq!(
        logs.matches("odysseythink-developers/.ody-plugin/plugin.json")
            .count(),
        4
    );
}

#[tokio::test]
async fn does_not_expand_local_plugins_by_installed_apps() {
    let ody_home = tempdir().expect("tempdir should succeed");
    let curated_root = curated_plugins_repo_path(ody_home.path());
    write_odysseythink_curated_marketplace(&curated_root, &["sample", "slack", "hubspot"]);
    write_plugin_app(&curated_root, "sample", "sample", "connector_sample");
    install_marketplace_plugin(ody_home.path(), curated_root.as_path(), "slack").await;

    let plugins = load_plugins_config(ody_home.path(), ody_home.path()).await;
    let plugins_manager = PluginsManager::new(ody_home.path().to_path_buf());
    let discoverable_plugins =
        list_discoverable_plugins(&plugins_manager, discovery_input(plugins, &[], &[], &[])).await;

    assert_eq!(discoverable_plugins, Vec::new());
}

#[tokio::test]
async fn does_not_read_local_plugins_for_loaded_apps() {
    let hubspot_app_id = "asdk_app_697acb8e53d88191bf7a79e62012ae14";
    let granola_app_id = "asdk_app_697761cab6f48191b5ed345919a3ce8b";
    let ody_home = tempdir().expect("tempdir should succeed");
    let curated_root = curated_plugins_repo_path(ody_home.path());
    write_odysseythink_curated_marketplace(&curated_root, &["hubspot", "granola", "sample"]);
    write_plugin_app(&curated_root, "hubspot", "hubspot", hubspot_app_id);
    write_plugin_app(&curated_root, "granola", "granola", granola_app_id);
    write_file(
        &curated_root.join("plugins/sample/.app.json"),
        "invalid json",
    );

    let plugins = load_plugins_config(ody_home.path(), ody_home.path()).await;
    let plugins_manager = PluginsManager::new(ody_home.path().to_path_buf());
    let buffer: &'static std::sync::Mutex<Vec<u8>> =
        Box::leak(Box::new(std::sync::Mutex::new(Vec::new())));
    let subscriber = tracing_subscriber::fmt()
        .with_level(true)
        .with_ansi(false)
        .with_max_level(Level::WARN)
        .with_span_events(FmtSpan::NONE)
        .with_writer(MockWriter::new(buffer))
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let discoverable_plugins = list_discoverable_plugins(
        &plugins_manager,
        discovery_input(plugins, &[], &[], &[hubspot_app_id]),
    )
    .await;

    assert_eq!(discoverable_plugins, Vec::new());
    let logs = String::from_utf8(buffer.lock().expect("buffer lock").clone())
        .expect("utf8 logs")
        .replace('\\', "/");
    assert_eq!(logs.matches("plugins/sample/.app.json").count(), 0);
}

#[tokio::test]
async fn does_not_expand_local_sales_apps() {
    let hubspot_app_id = "asdk_app_697acb8e53d88191bf7a79e62012ae14";
    let granola_app_id = "asdk_app_697761cab6f48191b5ed345919a3ce8b";
    let test_app_id = "asdk_app_test_source";
    let ody_home = tempdir().expect("tempdir should succeed");
    let curated_root = curated_plugins_repo_path(ody_home.path());
    write_odysseythink_curated_marketplace(&curated_root, &["hubspot", "granola", "test-source"]);
    write_plugin_app(&curated_root, "hubspot", "hubspot", hubspot_app_id);
    write_plugin_app(&curated_root, "granola", "granola", granola_app_id);
    write_plugin_app(&curated_root, "test-source", "test_source", test_app_id);

    let sales_marketplace_name = "oai-maintained-plugins";
    let sales_marketplace_root = ody_home
        .path()
        .join(format!(".tmp/marketplaces/{sales_marketplace_name}"));
    write_file(
        &sales_marketplace_root.join(".agents/plugins/marketplace.json"),
        &format!(
            r#"{{
  "name": "{sales_marketplace_name}",
  "plugins": [
    {{"name": "sales", "source": {{"source": "local", "path": "./plugins/sales"}}}}
  ]
}}
"#
        ),
    );
    write_curated_plugin(&sales_marketplace_root, "sales");
    write_file(
        &sales_marketplace_root.join("plugins/sales/.app.json"),
        &format!(
            r#"{{
  "apps": {{
    "hubspot": {{
      "id": "{hubspot_app_id}"
    }},
    "granola": {{
      "id": "{granola_app_id}"
    }}
  }}
}}
"#
        ),
    );
    write_file(
        &ody_home.path().join(CONFIG_TOML_FILE),
        &format!(
            r#"[features]
plugins = true

[marketplaces.{sales_marketplace_name}]
source_type = "git"
source = "/tmp/{sales_marketplace_name}"
"#
        ),
    );
    install_marketplace_plugin(ody_home.path(), sales_marketplace_root.as_path(), "sales").await;

    let plugins = load_plugins_config(ody_home.path(), ody_home.path()).await;
    let plugins_manager = PluginsManager::new(ody_home.path().to_path_buf());
    let discoverable_plugins =
        list_discoverable_plugins(&plugins_manager, discovery_input(plugins, &[], &[], &[])).await;

    assert_eq!(discoverable_plugins, Vec::new());
}

fn discovery_input(
    plugins: PluginsConfigInput,
    configured_plugin_ids: &[&str],
    disabled_plugin_ids: &[&str],
    loaded_plugin_app_connector_ids: &[&str],
) -> ToolSuggestPluginDiscoveryInput {
    ToolSuggestPluginDiscoveryInput {
        plugins,
        configured_plugin_ids: string_set(configured_plugin_ids),
        disabled_plugin_ids: string_set(disabled_plugin_ids),
        loaded_plugin_app_connector_ids: string_set(loaded_plugin_app_connector_ids),
    }
}

async fn list_discoverable_plugins(
    plugins_manager: &PluginsManager,
    input: ToolSuggestPluginDiscoveryInput,
) -> Vec<ToolSuggestDiscoverablePlugin> {
    plugins_manager
        .list_tool_suggest_discoverable_plugins(&input)
        .await
        .expect("discoverable plugins should load")
}

fn string_set(values: &[&str]) -> HashSet<String> {
    values.iter().map(ToString::to_string).collect()
}

async fn install_marketplace_plugin(ody_home: &Path, marketplace_root: &Path, plugin_name: &str) {
    write_curated_plugin_sha_with(ody_home, TEST_CURATED_PLUGIN_SHA);
    PluginsManager::new(ody_home.to_path_buf())
        .install_plugin(PluginInstallRequest {
            plugin_name: plugin_name.to_string(),
            marketplace_path: AbsolutePathBuf::try_from(
                marketplace_root.join(".agents/plugins/marketplace.json"),
            )
            .expect("marketplace path"),
        })
        .await
        .expect("plugin should install");
}

fn write_plugin_app(root: &Path, plugin_name: &str, app_name: &str, app_id: &str) {
    write_file(
        &root.join(format!("plugins/{plugin_name}/.app.json")),
        &format!(
            r#"{{
  "apps": {{
    "{app_name}": {{
      "id": "{app_id}"
    }}
  }}
}}
"#
        ),
    );
}
