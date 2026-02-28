//! CLI parser definition for `cargo ai hatch`.
use clap::{Arg, Command};

/// Builds the `hatch` command schema.
pub fn command() -> Command {
    Command::new("hatch")
        .about("Hatch a new AI agent from a JSON config")
        .arg(
            Arg::new("name")
                .help("Name of the new agent project")
                .required(true),
        )
        .arg(
            Arg::new("config")
                .long("config")
                .short('c')
                .help("Path to the agent configuration (local .json file or remote registry name)")
                .value_name("FILE")
                .num_args(1),
        )
}
