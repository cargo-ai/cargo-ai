//! CLI parser definitions for `cargo ai account`.
//!
//! Larger nested branches (`mail`, `agents`) are split into dedicated modules.
use clap::{Arg, Command};

mod agents;
mod mail;

/// Builds the `account` command schema and all nested subcommands.
pub fn command() -> Command {
    Command::new("account")
        .about("Manage account lifecycle")
        .subcommand(
            Command::new("register")
                .about("Register a new account by email")
                .arg(
                    Arg::new("email")
                        .help("Email address to register")
                        .required(true)
                        .value_name("EMAIL"),
                ),
        )
        .subcommand(
            Command::new("confirm")
                .about("Confirm an account using the temporary code")
                .arg(
                    Arg::new("code")
                        .help("Temporary confirmation code from email")
                        .required(true)
                        .value_name("CODE"),
                ),
        )
        .subcommand(Command::new("status").about("Show account status"))
        .subcommand(mail::command())
        .subcommand(
            Command::new("handle")
                .about("Get or set account handle")
                .arg(
                    Arg::new("set")
                        .long("set")
                        .help("Set a new handle (if omitted, returns current handle)")
                        .required(false)
                        .value_name("HANDLE")
                        .num_args(1),
                ),
        )
        .subcommand(agents::command())
}
