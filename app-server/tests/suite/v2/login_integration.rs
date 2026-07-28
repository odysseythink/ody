//! Mid-session `/login` integration test for numeric provider aliases.
//!
//! Simulates the TUI `/login` flow by writing the same config edits that
//! `persist_login_provider` produces, then verifies that:
//! 1. `model/list` is empty before login.
//! 2. After login, `model/list` surfaces the logged-in model under a numeric alias.
//! 3. The config round-trips `providers.123456`, `models."123456/<model>"`, and
//!    `default_model = "123456/<model>"`.
//! 4. A second login does not override the default model.
//! 5. `thread/start` resolves the numeric alias to the correct provider kind.

use anyhow::Result;
use app_test_support::TestAppServer;
use app_test_support::to_response;
use ody_app_server_protocol::ConfigBatchWriteParams;
use ody_app_server_protocol::ConfigEdit;
use ody_app_server_protocol::JSONRPCResponse;
use ody_app_server_protocol::MergeStrategy;
use ody_app_server_protocol::Model;
use ody_app_server_protocol::ModelListParams;
use ody_app_server_protocol::ModelListResponse;
use ody_app_server_protocol::RequestId;
use ody_app_server_protocol::ThreadSource;
use ody_app_server_protocol::ThreadStartParams;
use ody_app_server_protocol::ThreadStartResponse;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;
use tokio::time::timeout;

const DEFAULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

const FIRST_PROVIDER_ALIAS: &str = "123456";
const FIRST_PROVIDER_TYPE: &str = "kimi";
const FIRST_MODEL_ID: &str = "kimi-for-coding";

const SECOND_PROVIDER_ALIAS: &str = "second";
const SECOND_PROVIDER_TYPE: &str = "deepseek";
const SECOND_MODEL_ID: &str = "deepseek-chat";

/// Build the config edits that the TUI `/login` flow writes for a single
/// logged-in provider.  The numeric alias `123456` is intentionally used to
/// lock down the "dotted-path segment is a string table key" invariant.
fn login_config_edits(
    alias: &str,
    provider_type: &str,
    model_id: &str,
    set_as_default: bool,
) -> Vec<ConfigEdit> {
    let mut edits = vec![
        ConfigEdit {
            key_path: format!("providers.{alias}.type"),
            value: json!(provider_type),
            merge_strategy: MergeStrategy::Replace,
        },
        ConfigEdit {
            key_path: format!("providers.{alias}.api_key"),
            value: json!("secret"),
            merge_strategy: MergeStrategy::Replace,
        },
        ConfigEdit {
            key_path: format!("providers.{alias}.base_url"),
            value: json!("https://api.example.com/v1"),
            merge_strategy: MergeStrategy::Replace,
        },
        ConfigEdit {
            key_path: format!("models.\"{alias}/{model_id}\".provider"),
            value: json!(alias),
            merge_strategy: MergeStrategy::Replace,
        },
        ConfigEdit {
            key_path: format!("models.\"{alias}/{model_id}\".model"),
            value: json!(model_id),
            merge_strategy: MergeStrategy::Replace,
        },
    ];
    if set_as_default {
        edits.push(ConfigEdit {
            key_path: "default_model".to_string(),
            value: json!(format!("{alias}/{model_id}")),
            merge_strategy: MergeStrategy::Replace,
        });
    }
    edits
}

fn batch_write_params(edits: Vec<ConfigEdit>) -> ConfigBatchWriteParams {
    ConfigBatchWriteParams {
        edits,
        file_path: None,
        expected_version: None,
        reload_user_config: true,
    }
}

async fn send_config_batch_write(
    mcp: &mut TestAppServer,
    edits: Vec<ConfigEdit>,
) -> Result<()> {
    let request_id = mcp
        .send_config_batch_write_request(batch_write_params(edits))
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let _: ody_app_server_protocol::ConfigWriteResponse = to_response(response)?;
    Ok(())
}

async fn list_models(mcp: &mut TestAppServer) -> Result<Vec<Model>> {
    let request_id = mcp
        .send_list_models_request(ModelListParams {
            limit: Some(100),
            cursor: None,
            include_hidden: None,
        })
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    let ModelListResponse { data, .. } = to_response::<ModelListResponse>(response)?;
    Ok(data)
}

async fn start_thread_with_default_model(mcp: &mut TestAppServer) -> Result<ThreadStartResponse> {
    let request_id = mcp
        .send_thread_start_request(ThreadStartParams {
            thread_source: Some(ThreadSource::User),
            ..Default::default()
        })
        .await?;
    let response: JSONRPCResponse = timeout(
        DEFAULT_TIMEOUT,
        mcp.read_stream_until_response_message(RequestId::Integer(request_id)),
    )
    .await??;
    to_response::<ThreadStartResponse>(response)
}

fn read_config_toml(ody_home: &TempDir) -> Result<String> {
    let config_path = ody_home.path().join("config.toml");
    Ok(std::fs::read_to_string(config_path)?)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_login_with_numeric_alias_establishes_default_model() -> Result<()> {
    let ody_home = TempDir::new()?;
    // Empty config: no models, no providers, just like a fresh user before `/login`.
    std::fs::write(ody_home.path().join("config.toml"), "")?;

    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    // 1. Empty config yields no models.
    let models = list_models(&mut mcp).await?;
    assert!(models.is_empty(), "model/list should be empty before login");

    // 2. Simulate the first TUI `/login` with the numeric alias `123456`.
    send_config_batch_write(
        &mut mcp,
        login_config_edits(FIRST_PROVIDER_ALIAS, FIRST_PROVIDER_TYPE, FIRST_MODEL_ID, true),
    )
    .await?;

    // 3. `model/list` now surfaces the logged-in model with the numeric alias.
    let models = list_models(&mut mcp).await?;
    assert_eq!(
        models.len(),
        1,
        "model/list should surface exactly one model after first login"
    );
    let model = models.into_iter().next().unwrap();
    assert_eq!(model.model, "kimi-for-coding");
    assert_eq!(model.provider, "123456");

    // 4. Config round-trip: the numeric alias is preserved as a string key.
    let config_toml = read_config_toml(&ody_home)?;
    assert!(
        config_toml.contains("[providers.123456]"),
        "provider table must use numeric alias as a string key:\n{config_toml}"
    );
    assert!(
        config_toml.contains("type = \"kimi\""),
        "provider type must resolve to kimi:\n{config_toml}"
    );
    assert!(
        config_toml.contains("[models.\"123456/kimi-for-coding\"]"),
        "model table must use the qualified numeric alias as a string key:\n{config_toml}"
    );
    assert!(
        config_toml.contains("default_model = \"123456/kimi-for-coding\""),
        "default_model must be the qualified numeric alias:\n{config_toml}"
    );

    // 5. `thread/start` without an explicit model proves the default is wired
    //    correctly and that the numeric alias resolves to a provider.
    let thread_start = start_thread_with_default_model(&mut mcp).await?;
    assert_eq!(thread_start.model, "kimi-for-coding");
    assert_eq!(thread_start.model_provider, "123456");

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn second_login_does_not_override_numeric_alias_default() -> Result<()> {
    let ody_home = TempDir::new()?;
    std::fs::write(ody_home.path().join("config.toml"), "")?;

    let mut mcp = TestAppServer::new(ody_home.path()).await?;
    timeout(DEFAULT_TIMEOUT, mcp.initialize()).await??;

    // First login with the numeric alias `123456`; it becomes the default.
    send_config_batch_write(
        &mut mcp,
        login_config_edits(FIRST_PROVIDER_ALIAS, FIRST_PROVIDER_TYPE, FIRST_MODEL_ID, true),
    )
    .await?;

    // Second login with a regular alias; `set_as_default = false` mirrors the
    // TUI behavior of not stealing the default when an active model already exists.
    send_config_batch_write(
        &mut mcp,
        login_config_edits(
            SECOND_PROVIDER_ALIAS,
            SECOND_PROVIDER_TYPE,
            SECOND_MODEL_ID,
            false,
        ),
    )
    .await?;

    // The active provider is still the first numeric alias, but model/list
    // surfaces models for every configured provider alias so the user can switch
    // via `/model` without a restart.
    let models = list_models(&mut mcp).await?;
    assert_eq!(
        models.len(),
        2,
        "model/list should surface models for both configured providers"
    );
    let providers: Vec<&str> = models.iter().map(|m| m.provider.as_str()).collect();
    assert!(providers.contains(&FIRST_PROVIDER_ALIAS), "first provider missing");
    assert!(providers.contains(&SECOND_PROVIDER_ALIAS), "second provider missing");
    let first_model = models.iter().find(|m| m.provider == FIRST_PROVIDER_ALIAS).unwrap();
    assert_eq!(first_model.model, "kimi-for-coding");
    assert_eq!(first_model.provider, "123456");

    // The default model must still be the first numeric-alias login.
    let config_toml = read_config_toml(&ody_home)?;
    assert!(
        config_toml.contains("default_model = \"123456/kimi-for-coding\""),
        "second login must not override the default model:\n{config_toml}"
    );

    let thread_start = start_thread_with_default_model(&mut mcp).await?;
    assert_eq!(thread_start.model, "kimi-for-coding");
    assert_eq!(thread_start.model_provider, "123456");

    Ok(())
}
