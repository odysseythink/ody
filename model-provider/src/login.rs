//! OpenAI-compatible `/models` fetcher used by the TUI `/login` flow.
//!
//! This is intentionally separate from the runtime model-manager endpoint client
//! so the login flow can verify a user-provided API key and base URL without
//! constructing a full runtime provider.

use std::collections::HashMap;
use std::time::Duration;

use http::HeaderMap;
use http::HeaderName;
use http::HeaderValue;
use ody_model_provider_info::LoginProvider;
use serde::Deserialize;
use serde::Serialize;

const MODELS_FETCH_TIMEOUT: Duration = Duration::from_secs(15);

/// Minimal model description returned by a provider's `/models` endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoginModelInfo {
    /// Model id sent in API requests (e.g. `kimi-k2`).
    pub id: String,
    /// Human-readable name shown in the picker.
    pub display_name: String,
}

/// OpenAI-compatible `/models` list response.
#[derive(Debug, Deserialize)]
struct OpenAiModelsResponse {
    #[serde(default)]
    data: Vec<OpenAiModelEntry>,
}

#[derive(Debug, Deserialize)]
struct OpenAiModelEntry {
    id: String,
}

/// Errors that can occur while verifying a login provider.
#[derive(Debug, thiserror::Error)]
pub enum LoginModelError {
    #[error("request failed: {0}")]
    RequestFailed(String),
    #[error("invalid response: {0}")]
    InvalidResponse(String),
    #[error("no models returned by provider")]
    NoModels,
}

/// Fetch available models from an OpenAI-compatible `/models` endpoint.
///
/// `base_url` is the provider's API root (e.g. `https://api.moonshot.ai/v1`).
/// `api_key` is used for `Authorization: Bearer ...`. Extra headers are merged
/// in for providers that require identity headers (e.g. Kimi).
pub async fn fetch_login_models(
    _provider: LoginProvider,
    base_url: &str,
    api_key: &str,
    extra_headers: Option<HashMap<String, String>>,
) -> Result<Vec<LoginModelInfo>, LoginModelError> {
    let client = reqwest::Client::builder()
        .timeout(MODELS_FETCH_TIMEOUT)
        .build()
        .map_err(|e| LoginModelError::RequestFailed(e.to_string()))?;

    let mut url = base_url.trim_end_matches('/').to_string();
    url.push_str("/models");

    let mut headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_str(&format!("Bearer {api_key}")) {
        headers.insert(http::header::AUTHORIZATION, value);
    }
    if let Some(extra) = extra_headers {
        for (k, v) in extra {
            if let (Ok(name), Ok(value)) = (HeaderName::try_from(k), HeaderValue::try_from(v)) {
                headers.insert(name, value);
            }
        }
    }

    let response = client
        .get(&url)
        .headers(headers)
        .send()
        .await
        .map_err(|e| LoginModelError::RequestFailed(e.to_string()))?;

    let status = response.status();
    if !status.is_success() {
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "<unreadable body>".to_string());
        return Err(LoginModelError::RequestFailed(format!(
            "HTTP {status}: {body}"
        )));
    }

    let body = response.text().await.map_err(|e| {
        LoginModelError::InvalidResponse(format!("failed to read response body: {e}"))
    })?;
    parse_models_response(&body)
}

fn parse_models_response(body: &str) -> Result<Vec<LoginModelInfo>, LoginModelError> {
    let parsed: OpenAiModelsResponse = serde_json::from_str(body)
        .map_err(|e| LoginModelError::InvalidResponse(format!("failed to parse JSON: {e}")))?;

    let mut models: Vec<LoginModelInfo> = parsed
        .data
        .into_iter()
        .map(|entry| LoginModelInfo {
            id: entry.id.clone(),
            display_name: entry.id,
        })
        .collect();

    if models.is_empty() {
        return Err(LoginModelError::NoModels);
    }

    models.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(models)
}

#[cfg(test)]
#[path = "login_tests.rs"]
mod tests;
