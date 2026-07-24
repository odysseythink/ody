//! Telemetry counters for the TUI `/login` and `/logout` flows.

use ody_model_provider_info::BuiltInApiKeyProvider;
use ody_otel::SessionTelemetry;

pub(crate) fn record_login_attempted(telemetry: &SessionTelemetry, provider: BuiltInApiKeyProvider) {
    telemetry.counter("ody.login.attempted", 1, &[("provider", provider.id())]);
}

pub(crate) fn record_login_succeeded(telemetry: &SessionTelemetry, provider: BuiltInApiKeyProvider) {
    telemetry.counter("ody.login.succeeded", 1, &[("provider", provider.id())]);
}

pub(crate) fn record_login_failed(
    telemetry: &SessionTelemetry,
    provider: BuiltInApiKeyProvider,
    reason: &str,
) {
    telemetry.counter(
        "ody.login.failed",
        1,
        &[("provider", provider.id()), ("reason", reason)],
    );
}

pub(crate) fn record_logout(telemetry: &SessionTelemetry, provider: Option<BuiltInApiKeyProvider>) {
    if let Some(provider) = provider {
        telemetry.counter("ody.logout", 1, &[("provider", provider.id())]);
    } else {
        telemetry.counter("ody.logout", 1, &[]);
    }
}
