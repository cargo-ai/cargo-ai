//! Local declaration inspection selectors.
use clap::{Arg, ArgGroup, Command};

pub fn command() -> Command {
    Command::new("requirements")
        .about("Inspect declared agent and package requirements without running them")
        .arg(
            Arg::new("config")
                .long("config")
                .value_name("FILE")
                .num_args(1),
        )
        .arg(
            Arg::new("build_profile")
                .long("build-profile")
                .value_name("NAME")
                .num_args(1),
        )
        .group(ArgGroup::new("selector").args(["config", "build_profile"]).required(true))
        .after_help("A build profile selects declared package contents; it is not a model connection profile. This command reads definitions only and does not assess model compatibility.")
}
