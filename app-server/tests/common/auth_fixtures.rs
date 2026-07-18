use std::path::Path;

use anyhow::Context;
use anyhow::Result;
use ody_app_server_protocol::AuthMode;
use ody_config::types::AuthCredentialsStoreMode;
use std::fs;

/// Builder for writing a fake auth.json in tests using API-key auth.
#[derive(Debug, Clone)]
pub struct ApiKeyAuthFixture {
    api_key: String,
}

impl ApiKeyAuthFixture {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
        }
    }

    pub fn refresh_token(self, _refresh_token: impl Into<String>) -> Self {
        self
    }

    pub fn account_id(self, _account_id: impl Into<String>) -> Self {
        self
    }

    pub fn plan_type(self, _plan_type: impl Into<String>) -> Self {
        self
    }

    pub fn email(self, _email: impl Into<String>) -> Self {
        self
    }

    pub fn last_refresh(self, _last_refresh: Option<()>) -> Self {
        self
    }
}

pub fn write_api_key_auth(
    ody_home: &Path,
    fixture: ApiKeyAuthFixture,
    _cli_auth_credentials_store_mode: AuthCredentialsStoreMode,
) -> Result<()> {
    let auth = serde_json::json!({
        "auth_mode": "api_key",
        "odysseythink_api_key": fixture.api_key,
    });

    let auth_path = ody_home.join("auth.json");
    fs::write(&auth_path, serde_json::to_vec_pretty(&auth)?).context("write auth.json")
}
