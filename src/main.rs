use anyhow::Result;
use clap::Parser;

use aiusg::cli::{Cli, Command, DEFAULT_INTERVAL, StatusArgs};
use aiusg::{app, mcp};

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
        Some(Command::Watch(args)) => {
            app::status(StatusArgs {
                watch: Some(args.watch.unwrap_or(DEFAULT_INTERVAL)),
                ..args
            })
            .await
        }
    }
}
