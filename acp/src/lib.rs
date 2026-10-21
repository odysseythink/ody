//! ACP (Agent Client Protocol) stdio frontend for Ody.
//!
//! Speaks newline-delimited JSON-RPC on stdio (the subset consumed by the
//! odyBox ACP client) and bridges to the in-process app-server runtime, the
//! same facade used by the TUI and exec frontends.
//!
//! stdout is reserved for protocol frames; all logging goes to stderr.
#![deny(clippy::print_stdout, clippy::print_stderr)]

use std::io::ErrorKind;
use std::io::Result as IoResult;
use std::sync::Arc;

use ody_app_server_client::DEFAULT_IN_PROCESS_CHANNEL_CAPACITY;
use ody_app_server_client::EnvironmentManager;
use ody_app_server_client::ExecServerRuntimePaths;
use ody_app_server_client::InProcessClientStartArgs;
use ody_arg0::Arg0DispatchPaths;
use ody_client::default_client::set_default_client_residency_requirement;
use ody_config::CloudConfigBundleLoader;
use ody_config::LoaderOverrides;
use ody_core::config::ConfigBuilder;
use ody_feedback::OdyFeedback;
use ody_protocol::protocol::SessionSource;
use ody_utils_cli::CliConfigOverrides;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::prelude::*;

mod backend;
mod bridge;
mod jsonrpc;

use crate::backend::AppServerBackend;
use crate::bridge::Bridge;

const OTEL_SERVICE_NAME: &str = "ody_acp";
const DEFAULT_ANALYTICS_ENABLED: bool = true;

/// Runs the ACP stdio server until stdin closes.
pub async fn run_main(
    arg0_paths: Arg0DispatchPaths,
    cli_config_overrides: CliConfigOverrides,
    strict_config: bool,
) -> IoResult<()> {
    let cli_kv_overrides = cli_config_overrides.parse_overrides().map_err(|e| {
        std::io::Error::new(
            ErrorKind::InvalidInput,
            format!("error parsing -c overrides: {e}"),
        )
    })?;
    let config = ConfigBuilder::default()
        .cli_overrides(cli_kv_overrides.clone())
        .strict_config(strict_config)
        .build()
        .await
        .map_err(|e| {
            std::io::Error::new(ErrorKind::InvalidData, format!("error loading config: {e}"))
        })?;
    set_default_client_residency_requirement(config.enforce_residency.value());
    let otel = ody_core::otel_init::build_provider(
        &config,
        env!("CARGO_PKG_VERSION"),
        Some(OTEL_SERVICE_NAME),
        DEFAULT_ANALYTICS_ENABLED,
    )
    .map_err(|e| {
        std::io::Error::new(
            ErrorKind::InvalidData,
            format!("error loading otel config: {e}"),
        )
    })?;
    ody_core::otel_init::record_process_start(otel.as_ref(), OTEL_SERVICE_NAME);
    ody_core::otel_init::install_sqlite_telemetry(otel.as_ref(), OTEL_SERVICE_NAME);
    let state_db = ody_core::init_state_db(&config).await;
    let environment_manager = Arc::new(
        EnvironmentManager::from_ody_home(
            config.ody_home.clone(),
            Some(ExecServerRuntimePaths::from_optional_paths(
                arg0_paths.ody_self_exe.clone(),
                arg0_paths.ody_linux_sandbox_exe.clone(),
            )?),
        )
        .await
        .map_err(std::io::Error::other)?,
    );

    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_filter(EnvFilter::from_default_env());
    let otel_logger_layer = otel.as_ref().and_then(|provider| provider.logger_layer());
    let otel_tracing_layer = otel.as_ref().and_then(|provider| provider.tracing_layer());
    let _ = tracing_subscriber::registry()
        .with(fmt_layer)
        .with(otel_logger_layer)
        .with(otel_tracing_layer)
        .try_init();

    let config_warnings: Vec<_> = config
        .startup_warnings
        .iter()
        .map(|warning| ody_app_server_protocol::ConfigWarningNotification {
            summary: warning.clone(),
            details: None,
            path: None,
            range: None,
        })
        .collect();

    let start_args = InProcessClientStartArgs {
        arg0_paths,
        config: Arc::new(config),
        cli_overrides: cli_kv_overrides,
        loader_overrides: LoaderOverrides::default(),
        strict_config,
        cloud_config_bundle: CloudConfigBundleLoader::default(),
        feedback: OdyFeedback::new(),
        log_db: None,
        state_db,
        environment_manager,
        config_warnings,
        session_source: SessionSource::Custom("acp".to_string()),
        client_name: "ody_acp".to_string(),
        client_version: env!("CARGO_PKG_VERSION").to_string(),
        experimental_api: true,
        mcp_server_form_elicitation: false,
        opt_out_notification_methods: Vec::new(),
        channel_capacity: DEFAULT_IN_PROCESS_CHANNEL_CAPACITY,
    };

    let backend = AppServerBackend::start(start_args).await.map_err(|err| {
        std::io::Error::other(format!("failed to initialize in-process app-server client: {err}"))
    })?;

    let stdin = tokio::io::stdin();
    let reader = tokio::io::BufReader::new(stdin);
    let stdout = tokio::io::stdout();
    let mut bridge = Bridge::new(backend, stdout);
    bridge.run(reader).await
}
