use clap::{Parser, Subcommand};

use crate::model::Provider;

#[derive(Debug, Parser)]
#[command(
    name = "aiusg",
    version,
    about = "Usage limits and reset times for all your AI provider accounts",
    args_conflicts_with_subcommands = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    #[command(flatten)]
    pub status: StatusArgs,
}

#[derive(Debug, Clone, clap::Args)]
pub struct StatusArgs {
    /// Only show accounts for this provider
    #[arg(short, long)]
    pub provider: Option<Provider>,

    /// Emit JSON instead of a table
    #[arg(long)]
    pub json: bool,

    /// Include accounts that are signed out
    #[arg(short, long)]
    pub all: bool,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Show usage for every account (default)
    Status(StatusArgs),

    /// Authenticate a new account
    Login { provider: Provider },

    /// Adopt accounts already signed in to their provider's CLI
    Import {
        /// Only import from this provider
        provider: Option<Provider>,
    },

    /// List stored accounts
    List,

    /// Forget a stored account
    Remove {
        /// Account id, as shown by `aiusg list`
        account: String,
    },

    /// Run an MCP server over stdio so agents can read usage and route requests
    Mcp,

    /// Live dashboard that refreshes on an interval
    Watch {
        /// Seconds between refreshes
        #[arg(short, long, default_value_t = 60)]
        interval: u64,
    },
}
