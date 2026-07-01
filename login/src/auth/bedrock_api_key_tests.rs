use ody_app_server_protocol::AuthMode;
use ody_config::types::AuthCredentialsStoreMode;
use pretty_assertions::assert_eq;
use serial_test::serial;
use tempfile::tempdir;

use super::*;
use crate::auth::AuthKeyringBackendKind;
use crate::auth::AuthManager;
use crate::auth::OdyAuth;
use crate::auth::storage::AuthStorageBackend;
use crate::auth::storage::FileAuthStorage;

fn api_key_auth() -> AuthDotJson {
    AuthDotJson {
        auth_mode: Some(AuthMode::ApiKey),
        odysseythink_api_key: Some("sk-test-key".to_string()),
        tokens: None,
        last_refresh: None,
        bedrock_api_key: None,
    }
}

fn bedrock_only_auth() -> AuthDotJson {
    AuthDotJson {
        auth_mode: None,
        odysseythink_api_key: None,
        tokens: None,
        last_refresh: None,
        bedrock_api_key: Some(bedrock_auth()),
    }
}

fn bedrock_auth() -> BedrockApiKeyAuth {
    BedrockApiKeyAuth {
        api_key: "bedrock-api-key-test".to_string(),
        region: "us-east-1".to_string(),
    }
}

#[tokio::test]
#[serial(ody_auth_env)]
async fn login_with_bedrock_api_key_replaces_odysseythink_auth() -> anyhow::Result<()> {
    let ody_home = tempdir()?;
    let storage = FileAuthStorage::new(ody_home.path().to_path_buf());
    storage.save(&api_key_auth())?;
    login_with_bedrock_api_key(
        ody_home.path(),
        "bedrock-api-key-test",
        "us-east-1",
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )?;

    let auth_manager = AuthManager::new(
        ody_home.path().to_path_buf(),
        /*enable_ody_api_key_env*/ false,
        AuthCredentialsStoreMode::File,
        /*forced_chatgpt_workspace_id*/ None,
        /*chatgpt_base_url*/ None,
        AuthKeyringBackendKind::default(),
        /*auth_route_config*/ None,
    )
    .await;

    let loaded = storage.load()?.expect("auth should be stored");
    let expected = AuthDotJson {
        auth_mode: Some(AuthMode::BedrockApiKey),
        odysseythink_api_key: None,
        tokens: None,
        last_refresh: None,
        bedrock_api_key: Some(bedrock_auth()),
    };
    assert_eq!(loaded, expected);
    assert_eq!(auth_manager.auth_mode(), Some(AuthMode::BedrockApiKey));
    assert_eq!(
        auth_manager.auth_cached().and_then(|auth| match auth {
            OdyAuth::BedrockApiKey(auth) => Some(auth),
            OdyAuth::ApiKey(_)
            | OdyAuth::Chatgpt(_)
            | OdyAuth::ChatgptAuthTokens(_)
            | OdyAuth::AgentIdentity(_)
            | OdyAuth::PersonalAccessToken(_) => None,
        }),
        Some(bedrock_auth())
    );
    Ok(())
}

#[tokio::test]
#[serial(ody_auth_env)]
async fn logout_removes_bedrock_auth() -> anyhow::Result<()> {
    let ody_home = tempdir()?;
    let storage = FileAuthStorage::new(ody_home.path().to_path_buf());
    login_with_bedrock_api_key(
        ody_home.path(),
        "bedrock-api-key-test",
        "us-east-1",
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )?;
    let auth_manager = AuthManager::new(
        ody_home.path().to_path_buf(),
        /*enable_ody_api_key_env*/ false,
        AuthCredentialsStoreMode::File,
        /*forced_chatgpt_workspace_id*/ None,
        /*chatgpt_base_url*/ None,
        AuthKeyringBackendKind::default(),
        /*auth_route_config*/ None,
    )
    .await;

    assert!(auth_manager.logout().await?);

    assert_eq!(storage.load()?, None);
    assert_eq!(auth_manager.auth_cached(), None);
    Ok(())
}

#[tokio::test]
#[serial(ody_auth_env)]
async fn bedrock_only_auth_storage_creates_primary_auth() -> anyhow::Result<()> {
    let ody_home = tempdir()?;
    let storage = FileAuthStorage::new(ody_home.path().to_path_buf());
    storage.save(&bedrock_only_auth())?;

    let auth_manager = AuthManager::new(
        ody_home.path().to_path_buf(),
        /*enable_ody_api_key_env*/ false,
        AuthCredentialsStoreMode::File,
        /*forced_chatgpt_workspace_id*/ None,
        /*chatgpt_base_url*/ None,
        AuthKeyringBackendKind::default(),
        /*auth_route_config*/ None,
    )
    .await;

    assert_eq!(auth_manager.auth_mode(), Some(AuthMode::BedrockApiKey));
    assert_eq!(
        auth_manager.auth_cached().and_then(|auth| match auth {
            OdyAuth::BedrockApiKey(auth) => Some(auth),
            OdyAuth::ApiKey(_)
            | OdyAuth::Chatgpt(_)
            | OdyAuth::ChatgptAuthTokens(_)
            | OdyAuth::AgentIdentity(_)
            | OdyAuth::PersonalAccessToken(_) => None,
        }),
        Some(bedrock_auth())
    );
    Ok(())
}

#[tokio::test]
async fn login_with_api_key_clears_bedrock_api_key() -> anyhow::Result<()> {
    let ody_home = tempdir()?;
    let storage = FileAuthStorage::new(ody_home.path().to_path_buf());
    login_with_bedrock_api_key(
        ody_home.path(),
        "bedrock-api-key-test",
        "us-east-1",
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )?;

    crate::auth::login_with_api_key(
        ody_home.path(),
        "sk-test-key",
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )?;

    assert_eq!(storage.load()?, Some(api_key_auth()));
    Ok(())
}
