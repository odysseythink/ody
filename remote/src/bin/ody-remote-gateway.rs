//! VPS-side hub: authenticates the single user, accepts agent connections,
//! serves the PWA, and proxies read-only JSON-RPC to instances.

use anyhow::Context as _;
use clap::Parser;
use clap::Subcommand;
use ody_remote::gateway::server::GatewayConfig;
use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "ody-remote-gateway", about = "ody remote gateway (VPS hub)")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the gateway. TLS is expected to be terminated in front (e.g. Caddy).
    Serve {
        /// Path to the gateway TOML config.
        #[arg(long)]
        config: PathBuf,
        /// Listen address; defaults to loopback (reverse-proxy fronting).
        #[arg(long, default_value = "127.0.0.1:8080")]
        listen: SocketAddr,
    },
    /// Print an argon2 hash for the given password (for the config file).
    /// Note: the password will be visible in shell history; prefer pasting via
    /// `read -s` on sensitive machines.
    HashPassword { password: String },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let cli = Cli::parse();
    match cli.command {
        Command::HashPassword { password } => {
            println!("{}", ody_remote::gateway::auth::hash_password(&password));
            Ok(())
        }
        Command::Serve { config, listen } => {
            let text = std::fs::read_to_string(&config)
                .with_context(|| format!("read config {}", config.display()))?;
            let config = GatewayConfig::from_toml_str(&text).context("parse config")?;
            let listener = tokio::net::TcpListener::bind(listen)
                .await
                .with_context(|| format!("bind {listen}"))?;
            tracing::info!(%listen, "ody-remote-gateway listening");
            axum::serve(listener, ody_remote::gateway::server::router(config))
                .await
                .context("serve")
        }
    }
}
