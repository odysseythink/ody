use super::*;
use anyhow::Context;
use sha2::Digest;
use ody_secrets::LocalSecretsNamespace;
use ody_secrets::SecretScope;
use ody_secrets::SecretsBackendKind;
use ody_secrets::SecretsManager;
use ody_secrets::compute_keyring_account;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::tempdir;

use ody_keyring_store::tests::MockKeyringStore;
use keyring::Error as KeyringError;

#[tokio::test]
async fn file_storage_load_returns_auth_dot_json() -> anyhow::Result<()> {
    let ody_home = tempdir()?;
    let storage = FileAuthStorage::new(ody_home.path().to_path_buf());
    let auth_dot_json = AuthDotJson {
        auth_mode: Some(AuthMode::ApiKey),
        odysseythink_api_key: Some("test-key".to_string()),
    };

    storage
        .save(&auth_dot_json)
        .context("failed to save auth file")?;

    let loaded = storage.load().context("failed to load auth file")?;
    assert_eq!(Some(auth_dot_json), loaded);
    Ok(())
}

#[tokio::test]
async fn file_storage_save_persists_auth_dot_json() -> anyhow::Result<()> {
    let ody_home = tempdir()?;
    let storage = FileAuthStorage::new(ody_home.path().to_path_buf());
    let auth_dot_json = AuthDotJson {
        auth_mode: Some(AuthMode::ApiKey),
        odysseythink_api_key: Some("test-key".to_string()),
    };

    let file = get_auth_file(ody_home.path());
    storage
        .save(&auth_dot_json)
        .context("failed to save auth file")?;

    let same_auth_dot_json = storage
        .try_read_auth_json(&file)
        .context("failed to read auth file after save")?;
    assert_eq!(auth_dot_json, same_auth_dot_json);
    Ok(())
}

#[test]
fn file_storage_delete_removes_auth_file() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let auth_dot_json = AuthDotJson {
        auth_mode: Some(AuthMode::ApiKey),
        odysseythink_api_key: Some("sk-test-key".to_string()),
    };
    let storage = create_auth_storage(
        dir.path().to_path_buf(),
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::default(),
    );
    storage.save(&auth_dot_json)?;
    assert!(dir.path().join("auth.json").exists());
    let storage = FileAuthStorage::new(dir.path().to_path_buf());
    let removed = storage.delete()?;
    assert!(removed);
    assert!(!dir.path().join("auth.json").exists());
    Ok(())
}

#[test]
fn ephemeral_storage_save_load_delete_is_in_memory_only() -> anyhow::Result<()> {
    let dir = tempdir()?;
    let storage = create_auth_storage(
        dir.path().to_path_buf(),
        AuthCredentialsStoreMode::Ephemeral,
        AuthKeyringBackendKind::default(),
    );
    let auth_dot_json = AuthDotJson {
        auth_mode: Some(AuthMode::ApiKey),
        odysseythink_api_key: Some("sk-ephemeral".to_string()),
    };

    storage.save(&auth_dot_json)?;
    let loaded = storage.load()?;
    assert_eq!(Some(auth_dot_json), loaded);

    let removed = storage.delete()?;
    assert!(removed);
    let loaded = storage.load()?;
    assert_eq!(None, loaded);
    assert!(!get_auth_file(dir.path()).exists());
    Ok(())
}

fn seed_secrets_backend_and_fallback_auth_file_for_delete(
    mock_keyring: &MockKeyringStore,
    ody_home: &Path,
    auth: &AuthDotJson,
) -> anyhow::Result<PathBuf> {
    let manager = SecretsManager::new_with_keyring_store_and_namespace(
        ody_home.to_path_buf(),
        SecretsBackendKind::Local,
        Arc::new(mock_keyring.clone()),
        LocalSecretsNamespace::OdyAuth,
    );
    manager.set(
        &SecretScope::Global,
        &ODY_AUTH_SECRET_NAME,
        &serde_json::to_string(auth)?,
    )?;
    let auth_file = get_auth_file(ody_home);
    std::fs::write(&auth_file, "stale")?;
    Ok(auth_file)
}

fn seed_secrets_backend_with_auth(
    mock_keyring: &MockKeyringStore,
    ody_home: &Path,
    auth: &AuthDotJson,
) -> anyhow::Result<()> {
    let manager = SecretsManager::new_with_keyring_store_and_namespace(
        ody_home.to_path_buf(),
        SecretsBackendKind::Local,
        Arc::new(mock_keyring.clone()),
        LocalSecretsNamespace::OdyAuth,
    );
    manager.set(
        &SecretScope::Global,
        &ODY_AUTH_SECRET_NAME,
        &serde_json::to_string(auth)?,
    )?;
    Ok(())
}

fn assert_keyring_saved_auth_and_removed_fallback(
    mock_keyring: &MockKeyringStore,
    ody_home: &Path,
    expected: &AuthDotJson,
) -> anyhow::Result<()> {
    let manager = SecretsManager::new_with_keyring_store_and_namespace(
        ody_home.to_path_buf(),
        SecretsBackendKind::Local,
        Arc::new(mock_keyring.clone()),
        LocalSecretsNamespace::OdyAuth,
    );
    let saved_value = manager
        .get(&SecretScope::Global, &ODY_AUTH_SECRET_NAME)?
        .context("encrypted auth entry should exist")?;
    let expected_serialized = serde_json::to_string(expected)?;
    assert_eq!(saved_value, expected_serialized);
    let old_key = compute_store_key(ody_home)?;
    assert!(
        mock_keyring.saved_value(&old_key).is_none(),
        "legacy keyring auth entry should not be used"
    );
    let secrets_key = compute_keyring_account(ody_home);
    assert!(
        mock_keyring.saved_value(&secrets_key).is_some(),
        "secrets backend should persist an encryption passphrase in the keyring"
    );
    assert!(encrypted_auth_file(ody_home).exists());
    let auth_file = get_auth_file(ody_home);
    assert!(
        !auth_file.exists(),
        "fallback auth.json should be removed after keyring save"
    );
    Ok(())
}

fn encrypted_auth_file(ody_home: &Path) -> PathBuf {
    ody_home.join("secrets").join("ody_auth.age")
}

fn auth_with_prefix(prefix: &str) -> AuthDotJson {
    AuthDotJson {
        auth_mode: Some(AuthMode::ApiKey),
        odysseythink_api_key: Some(format!("{prefix}-api-key")),
    }
}

#[test]
fn secrets_keyring_auth_storage_load_returns_deserialized_auth() -> anyhow::Result<()> {
    let ody_home = tempdir()?;
    let mock_keyring = MockKeyringStore::default();
    let storage = SecretsKeyringAuthStorage::new(
        ody_home.path().to_path_buf(),
        Arc::new(mock_keyring.clone()),
    );
    let expected = AuthDotJson {
        auth_mode: Some(AuthMode::ApiKey),
        odysseythink_api_key: Some("sk-test".to_string()),
    };
    seed_secrets_backend_with_auth(&mock_keyring, ody_home.path(), &expected)?;

    let loaded = storage.load()?;
    assert_eq!(Some(expected), loaded);
    Ok(())
}

#[test]
fn keyring_auth_storage_compute_store_key_for_home_directory() -> anyhow::Result<()> {
    let ody_home = tempdir()?;
    let canonical = ody_home
        .path()
        .canonicalize()
        .unwrap_or_else(|_| ody_home.path().to_path_buf());
    let mut hasher = sha2::Sha256::new();
    hasher.update(canonical.to_string_lossy().as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    let expected = format!("cli|{}", &digest[..16]);

    let key = compute_store_key(ody_home.path())?;
    assert_eq!(key, expected);
    Ok(())
}

#[test]
fn direct_keyring_auth_storage_saves_legacy_keyring_entry() -> anyhow::Result<()> {
    let ody_home = tempdir()?;
    let mock_keyring = MockKeyringStore::default();
    let storage = DirectKeyringAuthStorage::new(
        ody_home.path().to_path_buf(),
        Arc::new(mock_keyring.clone()),
    );
    let auth_file = get_auth_file(ody_home.path());
    std::fs::write(&auth_file, "stale")?;
    let auth = auth_with_prefix("direct");

    storage.save(&auth)?;

    let legacy_key = compute_store_key(ody_home.path())?;
    let saved_value = mock_keyring
        .saved_value(&legacy_key)
        .context("direct keyring auth entry should exist")?;
    assert_eq!(saved_value, serde_json::to_string(&auth)?);
    assert!(!encrypted_auth_file(ody_home.path()).exists());
    assert!(
        !auth_file.exists(),
        "fallback auth.json should be removed after keyring save"
    );
    assert_eq!(storage.load()?, Some(auth));
    Ok(())
}

#[test]
fn direct_keyring_auth_storage_delete_removes_keyring_and_file() -> anyhow::Result<()> {
    let ody_home = tempdir()?;
    let mock_keyring = MockKeyringStore::default();
    let storage = DirectKeyringAuthStorage::new(
        ody_home.path().to_path_buf(),
        Arc::new(mock_keyring.clone()),
    );
    let auth = auth_with_prefix("direct-delete");
    storage.save(&auth)?;
    let auth_file = get_auth_file(ody_home.path());
    std::fs::write(&auth_file, "stale")?;

    let removed = storage.delete()?;

    assert!(removed, "delete should report removal");
    assert_eq!(storage.load()?, None, "keyring auth should be removed");
    assert!(
        mock_keyring
            .saved_value(&compute_store_key(ody_home.path())?)
            .is_none(),
        "legacy keyring auth entry should be removed"
    );
    assert!(
        !auth_file.exists(),
        "fallback auth.json should be removed after keyring delete"
    );
    assert!(!encrypted_auth_file(ody_home.path()).exists());
    Ok(())
}

#[test]
fn factory_uses_secrets_backend_only_when_requested() -> anyhow::Result<()> {
    let direct_home = tempdir()?;
    let direct_keyring = MockKeyringStore::default();
    let direct_storage = create_auth_storage_with_store(
        direct_home.path().to_path_buf(),
        AuthCredentialsStoreMode::Keyring,
        Arc::new(direct_keyring.clone()),
        AuthKeyringBackendKind::Direct,
    );
    let direct_auth = auth_with_prefix("factory-direct");
    direct_storage.save(&direct_auth)?;
    assert!(
        direct_keyring
            .saved_value(&compute_store_key(direct_home.path())?)
            .is_some()
    );
    assert!(!encrypted_auth_file(direct_home.path()).exists());

    let secrets_home = tempdir()?;
    let secrets_keyring = MockKeyringStore::default();
    let secrets_storage = create_auth_storage_with_store(
        secrets_home.path().to_path_buf(),
        AuthCredentialsStoreMode::Keyring,
        Arc::new(secrets_keyring.clone()),
        AuthKeyringBackendKind::Secrets,
    );
    let secrets_auth = auth_with_prefix("factory-secrets");
    secrets_storage.save(&secrets_auth)?;
    assert!(
        secrets_keyring
            .saved_value(&compute_keyring_account(secrets_home.path()))
            .is_some()
    );
    assert!(encrypted_auth_file(secrets_home.path()).exists());
    Ok(())
}

#[test]
fn secrets_keyring_auth_storage_save_persists_and_removes_fallback_file() -> anyhow::Result<()> {
    let ody_home = tempdir()?;
    let mock_keyring = MockKeyringStore::default();
    let storage = SecretsKeyringAuthStorage::new(
        ody_home.path().to_path_buf(),
        Arc::new(mock_keyring.clone()),
    );
    let auth_file = get_auth_file(ody_home.path());
    std::fs::write(&auth_file, "stale")?;
    let auth = AuthDotJson {
        auth_mode: Some(AuthMode::ApiKey),
        odysseythink_api_key: Some("sk-api-key".to_string()),
    };

    storage.save(&auth)?;

    assert_keyring_saved_auth_and_removed_fallback(&mock_keyring, ody_home.path(), &auth)?;
    Ok(())
}

#[test]
fn secrets_keyring_auth_storage_delete_removes_keyring_and_file() -> anyhow::Result<()> {
    let ody_home = tempdir()?;
    let mock_keyring = MockKeyringStore::default();
    let storage = SecretsKeyringAuthStorage::new(
        ody_home.path().to_path_buf(),
        Arc::new(mock_keyring.clone()),
    );
    let auth = auth_with_prefix("to-delete");
    let auth_file = seed_secrets_backend_and_fallback_auth_file_for_delete(
        &mock_keyring,
        ody_home.path(),
        &auth,
    )?;

    let removed = storage.delete()?;

    assert!(removed, "delete should report removal");
    assert_eq!(storage.load()?, None, "encrypted auth should be removed");
    assert!(
        !auth_file.exists(),
        "fallback auth.json should be removed after keyring delete"
    );
    Ok(())
}

#[test]
fn secrets_keyring_auth_storage_delete_removes_legacy_direct_keyring_entry() -> anyhow::Result<()> {
    let ody_home = tempdir()?;
    let mock_keyring = MockKeyringStore::default();
    let direct_storage = DirectKeyringAuthStorage::new(
        ody_home.path().to_path_buf(),
        Arc::new(mock_keyring.clone()),
    );
    direct_storage.save(&auth_with_prefix("legacy-direct"))?;
    let storage = SecretsKeyringAuthStorage::new(
        ody_home.path().to_path_buf(),
        Arc::new(mock_keyring.clone()),
    );
    let auth = auth_with_prefix("to-delete");
    let auth_file = seed_secrets_backend_and_fallback_auth_file_for_delete(
        &mock_keyring,
        ody_home.path(),
        &auth,
    )?;

    let removed = storage.delete()?;

    assert!(removed, "delete should report removal");
    assert_eq!(storage.load()?, None, "encrypted auth should be removed");
    assert_eq!(
        direct_storage.load()?,
        None,
        "legacy direct keyring auth should be removed"
    );
    assert!(
        !auth_file.exists(),
        "fallback auth.json should be removed after keyring delete"
    );
    Ok(())
}

#[test]
fn auto_auth_storage_load_prefers_keyring_value() -> anyhow::Result<()> {
    let ody_home = tempdir()?;
    let mock_keyring = MockKeyringStore::default();
    let storage = AutoAuthStorage::new(
        ody_home.path().to_path_buf(),
        Arc::new(mock_keyring.clone()),
        AuthKeyringBackendKind::Secrets,
    );
    let keyring_auth = auth_with_prefix("keyring");
    seed_secrets_backend_with_auth(&mock_keyring, ody_home.path(), &keyring_auth)?;

    let file_auth = auth_with_prefix("file");
    storage.file_storage.save(&file_auth)?;

    let loaded = storage.load()?;
    assert_eq!(loaded, Some(keyring_auth));
    Ok(())
}

#[test]
fn auto_auth_storage_load_uses_file_when_keyring_empty() -> anyhow::Result<()> {
    let ody_home = tempdir()?;
    let mock_keyring = MockKeyringStore::default();
    let storage = AutoAuthStorage::new(
        ody_home.path().to_path_buf(),
        Arc::new(mock_keyring),
        AuthKeyringBackendKind::Secrets,
    );

    let expected = auth_with_prefix("file-only");
    storage.file_storage.save(&expected)?;

    let loaded = storage.load()?;
    assert_eq!(loaded, Some(expected));
    Ok(())
}

#[test]
fn auto_auth_storage_load_falls_back_when_keyring_errors() -> anyhow::Result<()> {
    let ody_home = tempdir()?;
    let mock_keyring = MockKeyringStore::default();
    let storage = AutoAuthStorage::new(
        ody_home.path().to_path_buf(),
        Arc::new(mock_keyring.clone()),
        AuthKeyringBackendKind::Secrets,
    );
    let key = compute_keyring_account(ody_home.path());

    let encrypted = auth_with_prefix("encrypted");
    seed_secrets_backend_with_auth(&mock_keyring, ody_home.path(), &encrypted)?;
    mock_keyring.set_error(&key, KeyringError::Invalid("error".into(), "load".into()));

    let expected = auth_with_prefix("fallback");
    storage.file_storage.save(&expected)?;

    let loaded = storage.load()?;
    assert_eq!(loaded, Some(expected));
    Ok(())
}

#[test]
fn auto_auth_storage_save_prefers_keyring() -> anyhow::Result<()> {
    let ody_home = tempdir()?;
    let mock_keyring = MockKeyringStore::default();
    let storage = AutoAuthStorage::new(
        ody_home.path().to_path_buf(),
        Arc::new(mock_keyring.clone()),
        AuthKeyringBackendKind::Secrets,
    );
    let stale = auth_with_prefix("stale");
    storage.file_storage.save(&stale)?;

    let expected = auth_with_prefix("to-save");
    storage.save(&expected)?;

    assert_keyring_saved_auth_and_removed_fallback(&mock_keyring, ody_home.path(), &expected)?;
    Ok(())
}

#[test]
fn auto_auth_storage_save_falls_back_when_keyring_errors() -> anyhow::Result<()> {
    let ody_home = tempdir()?;
    let mock_keyring = MockKeyringStore::default();
    let storage = AutoAuthStorage::new(
        ody_home.path().to_path_buf(),
        Arc::new(mock_keyring.clone()),
        AuthKeyringBackendKind::Secrets,
    );
    let key = compute_keyring_account(ody_home.path());
    mock_keyring.set_error(&key, KeyringError::Invalid("error".into(), "save".into()));

    let auth = auth_with_prefix("fallback");
    storage.save(&auth)?;

    let auth_file = get_auth_file(ody_home.path());
    assert!(
        auth_file.exists(),
        "fallback auth.json should be created when keyring save fails"
    );
    let saved = storage
        .file_storage
        .load()?
        .context("fallback auth should exist")?;
    assert_eq!(saved, auth);
    assert!(
        mock_keyring.saved_value(&key).is_none(),
        "keyring should not contain value when save fails"
    );
    Ok(())
}

#[test]
fn auto_auth_storage_delete_removes_keyring_and_file() -> anyhow::Result<()> {
    let ody_home = tempdir()?;
    let mock_keyring = MockKeyringStore::default();
    let storage = AutoAuthStorage::new(
        ody_home.path().to_path_buf(),
        Arc::new(mock_keyring.clone()),
        AuthKeyringBackendKind::Secrets,
    );
    let auth = auth_with_prefix("to-delete");
    let auth_file = seed_secrets_backend_and_fallback_auth_file_for_delete(
        &mock_keyring,
        ody_home.path(),
        &auth,
    )?;

    let removed = storage.delete()?;

    assert!(removed, "delete should report removal");
    assert_eq!(storage.load()?, None, "encrypted auth should be removed");
    assert!(
        !auth_file.exists(),
        "fallback auth.json should be removed after delete"
    );
    Ok(())
}

#[test]
fn auth_dot_json_ignores_unknown_legacy_fields_and_bedrock_key() -> anyhow::Result<()> {
    let ody_home = tempdir()?;
    let auth_file = get_auth_file(ody_home.path());
    // Legacy auth.json payloads may carry unknown fields such as OAuth tokens or a
    // Bedrock API key. We should still be able to read the file and the unknown
    // fields should be ignored.
    std::fs::write(
        &auth_file,
        json!({
            "auth_mode": "apikey",
            "OPENAI_API_KEY": "sk-api-key",
            "tokens": {
                "id_token": "x.y.z",
                "access_token": "at",
                "refresh_token": "rt",
                "account_id": "account-id"
            },
            "last_refresh": "2024-01-01T00:00:00Z",
            "bedrock_api_key": {
                "api_key": "bedrock-key",
                "region": "us-east-1"
            }
        })
        .to_string(),
    )?;

    let storage = FileAuthStorage::new(ody_home.path().to_path_buf());
    let loaded = storage.load()?.context("auth should load")?;
    assert_eq!(
        loaded,
        AuthDotJson {
            auth_mode: Some(AuthMode::ApiKey),
            odysseythink_api_key: Some("sk-api-key".to_string()),
        }
    );
    assert_eq!(loaded.resolved_mode(), AuthMode::ApiKey);
    Ok(())
}
