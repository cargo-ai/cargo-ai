//! CLI parser definition for `cargo ai hatch`.
use clap::{Arg, Command};

/// Builds the `hatch` command schema.
pub fn command() -> Command {
    Command::new("hatch")
        .about("Hatch a new AI agent from a JSON config")
        .arg(
            Arg::new("name")
                .help("Agent name or local .json config path (shorthand)")
                .required(true),
        )
        .arg(
            Arg::new("check")
                .long("check")
                .help("Validate scaffold and compile path with `cargo check` (no binary export)")
                .required(false)
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("config")
                .long("config")
                .short('c')
                .help("Local path to the agent .json configuration file")
                .value_name("FILE")
                .num_args(1),
        )
        .arg(
            Arg::new("force")
                .long("force")
                .short('f')
                .help("Overwrite existing output binary and replace any kept internal workspace")
                .required(false)
                .action(clap::ArgAction::SetTrue),
        )
        .arg(
            Arg::new("keep_project")
                .long("keep-project")
                .help("Preserve the internal hatched project workspace for inspection")
                .required(false)
                .action(clap::ArgAction::SetTrue),
        )
}
