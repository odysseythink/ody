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
        // The remote analytics endpoint is no longer available; use an empty base URL so the
        // analytics client treats delivery as disabled.
        String::new(),
        config.analytics_enabled,
    )
}
