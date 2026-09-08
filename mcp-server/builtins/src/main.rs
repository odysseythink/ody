use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "ody-builtin-mcp")]
#[command(about = "odyBox builtin MCP servers (stdio transport)")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Fetch web pages and convert HTML to markdown
    Fetch,
    /// Structured sequential thinking
    Sequentialthinking,
    /// Search, download, and read arXiv papers
    Arxiv,
    /// Up-to-date library documentation via Context7
    Context7,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Fetch => ody_builtin_mcp::fetch::run().await?,
        Commands::Sequentialthinking => ody_builtin_mcp::sequentialthinking::run().await?,
        Commands::Arxiv => ody_builtin_mcp::arxiv::run().await?,
        Commands::Context7 => ody_builtin_mcp::context7::run().await?,
    }
    Ok(())
}
