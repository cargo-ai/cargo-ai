//! CLI parser definition for `cargo ai add guidance`.
use clap::{Arg, ArgAction, Command};

/// Builds the `guidance` subcommand schema.
pub fn command() -> Command {
    Command::new("guidance")
        .about("Install Cargo AI authoring guidance for one or more assistants")
        .arg(
            Arg::new("style")
                .long("style")
                .help("Assistant discovery style to install (repeat for multiple styles)")
                .required(true)
                .value_name("STYLE")
                .value_parser(["codex", "claude"])
                .action(ArgAction::Append)
                .num_args(1),
        )
}
