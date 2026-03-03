//! CLI parser definition for `cargo ai version`.
use clap::{Arg, ArgAction, Command};

/// Builds the `version` command schema.
pub fn command() -> Command {
    Command::new("version")
        .about("Print version information and manage update-check behavior")
        .arg(
            Arg::new("check")
                .long("check")
                .help("Force an immediate crates.io update check")
                .action(ArgAction::SetTrue)
                .conflicts_with("update_mode"),
        )
        .arg(
            Arg::new("update_mode")
                .long("update-mode")
                .help("Persist update-check mode")
                .value_name("MODE")
                .value_parser(["check", "off"])
                .num_args(1),
        )
}
