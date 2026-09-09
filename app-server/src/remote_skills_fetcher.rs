//! HTTP fetcher backing [`ody_skills::remote_sync`] for builtin-skill sync.
//!
//! Talks to a chatbox-compatible backend (`https://api.chatboxai.app` by
//! default, overridable via `ODY_BUILTIN_SKILLS_ORIGIN`) and unwraps the
//! `{ data: ... }` response envelope the backend returns.

use std::sync::Arc;

use ody_core::skills::SkillsService;
use ody_protocol::protocol::Product;
use ody_skills::remote_sync::RemoteManifestItem;
use ody_skills::remote_sync::RemoteSkillDetail;
use ody_skills::remote_sync::RemoteSkillFetcher;
use ody_skills::remote_sync::RemoteSyncError;
use ody_skills::remote_sync::RemoteSyncOptions;
use ody_skills::remote_sync::sync_remote_builtin_skills;
use ody_utils_absolute_path::AbsolutePathBuf;

const DEFAULT_ORIGIN: &str = "https://api.chatboxai.app";
const ORIGIN_ENV_VAR: &str = "ODY_BUILTIN_SKILLS_ORIGIN";
const REQUEST_TIMEOUT_SECS: u64 = 10;

/// Minimal envelope shared by both backend endpoints.
#[derive(serde::Deserialize)]
struct DataEnvelope<T> {
    data: T,
}

pub struct HttpRemoteSkillFetcher {
    client: reqwest::Client,
    origin: String,
}

impl HttpRemoteSkillFetcher {
    pub fn new() -> Self {
        let origin = std::env::var(ORIGIN_ENV_VAR)
            .ok()
            .map(|value| value.trim().trim_end_matches('/').to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_ORIGIN.to_string());
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()
            .expect("reqwest client for builtin skill sync");
        Self { client, origin }
    }

    fn manifest_url(&self) -> String {
        format!("{}/api/builtin_skills", self.origin)
    }

    fn detail_url(&self, name: &str) -> String {
        format!(
            "{}/api/builtin_skills/{}",
            self.origin,
            urlencoding_proxy(name)
        )
    }

    async fn get_text(&self, url: &str) -> Result<Option<String>, RemoteSyncError> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|err| RemoteSyncError::Fetch(format!("GET {url} failed: {err}")))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(RemoteSyncError::Fetch(format!(
                "GET {url} returned {}",
                response.status()
            )));
        }
        response
            .text()
            .await
            .map(Some)
            .map_err(|err| RemoteSyncError::Fetch(format!("reading {url} body failed: {err}")))
    }
}

/// Percent-encodes a skill name for the URL path. Skill names are validated
/// to `[a-z0-9-]` by the sync layer, so only minimal escaping is needed; this
/// still avoids pulling in a url-encoding crate for a single use.
fn urlencoding_proxy(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '~') {
            out.push(ch);
        } else {
            out.push_str(&format!("%{:02X}", ch as u32));
        }
    }
    out
}

/// Spawns the background builtin-skill sync for odyBox sessions. Synced
/// skills are tagged for odyBox only; after a successful change the skills
/// cache is cleared so the next `skills/list` sees the fresh snapshot. All
/// failures are logged and swallowed (fail-open keeps the local snapshot).
pub fn spawn_builtin_skills_sync(ody_home: AbsolutePathBuf, skills_service: Arc<SkillsService>) {
    tokio::spawn(async move {
        let fetcher = HttpRemoteSkillFetcher::new();
        let options = RemoteSyncOptions {
            products: vec![Product::OdyBox],
        };
        // Embedded seeds are the source of truth for their names; the
        // chatbox backend serves some of them too and must not duplicate.
        let skip = ody_skills::embedded_system_skill_names();
        match sync_remote_builtin_skills(&ody_home, &fetcher, &options, &skip).await {
            Ok(true) => {
                tracing::info!("remote builtin skills synced; clearing skills cache");
                skills_service.clear_cache();
            }
            Ok(false) => {}
            Err(err) => {
                tracing::warn!("remote builtin skills sync failed: {err}");
            }
        }
    });
}

impl RemoteSkillFetcher for HttpRemoteSkillFetcher {
    async fn fetch_manifest(&self) -> Result<Vec<RemoteManifestItem>, RemoteSyncError> {
        let url = self.manifest_url();
        let text = self
            .get_text(&url)
            .await?
            .ok_or_else(|| RemoteSyncError::Fetch(format!("manifest missing at {url}")))?;
        let envelope: DataEnvelope<Vec<RemoteManifestItem>> = serde_json::from_str(&text)
            .map_err(|err| RemoteSyncError::Fetch(format!("manifest parse failed: {err}")))?;
        Ok(envelope.data)
    }

    async fn fetch_detail(&self, name: &str) -> Result<Option<RemoteSkillDetail>, RemoteSyncError> {
        let url = self.detail_url(name);
        let Some(text) = self.get_text(&url).await? else {
            return Ok(None);
        };
        let envelope: DataEnvelope<RemoteSkillDetail> = serde_json::from_str(&text)
            .map_err(|err| RemoteSyncError::Fetch(format!("detail parse failed for {name}: {err}")))?;
        Ok(Some(envelope.data))
    }
}

#[cfg(test)]
mod tests {
    use super::urlencoding_proxy;

    #[test]
    fn urlencoding_leaves_valid_skill_names_untouched() {
        assert_eq!(urlencoding_proxy("a-stock-data"), "a-stock-data");
        assert_eq!(urlencoding_proxy("humanizer-zh"), "humanizer-zh");
    }

    #[test]
    fn urlencoding_escapes_unsafe_characters() {
        assert_eq!(urlencoding_proxy("a b"), "a%20b");
        assert_eq!(urlencoding_proxy("../x"), "..%2Fx");
    }
}
