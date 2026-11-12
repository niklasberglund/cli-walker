//! CLI-Walker: a terminal dashboard for the Kingsmith WalkingPad A1 Pro.
//!
//! This is the Rust port, which is being built up one layer at a time. The Kotlin/Native
//! sources under `src/nativeMain` remain the reference until it reaches parity.

mod tui;

use std::io::IsTerminal;

use clap::Parser;

/// Log walking sessions from a Kingsmith WalkingPad A1 Pro.
#[derive(Debug, Parser)]
#[command(name = "cli-walker", version, about)]
struct Cli {}

fn main() -> anyhow::Result<()> {
    let _cli = Cli::parse();
    if std::io::stdout().is_terminal() {
        tui::run()?;
    } else {
        println!(
            "cli-walker {}: the Rust port is not functional yet.",
            env!("CARGO_PKG_VERSION")
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }
}
