use std::sync::Arc;

use ody_analytics::AnalyticsEventsClient;
use ody_core::config::Config;
use ody_login::AuthManager;

pub(crate) fn analytics_events_client_from_config(
    auth_manager: Arc<AuthManager>,
    config: &Config,
) -> AnalyticsEventsClient {
    AnalyticsEventsClient::new(
        auth_manager,
        // The remote hosted plugin/Apps catalog config field this used to be sourced from has
        // been removed; analytics telemetry is unrelated to that catalog, so keep using its
        // historical default endpoint here.
        "https://chatgpt.com/backend-api".to_string(),
        config.analytics_enabled,
    )
}
