//! CLI-Walker: a terminal dashboard for the Kingsmith WalkingPad A1 Pro.

mod commands;
mod model;
mod pad;
mod paint;
mod protocol;
mod store;
mod strava;
mod tracker;
mod tui;
mod walk;

use std::io::IsTerminal;

use clap::{Parser, Subcommand};

/// Log walking sessions from a Kingsmith WalkingPad A1 Pro.
#[derive(Debug, Parser)]
#[command(name = "cli-walker", version, about)]
struct Cli {
    /// Starting speed in km/h
    #[arg(long, default_value_t = 2.5, value_name = "KMH")]
    speed: f64,

    /// Log raw BLE packets to ~/.cli-walker/ble.log
    #[arg(long)]
    debug: bool,

    /// Do not upload this run's sessions to Strava
    #[arg(long)]
    no_strava: bool,

    /// Simulated pad and sample history in a throwaway directory, for recordings and tests
    #[arg(long, hide = true)]
    demo: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Connect to Strava and upload walking sessions
    Strava {
        #[command(subcommand)]
        command: commands::StravaCommand,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    paint::set_enabled(
        std::io::stdout().is_terminal()
            && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty()),
    );
    match cli.command {
        Some(Command::Strava { command }) => commands::run(command).await,
        None => {
            if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
                anyhow::bail!("the dashboard needs an interactive terminal");
            }
            walk::walk(walk::WalkOptions {
                start_speed: cli.speed.clamp(pad::MIN_SPEED_KMH, pad::MAX_SPEED_KMH),
                debug: cli.debug,
                no_strava: cli.no_strava,
                demo: cli.demo,
            })
            .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_the_documented_invocations() {
        let cli = Cli::try_parse_from(["cli-walker", "--speed", "3", "--no-strava"]).unwrap();
        assert_eq!(cli.speed, 3.0);
        assert!(cli.no_strava && cli.command.is_none());

        for args in [
            &[
                "cli-walker",
                "strava",
                "login",
                "--port",
                "9000",
                "--no-browser",
            ][..],
            &[
                "cli-walker",
                "strava",
                "sync",
                "--dry-run",
                "--all",
                "--limit",
                "3",
            ],
            &["cli-walker", "strava", "threshold", "500m"],
            &["cli-walker", "strava", "mute", "on"],
            &["cli-walker", "strava", "status"],
            &["cli-walker", "strava", "logout"],
        ] {
            Cli::try_parse_from(args).unwrap_or_else(|e| panic!("{args:?}: {e}"));
        }
        assert!(Cli::try_parse_from(["cli-walker", "strava", "mute", "maybe"]).is_err());
    }
}
