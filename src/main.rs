use anyhow::Result;
use clap::Parser;

use aiusg::cli::{Cli, Command};
use aiusg::{app, mcp, render};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        None => app::status(cli.status).await,
        Some(Command::Status(args)) => app::status(args).await,
        Some(Command::Login { provider }) => app::login(provider).await,
        Some(Command::Import { provider }) => app::import(provider).await,
        Some(Command::List) => app::list().await,
        Some(Command::Remove { account }) => app::remove(&account).await,
        Some(Command::Mcp) => mcp::serve().await,
        Some(Command::Watch { interval }) => render::watch::run(interval).await,
    }
}
