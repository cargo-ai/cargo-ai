//! CLI parser definition for `cargo ai init`.
use clap::{Arg, Command};

/// Builds the `init` command schema.
pub fn command() -> Command {
    Command::new("init")
        .about("Initialize a Cargo-AI project in an existing directory")
        .arg(
            Arg::new("path")
                .help("Target directory to initialize (defaults to current directory)")
                .required(false)
                .default_value("."),
        )
        .arg(
            Arg::new("template")
                .long("template")
                .help("Starter template overlay to apply")
                .value_name("TEMPLATE")
                .value_parser(["codex", "claude"])
                .num_args(1),
        )
        .arg(
            Arg::new("vcs")
                .long("vcs")
                .help("Version control setup to initialize")
                .value_name("VCS")
                .value_parser(["git", "none"])
                .default_value("git")
                .num_args(1),
        )
}
