use std::collections::HashMap;
use std::sync::RwLock;
use std::time::Duration;
use std::time::Instant;

use ody_core::config::Config;
use ody_login::OdyAuth;
use serde::Deserialize;

use crate::chatgpt_client::chatgpt_get_request_with_timeout;

const WORKSPACE_SETTINGS_TIMEOUT: Duration = Duration::from_secs(10);
const WORKSPACE_SETTINGS_CACHE_TTL: Duration = Duration::from_secs(15 * 60);
const ODY_PLUGINS_BETA_SETTING: &str = "enable_plugins";

#[derive(Debug, Deserialize)]
struct WorkspaceSettingsResponse {
    #[serde(default)]
    beta_settings: HashMap<String, bool>,
}

#[derive(Debug, Default)]
pub struct WorkspaceSettingsCache {
    entry: RwLock<Option<CachedWorkspaceSettings>>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct WorkspaceSettingsCacheKey {
    chatgpt_base_url: String,
    account_id: String,
}

#[derive(Clone, Debug)]
struct CachedWorkspaceSettings {
    key: WorkspaceSettingsCacheKey,
    expires_at: Instant,
    ody_plugins_enabled: bool,
}

impl WorkspaceSettingsCache {
    fn get_ody_plugins_enabled(&self, key: &WorkspaceSettingsCacheKey) -> Option<bool> {
        {
            let entry = match self.entry.read() {
                Ok(entry) => entry,
                Err(err) => err.into_inner(),
            };
            let now = Instant::now();
            if let Some(cached) = entry.as_ref()
                && now < cached.expires_at
                && cached.key == *key
            {
                return Some(cached.ody_plugins_enabled);
            }
        }

        let mut entry = match self.entry.write() {
            Ok(entry) => entry,
            Err(err) => err.into_inner(),
        };
        let now = Instant::now();
        if entry
            .as_ref()
            .is_some_and(|cached| now >= cached.expires_at || cached.key != *key)
        {
            *entry = None;
        }
        None
    }

    fn set_ody_plugins_enabled(&self, key: WorkspaceSettingsCacheKey, enabled: bool) {
        let mut entry = match self.entry.write() {
            Ok(entry) => entry,
            Err(err) => err.into_inner(),
        };
        *entry = Some(CachedWorkspaceSettings {
            key,
            expires_at: Instant::now() + WORKSPACE_SETTINGS_CACHE_TTL,
            ody_plugins_enabled: enabled,
        });
    }
}

pub async fn ody_plugins_enabled_for_workspace(
    config: &Config,
    auth: Option<&OdyAuth>,
    cache: Option<&WorkspaceSettingsCache>,
) -> anyhow::Result<bool> {
    // Workspace settings were only consulted for ChatGPT workspace auth, which
    // has been removed.
    let _ = (config, auth, cache);
    Ok(true)
}

fn encode_path_segment(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

#[cfg(test)]
#[path = "workspace_settings_tests.rs"]
mod tests;
