use std::sync::Arc;
use std::time::Duration;

use http::HeaderMap;
use ody_api::ModelsClient;
use ody_api::RequestTelemetry;
use ody_api::ReqwestTransport;
use ody_api::TransportError;
use ody_api::auth_header_telemetry;
use ody_api::map_api_error;
use ody_client::default_client::build_reqwest_client;
use ody_model_provider_info::ModelProviderInfo;
use ody_models_manager::manager::ModelsEndpointClient;
use ody_models_manager::manager::ModelsEndpointFuture;
use ody_protocol::error::OdyErr;
use ody_protocol::error::Result as CoreResult;
use ody_protocol::model_metadata::ModelInfo;
use ody_response_debug_context::extract_response_debug_context;
use ody_response_debug_context::telemetry_transport_error_message;
use tokio::time::timeout;

use crate::auth::resolve_provider_auth;

const MODELS_REFRESH_TIMEOUT: Duration = Duration::from_secs(5);
const MODELS_ENDPOINT: &str = "/models";

/// Provider-owned OpenAI-compatible `/models` endpoint.
#[derive(Debug)]
pub(crate) struct OpenAiModelsEndpoint {
    provider_info: ModelProviderInfo,
}

impl OpenAiModelsEndpoint {
    pub(crate) fn new(provider_info: ModelProviderInfo) -> Self {
        Self { provider_info }
    }

    async fn uses_ody_backend(&self) -> bool {
        false
    }

    async fn list_models(
        &self,
        client_version: &str,
    ) -> CoreResult<(Vec<ModelInfo>, Option<String>)> {
        let _timer =
            ody_otel::start_global_timer("ody.remote_models.fetch_update.duration_ms", &[]);
        let api_provider = self.provider_info.to_api_provider(None)?;
        let api_auth = resolve_provider_auth(&self.provider_info)?;
        let transport = ReqwestTransport::new(build_reqwest_client());
        let auth_telemetry = auth_header_telemetry(api_auth.as_ref());
        let request_telemetry: Arc<dyn RequestTelemetry> = Arc::new(ModelsRequestTelemetry {
            auth_mode: None,
            auth_header_attached: auth_telemetry.attached,
            auth_header_name: auth_telemetry.name,
        });
        let client = ModelsClient::new(transport, api_provider, api_auth)
            .with_telemetry(Some(request_telemetry));

        timeout(
            MODELS_REFRESH_TIMEOUT,
            client.list_models(client_version, HeaderMap::new()),
        )
        .await
        .map_err(|_| OdyErr::Timeout)?
        .map_err(map_api_error)
    }
}

impl ModelsEndpointClient for OpenAiModelsEndpoint {
    fn has_command_auth(&self) -> bool {
        self.provider_info.has_command_auth()
    }

    fn uses_ody_backend(&self) -> ModelsEndpointFuture<'_, bool> {
        Box::pin(OpenAiModelsEndpoint::uses_ody_backend(self))
    }

    fn list_models<'a>(
        &'a self,
        client_version: &'a str,
    ) -> ModelsEndpointFuture<'a, CoreResult<(Vec<ModelInfo>, Option<String>)>> {
        Box::pin(OpenAiModelsEndpoint::list_models(self, client_version))
    }
}

#[derive(Clone)]
struct ModelsRequestTelemetry {
    auth_mode: Option<String>,
    auth_header_attached: bool,
    auth_header_name: Option<&'static str>,
}

impl RequestTelemetry for ModelsRequestTelemetry {
    fn on_request(
        &self,
        attempt: u64,
        status: Option<http::StatusCode>,
        error: Option<&TransportError>,
        duration: Duration,
    ) {
        let success = status.is_some_and(|code| code.is_success()) && error.is_none();
        let error_message = error.map(telemetry_transport_error_message);
        let response_debug = error
            .map(extract_response_debug_context)
            .unwrap_or_default();
        let status = status.map(|status| status.as_u16());
        tracing::event!(
            target: "ody_otel.log_only",
            tracing::Level::INFO,
            event.name = "ody.api_request",
            duration_ms = %duration.as_millis(),
            http.response.status_code = status,
            success = success,
            error.message = error_message.as_deref(),
            attempt = attempt,
            endpoint = MODELS_ENDPOINT,
            auth.header_attached = self.auth_header_attached,
            auth.header_name = self.auth_header_name,
            auth.request_id = response_debug.request_id.as_deref(),
            auth.cf_ray = response_debug.cf_ray.as_deref(),
            auth.error = response_debug.auth_error.as_deref(),
            auth.error_code = response_debug.auth_error_code.as_deref(),
            auth.mode = self.auth_mode.as_deref(),
        );
        tracing::event!(
            target: "ody_otel.trace_safe",
            tracing::Level::INFO,
            event.name = "ody.api_request",
            duration_ms = %duration.as_millis(),
            http.response.status_code = status,
            success = success,
            error.message = error_message.as_deref(),
            attempt = attempt,
            endpoint = MODELS_ENDPOINT,
            auth.header_attached = self.auth_header_attached,
            auth.header_name = self.auth_header_name,
            auth.request_id = response_debug.request_id.as_deref(),
            auth.cf_ray = response_debug.cf_ray.as_deref(),
            auth.error = response_debug.auth_error.as_deref(),
            auth.error_code = response_debug.auth_error_code.as_deref(),
            auth.mode = self.auth_mode.as_deref(),
        );
    }
}
