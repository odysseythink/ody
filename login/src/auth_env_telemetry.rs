use ody_model_provider_info::ModelProviderInfo;
use ody_otel::AuthEnvTelemetryMetadata;

use crate::ODY_API_KEY_ENV_VAR;
use crate::OPENAI_API_KEY_ENV_VAR;
use crate::REFRESH_TOKEN_URL_OVERRIDE_ENV_VAR;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthEnvTelemetry {
    pub odysseythink_api_key_env_present: bool,
    pub ody_api_key_env_present: bool,
    pub ody_api_key_env_enabled: bool,
    pub provider_env_key_name: Option<String>,
    pub provider_env_key_present: Option<bool>,
    pub refresh_token_url_override_present: bool,
}

impl AuthEnvTelemetry {
    pub fn to_otel_metadata(&self) -> AuthEnvTelemetryMetadata {
        AuthEnvTelemetryMetadata {
            odysseythink_api_key_env_present: self.odysseythink_api_key_env_present,
            ody_api_key_env_present: self.ody_api_key_env_present,
            ody_api_key_env_enabled: self.ody_api_key_env_enabled,
            provider_env_key_name: self.provider_env_key_name.clone(),
            provider_env_key_present: self.provider_env_key_present,
            refresh_token_url_override_present: self.refresh_token_url_override_present,
        }
    }
}

pub fn collect_auth_env_telemetry(
    provider: &ModelProviderInfo,
    ody_api_key_env_enabled: bool,
) -> AuthEnvTelemetry {
    AuthEnvTelemetry {
        odysseythink_api_key_env_present: env_var_present(OPENAI_API_KEY_ENV_VAR),
        ody_api_key_env_present: env_var_present(ODY_API_KEY_ENV_VAR),
        ody_api_key_env_enabled,
        provider_env_key_name: provider.env_key.as_ref().map(|_| "configured".to_string()),
        provider_env_key_present: provider.env_key.as_deref().map(env_var_present),
        refresh_token_url_override_present: env_var_present(REFRESH_TOKEN_URL_OVERRIDE_ENV_VAR),
    }
}

fn env_var_present(name: &str) -> bool {
    match std::env::var(name) {
        Ok(value) => !value.trim().is_empty(),
        Err(std::env::VarError::NotUnicode(_)) => true,
        Err(std::env::VarError::NotPresent) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ody_model_provider_info::WireApi;
    use pretty_assertions::assert_eq;

    #[test]
    fn collect_auth_env_telemetry_buckets_provider_env_key_name() {
        let provider = ModelProviderInfo {
            name: "Custom".to_string(),
            base_url: None,
            env_key: Some("sk-should-not-leak".to_string()),
            env_key_instructions: None,
            experimental_bearer_token: None,
            auth: None,
            aws: None,
            wire_api: WireApi::Responses,
            query_params: None,
            http_headers: None,
            env_http_headers: None,
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            websocket_connect_timeout_ms: None,
            requires_odysseythink_auth: false,
            supports_websockets: false,
        };

        let telemetry =
            collect_auth_env_telemetry(&provider, /*ody_api_key_env_enabled*/ false);

        assert_eq!(
            telemetry.provider_env_key_name,
            Some("configured".to_string())
        );
    }
}
