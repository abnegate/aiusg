use clap::{Parser, Subcommand};

use crate::model::Provider;

pub const DEFAULT_INTERVAL: u64 = 30;

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

    /// Order accounts by ORDER instead of by provider
    #[arg(
        short,
        long,
        value_enum,
        value_name = "ORDER",
        num_args = 0..=1,
        default_value_t = Sort::Provider,
        default_missing_value = "usable"
    )]
    pub sort: Sort,

    /// Keep the dashboard on screen, refreshing every SECONDS
    #[arg(
        short,
        long,
        visible_alias = "follow",
        alias = "live",
        alias = "interval",
        short_alias = 'i',
        value_name = "SECONDS",
        num_args = 0..=1,
        default_missing_value = "30",
        conflicts_with = "json"
    )]
    pub watch: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum Sort {
    /// Group by provider, as stored
    #[default]
    Provider,
    /// Most usable right now first: most headroom, then soonest back
    Usable,
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

    /// Live dashboard that refreshes on an interval, same as `aiusg --watch`
    Watch(StatusArgs),
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).unwrap()
    }

    #[test]
    fn watching_defaults_to_thirty_seconds() {
        for args in [
            ["aiusg", "--watch"].as_slice(),
            ["aiusg", "--follow"].as_slice(),
            ["aiusg", "--live"].as_slice(),
            ["aiusg", "-w"].as_slice(),
        ] {
            assert_eq!(parse(args).status.watch, Some(DEFAULT_INTERVAL));
        }
    }

    #[test]
    fn watching_takes_an_interval() {
        assert_eq!(parse(&["aiusg", "--watch", "5"]).status.watch, Some(5));
        assert_eq!(
            parse(&["aiusg", "--watch=5", "--provider", "claude"])
                .status
                .watch,
            Some(5)
        );
    }

    #[test]
    fn watching_is_off_by_default() {
        assert_eq!(parse(&["aiusg"]).status.watch, None);
    }

    #[test]
    fn the_watch_subcommand_defaults_to_the_same_interval() {
        let Some(Command::Watch(args)) = parse(&["aiusg", "watch"]).command else {
            panic!("expected the watch subcommand");
        };
        assert_eq!(args.watch, None);
        for args in [
            ["aiusg", "watch", "--interval", "5"].as_slice(),
            ["aiusg", "watch", "-i", "5"].as_slice(),
            ["aiusg", "watch", "-w", "5"].as_slice(),
        ] {
            let Some(Command::Watch(args)) = parse(args).command else {
                panic!("expected the watch subcommand");
            };
            assert_eq!(args.watch, Some(5));
        }
    }

    #[test]
    fn sorting_follows_the_stored_order_by_default() {
        assert_eq!(parse(&["aiusg"]).status.sort, Sort::Provider);
    }

    #[test]
    fn sorting_by_what_is_usable_takes_the_value_or_stands_alone() {
        for args in [
            ["aiusg", "--sort"].as_slice(),
            ["aiusg", "-s"].as_slice(),
            ["aiusg", "--sort", "usable"].as_slice(),
            ["aiusg", "--sort=usable"].as_slice(),
            ["aiusg", "-s", "usable"].as_slice(),
        ] {
            assert_eq!(parse(args).status.sort, Sort::Usable, "for {args:?}");
        }
    }

    #[test]
    fn watching_takes_an_order_too() {
        let cli = parse(&["aiusg", "--watch", "10", "--sort", "usable"]);
        assert_eq!(cli.status.watch, Some(10));
        assert_eq!(cli.status.sort, Sort::Usable);

        let Some(Command::Watch(args)) = parse(&["aiusg", "watch", "--sort"]).command else {
            panic!("expected the watch subcommand");
        };
        assert_eq!(args.sort, Sort::Usable);
    }

    #[test]
    fn an_unknown_order_is_rejected() {
        assert!(Cli::try_parse_from(["aiusg", "--sort", "sideways"]).is_err());
    }

    #[test]
    fn watching_cannot_emit_json() {
        assert!(Cli::try_parse_from(["aiusg", "--watch", "--json"]).is_err());
    }
}
