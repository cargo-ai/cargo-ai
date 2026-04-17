//! CLI parser definition for `cargo ai add tool`.
use clap::{Arg, Command};

/// Builds the `tool` subcommand schema.
pub fn command() -> Command {
    Command::new("tool")
        .about("Scaffold a new local source-backed Cargo AI tool")
        .arg(
            Arg::new("name")
                .help("Tool name")
                .required(true)
                .value_name("NAME"),
        )
}
