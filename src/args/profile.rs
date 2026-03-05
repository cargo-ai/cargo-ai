//! CLI parser definition for `cargo ai profile`.
use clap::{Arg, ArgAction, ArgGroup, Command};

/// Builds the `profile` command schema and nested subcommands.
pub fn command() -> Command {
    Command::new("profile")
        .about("Manage connection profiles and profile auth behavior")
        .subcommand(Command::new("list").about("List all configured profiles"))
        .subcommand(
            Command::new("show")
                .about("Show detailed information for a specific profile")
                .arg(
                    Arg::new("name")
                        .help("Name of the profile to display")
                        .required(true)
                        .value_name("NAME"),
                ),
        )
        .subcommand(
            Command::new("add")
                .about("Add a new connection profile or overwrite an existing one")
                .arg(
                    Arg::new("name")
                        .help("Name of the profile to add or update")
                        .required(true)
                        .value_name("NAME"),
                )
                .arg(
                    Arg::new("server")
                        .long("server")
                        .short('s')
                        .help("LLM server (e.g., openai or ollama)")
                        .required(true)
                        .value_name("SERVER"),
                )
                .arg(
                    Arg::new("model")
                        .long("model")
                        .short('m')
                        .help("LLM model identifier (e.g., gpt-4o, mistral)")
                        .required(true)
                        .value_name("MODEL"),
                )
                .arg(
                    Arg::new("auth")
                        .long("auth")
                        .help("Optional auth mode (default: none)")
                        .required(false)
                        .value_name("MODE")
                        .value_parser(["none", "api_key", "openai_account"]),
                )
                .arg(
                    Arg::new("url")
                        .long("url")
                        .help("Custom transformer server URL (HTTPS preferred)")
                        .required(false)
                        .value_name("URL"),
                )
                .arg(
                    Arg::new("description")
                        .long("description")
                        .short('d')
                        .help("Optional description for the profile")
                        .required(false)
                        .value_name("TEXT"),
                )
                .arg(
                    Arg::new("default")
                        .long("default")
                        .help("Set this profile as the default")
                        .action(ArgAction::SetTrue),
                ),
        )
        .subcommand(
            Command::new("remove")
                .about("Remove an existing connection profile by name")
                .arg(
                    Arg::new("name")
                        .help("Name of the profile to remove")
                        .required(true)
                        .value_name("NAME"),
                ),
        )
        .subcommand(
            Command::new("auth")
                .about("Manage per-profile auth mode")
                .subcommand(
                    Command::new("set")
                        .about("Set auth mode for a profile")
                        .arg(
                            Arg::new("name")
                                .help("Profile name")
                                .required(true)
                                .value_name("NAME"),
                        )
                        .arg(
                            Arg::new("mode")
                                .help("Auth mode")
                                .required(true)
                                .value_name("MODE")
                                .value_parser(["none", "api_key", "openai_account"]),
                        ),
                )
                .subcommand(
                    Command::new("status")
                        .about("Show auth mode for one profile or all profiles")
                        .arg(
                            Arg::new("name")
                                .help("Optional profile name")
                                .required(false)
                                .value_name("NAME"),
                        ),
                ),
        )
        .subcommand(
            Command::new("token")
                .about("Manage API keys for profiles")
                .subcommand(
                    Command::new("set")
                        .about("Store API key material for a profile")
                        .group(
                            ArgGroup::new("token_source")
                                .args(["token", "stdin", "env"])
                                .required(true),
                        )
                        .arg(
                            Arg::new("name")
                                .help("Profile name")
                                .required(true)
                                .value_name("NAME"),
                        )
                        .arg(
                            Arg::new("token")
                                .long("token")
                                .help("Raw API token value")
                                .required(false)
                                .value_name("TOKEN")
                                .num_args(1),
                        )
                        .arg(
                            Arg::new("stdin")
                                .long("stdin")
                                .help("Read API token from standard input")
                                .required(false)
                                .action(ArgAction::SetTrue),
                        )
                        .arg(
                            Arg::new("env")
                                .long("env")
                                .help("Read API token from environment variable")
                                .required(false)
                                .value_name("ENV_VAR")
                                .num_args(1),
                        ),
                )
                .subcommand(
                    Command::new("clear")
                        .about("Clear API token for a profile")
                        .arg(
                            Arg::new("name")
                                .help("Profile name")
                                .required(true)
                                .value_name("NAME"),
                        )
                        .arg(
                            Arg::new("yes")
                                .long("yes")
                                .help("Skip interactive confirmation")
                                .required(false)
                                .action(ArgAction::SetTrue),
                        ),
                )
                .subcommand(
                    Command::new("status")
                        .about("Show whether a profile has an API token")
                        .arg(
                            Arg::new("name")
                                .help("Profile name")
                                .required(true)
                                .value_name("NAME"),
                        ),
                ),
        )
}
