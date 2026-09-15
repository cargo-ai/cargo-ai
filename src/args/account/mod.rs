//! CLI parser definitions for `cargo ai account`.
use clap::{Arg, ArgAction, Command};

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
                )
                .arg(
                    Arg::new("yes")
                        .long("yes")
                        .action(ArgAction::SetTrue)
                        .help("Accept that fresh confirmation can reactivate the account and cancel pending deletion"),
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
        .subcommand(
            Command::new("deactivate")
                .about("Stop hosted access and hide public content, retaining account data")
                .long_about("Stop hosted access and hide public packages and agents. Data and sharing settings are retained. Fresh email confirmation reactivates the account and cancels pending deletion before an administrator begins removal. Local projects, installed applications and provider credentials are preserved.\n\nWith --request-deletion, live account removal is processed manually and may take several days. Restricted recovery copies and operational records may remain until verified expiry or separate retirement; there is no universal expiry deadline. Restored copies must exclude completed deletions before returning to service. See the account guide's Retained copies section.")
                .arg(Arg::new("request-deletion").long("request-deletion")
                    .action(ArgAction::SetTrue)
                    .help("Also request manual removal of the account and live hosted data; restricted recovery copies follow the retention policy"))
                .arg(Arg::new("yes").long("yes").action(ArgAction::SetTrue)
                    .help("Confirm ordinary deactivation without prompting; deletion requests still require email confirmation"))
                .arg(Arg::new("confirm-email").long("confirm-email").value_name("EMAIL")
                    .requires("request-deletion")
                    .help("Confirm a deletion request using the configured account email; this does not select another account")),
        )
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
}
