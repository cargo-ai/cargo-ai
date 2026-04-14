//! CLI parser definition for `cargo ai new`.
use clap::{Arg, Command};

/// Builds the `new` command schema.
pub fn command() -> Command {
    Command::new("new")
        .about("Create a new Cargo-AI project directory")
        .arg(
            Arg::new("path")
                .help("Directory path for the new project")
                .required(true),
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
