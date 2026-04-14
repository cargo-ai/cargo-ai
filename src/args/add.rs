//! CLI parser definition for `cargo ai add`.
use clap::Command;

mod guidance;
mod tool;

/// Builds the `add` command schema and nested subcommands.
pub fn command() -> Command {
    Command::new("add")
        .about("Add lightweight support artifacts")
        .subcommand(guidance::command())
        .subcommand(tool::command())
}
