//! CLI parser definition for `cargo ai auth`.
use clap::{Arg, ArgAction, Command};

/// Builds the `auth` command schema and nested subcommands.
pub fn command() -> Command {
    Command::new("auth")
        .about("Manage OpenAI account login state")
        .subcommand(
            Command::new("login")
                .about("Authenticate via provider login")
                .subcommand(
                    Command::new("openai")
                        .about("Login with an OpenAI account")
                        .arg(
                            Arg::new("profile")
                                .long("profile")
                                .help("Optional profile to mark as openai_account mode")
                                .required(false)
                                .value_name("NAME")
                                .num_args(1),
                        )
                        .arg(
                            Arg::new("set_default")
                                .long("set-default")
                                .help("Set --profile as the default profile")
                                .requires("profile")
                                .required(false)
                                .action(ArgAction::SetTrue),
                        ),
                ),
        )
        .subcommand(
            Command::new("status")
                .about("Show OpenAI account-login session status")
                .arg(
                    Arg::new("json")
                        .long("json")
                        .help("Print machine-readable JSON output")
                        .required(false)
                        .action(ArgAction::SetTrue),
                ),
        )
        .subcommand(
            Command::new("logout")
                .about("Log out of OpenAI for Cargo AI (local by default)")
                .arg(
                    Arg::new("global")
                        .long("global")
                        .help("Also log out Codex/OpenAI session globally via `codex logout`")
                        .required(false)
                        .action(ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("revoke")
                        .long("revoke")
                        .help("Deprecated alias for --global")
                        .required(false)
                        .action(ArgAction::SetTrue),
                )
                .arg(
                    Arg::new("yes")
                        .long("yes")
                        .help("Skip interactive confirmation")
                        .required(false)
                        .action(ArgAction::SetTrue),
                ),
        )
}
