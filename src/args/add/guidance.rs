//! CLI parser definition for `cargo ai add guidance`.
use clap::{Arg, Command};

/// Builds the `guidance` subcommand schema.
pub fn command() -> Command {
    Command::new("guidance")
        .about("Write a local guidance file for Cargo AI agent authoring")
        .arg(
            Arg::new("style")
                .long("style")
                .help("Guidance style to write")
                .required(true)
                .value_name("STYLE")
                .value_parser(["codex"])
                .num_args(1),
        )
}
