use std::path::Path;

use anyhow::Context;
use anyhow::Result;
use ody_app_server_protocol::AuthMode;
use ody_config::types::AuthCredentialsStoreMode;
use ody_login::AuthDotJson;
use ody_login::AuthKeyringBackendKind;
use ody_login::save_auth;

/// Builder for writing a fake auth.json in tests.
///
/// Historically this constructed ChatGPT JWT credentials. With ChatGPT auth
/// removed, the fixture now writes API-key auth while keeping the builder API
/// so existing tests continue to compile.
#[derive(Debug, Clone)]
pub struct ChatGptAuthFixture {
    access_token: String,
}

impl ChatGptAuthFixture {
    pub fn new(access_token: impl Into<String>) -> Self {
        Self {
            access_token: access_token.into(),
        }
    }

    pub fn refresh_token(mut self, _refresh_token: impl Into<String>) -> Self {
        self
    }

    pub fn account_id(mut self, _account_id: impl Into<String>) -> Self {
        self
    }

    pub fn plan_type(mut self, _plan_type: impl Into<String>) -> Self {
        self
    }

    pub fn chatgpt_user_id(mut self, _chatgpt_user_id: impl Into<String>) -> Self {
        self
    }

    pub fn chatgpt_account_id(mut self, _chatgpt_account_id: impl Into<String>) -> Self {
        self
    }

    pub fn email(mut self, _email: impl Into<String>) -> Self {
        self
    }

    pub fn last_refresh(mut self, _last_refresh: Option<()>) -> Self {
        self
    }

    pub fn claims(mut self, _claims: ChatGptIdTokenClaims) -> Self {
        self
    }
}

/// Legacy claims type. Kept for source compatibility but no longer used.
#[derive(Debug, Clone, Default)]
pub struct ChatGptIdTokenClaims {
    pub email: Option<String>,
    pub plan_type: Option<String>,
    pub chatgpt_user_id: Option<String>,
    pub chatgpt_account_id: Option<String>,
}

impl ChatGptIdTokenClaims {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn email(mut self, _email: impl Into<String>) -> Self {
        self
    }

    pub fn plan_type(mut self, _plan_type: impl Into<String>) -> Self {
        self
    }

    pub fn chatgpt_user_id(mut self, _chatgpt_user_id: impl Into<String>) -> Self {
        self
    }

    pub fn chatgpt_account_id(mut self, _chatgpt_account_id: impl Into<String>) -> Self {
        self
    }
}

/// Legacy token encoder. Kept for source compatibility but returns a dummy value.
pub fn encode_id_token(_claims: &ChatGptIdTokenClaims) -> Result<String> {
    Ok("dummy-token".to_string())
}

pub fn write_chatgpt_auth(
    ody_home: &Path,
    fixture: ChatGptAuthFixture,
    cli_auth_credentials_store_mode: AuthCredentialsStoreMode,
) -> Result<()> {
    let auth = AuthDotJson {
        auth_mode: Some(AuthMode::ApiKey),
        odysseythink_api_key: Some(fixture.access_token),
    };

    save_auth(
        ody_home,
        &auth,
        cli_auth_credentials_store_mode,
        AuthKeyringBackendKind::default(),
    )
    .context("write auth.json")
}
