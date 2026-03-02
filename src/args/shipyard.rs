//! CLI parser definition for `cargo ai shipyard`.
use clap::{Arg, Command};

/// Builds the hidden `shipyard` command schema.
pub fn command() -> Command {
    Command::new("shipyard")
        .hide(true)
        .about("Launch Shipyard desktop UI (exploratory)")
        .arg(
            Arg::new("experimental")
                .long("experimental")
                .help("Internal: enable experimental Shipyard UI launch")
                .hide(true)
                .action(clap::ArgAction::SetTrue),
        )
}
