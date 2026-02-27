use clap::{Arg, Command};

pub fn command() -> Command {
    Command::new("mail")
        .about("Send account mail")
        .subcommand(
            Command::new("test")
                .about("Send a test email to your account email")
                .arg(
                    Arg::new("subject")
                        .long("subject")
                        .help("Optional subject override (quote values with spaces)")
                        .required(false)
                        .value_name("TEXT")
                        .num_args(1),
                )
                .arg(
                    Arg::new("text")
                        .long("text")
                        .help("Optional body override (quote values with spaces)")
                        .required(false)
                        .value_name("TEXT")
                        .num_args(1),
                ),
        )
        .subcommand(
            Command::new("prefs")
                .about("View or set account email delivery preferences")
                .arg(
                    Arg::new("disable_all")
                        .long("disable-all")
                        .help("Disable all account emails")
                        .required(false)
                        .action(clap::ArgAction::SetTrue)
                        .conflicts_with("enable_all"),
                )
                .arg(
                    Arg::new("enable_all")
                        .long("enable-all")
                        .help("Enable all account emails")
                        .required(false)
                        .action(clap::ArgAction::SetTrue)
                        .conflicts_with("disable_all"),
                ),
        )
}
