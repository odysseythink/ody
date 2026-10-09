//! Per-machine agent: reverse-connects to the gateway and bridges each
//! configured local ody app-server.

use anyhow::Context as _;
use clap::Parser;
use ody_remote::agent::config::AgentConfig;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "ody-remote-agent", about = "ody remote per-machine agent")]
struct Cli {
    /// Path to the agent TOML config.
    #[arg(long)]
    config: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let cli = Cli::parse();
    let text = std::fs::read_to_string(&cli.config)
        .with_context(|| format!("read config {}", cli.config.display()))?;
    let config = AgentConfig::from_toml_str(&text).context("parse config")?;
    tracing::info!(
        machine = %config.machine,
        instances = config.instances.len(),
        "ody-remote-agent starting"
    );
    ody_remote::agent::run(config).await
}
