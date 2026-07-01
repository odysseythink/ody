use ody_app_server_protocol::AuthMode;
use ody_config::types::AuthCredentialsStoreMode;
use ody_login::AuthDotJson;
use ody_login::AuthKeyringBackendKind;
use ody_login::AuthManager;
use ody_login::logout;
use ody_login::save_auth;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

#[serial_test::serial(auth_env)]
#[tokio::test]
async fn logout_removes_auth_file() -> anyhow::Result<()> {
    let ody_home = TempDir::new()?;
    save_auth(
        ody_home.path(),
        &api_key_auth("sk-test-key"),
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )?;

    let removed = logout(
        ody_home.path(),
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )?;

    assert!(removed);
    assert!(!ody_home.path().join("auth.json").exists());
    Ok(())
}

#[serial_test::serial(auth_env)]
#[tokio::test]
async fn auth_manager_logout_removes_auth() -> anyhow::Result<()> {
    let ody_home = TempDir::new()?;
    save_auth(
        ody_home.path(),
        &api_key_auth("sk-test-key"),
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    )?;
    let manager = AuthManager::new(
        ody_home.path().to_path_buf(),
        /*enable_ody_api_key_env*/ false,
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
        /*auth_route_config*/ None,
    )
    .await;

    let removed = manager.logout().await?;

    assert!(removed);
    assert!(manager.auth_cached().is_none());
    assert!(!ody_home.path().join("auth.json").exists());
    Ok(())
}

fn api_key_auth(api_key: &str) -> AuthDotJson {
    AuthDotJson {
        auth_mode: Some(AuthMode::ApiKey),
        odysseythink_api_key: Some(api_key.to_string()),
    }
}
