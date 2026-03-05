//! CLI parser definition for `cargo ai credentials`.
use clap::{Arg, ArgAction, Command};

/// Builds the `credentials` command schema.
pub fn command() -> Command {
    Command::new("credentials")
        .about("Manage credential storage behavior")
        .subcommand(
            Command::new("store")
                .about("Manage credential storage mode")
                .subcommand(
                    Command::new("status")
                        .about("Show current credential-store mode and backend state"),
                )
                .subcommand(
                    Command::new("set")
                        .about("Set credential-store mode")
                        .arg(
                            Arg::new("mode")
                                .help("Target storage mode")
                                .value_name("MODE")
                                .value_parser(["file", "keychain"])
                                .required(true),
                        )
                        .arg(
                            Arg::new("migrate")
                                .long("migrate")
                                .help("Migrate existing credentials into the target mode")
                                .action(ArgAction::SetTrue),
                        )
                        .arg(
                            Arg::new("yes")
                                .long("yes")
                                .help("Skip interactive confirmation for migration")
                                .requires("migrate")
                                .action(ArgAction::SetTrue),
                        )
                        .arg(
                            Arg::new("dry_run")
                                .long("dry-run")
                                .help("Preview migration actions without writing changes")
                                .requires("migrate")
                                .action(ArgAction::SetTrue),
                        ),
                ),
        )
}
